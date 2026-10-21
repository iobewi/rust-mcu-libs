//! `GET` of a body the caller reads itself (a stream), and the redirect policy
//! that goes with it.
//!
//! [`crate::drain_response`] discards the body: it serves request/response
//! exchanges. A stream needs the response head and then the connection, left
//! positioned at the first body byte. TLS, DNS and the redirect *following*
//! (a new connection, hence a new certificate verification of the new host)
//! stay with the caller.

use alloc::{format, string::String};
use embedded_io_async::{Read, Write};

/// Longest `Location` kept; a longer one is refused rather than truncated.
const MAX_LOCATION: usize = 256;

/// Most response headers parsed; one more is [`HeadError::TooManyHeaders`].
///
/// Real servers send more than a minimal Icecast does (a CDN or a `Set-Cookie`
/// plus CORS plus `icy-*` set reaches the 20s, a rich one 26). The table lives on
/// the stack for the parse only: 16 bytes per entry on a 32-bit target.
pub const MAX_HEADERS: usize = 48;

/// Write a `GET` request (HTTP/1.1, `Connection: close`) and flush it.
///
/// `host` is the value of the `Host` header: for a non-default port it must
/// include it (`host:8000`), which [`Target::authority`] gives.
/// `headers` are extra `(name, value)` pairs; CR/LF in any field is refused.
pub async fn get<S>(
    session: &mut S,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<(), String>
where
    S: Write,
    S::Error: core::fmt::Display,
{
    let bad = |v: &str| v.contains('\r') || v.contains('\n');
    if bad(host)
        || bad(path)
        || !path.starts_with('/')
        || headers.iter().any(|(n, v)| bad(n) || bad(v))
    {
        return Err(String::from("invalid HTTP request field"));
    }
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    session
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write failed: {e}"))?;
    session
        .flush()
        .await
        .map_err(|e| format!("flush failed: {e}"))
}

/// What the response head says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub content_length: Option<usize>,
    pub chunked: bool,
    /// `Location`, for a redirect.
    pub location: Option<String>,
    /// Body bytes already read together with the head: `buf[body_start..filled]`.
    pub body_start: usize,
    pub filled: usize,
}

/// Why no response head could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadError<E> {
    /// The transport failed (a TLS or TCP error).
    Io(E),
    /// The peer closed before the head was complete.
    Closed,
    /// The head does not fit in the buffer.
    TooLarge,
    /// More than [`MAX_HEADERS`] headers: refused, never truncated, so that
    /// a framing or `Location` header past the limit cannot go unseen.
    TooManyHeaders,
    /// The head is not HTTP, a `Location` is too long, or a `Transfer-Encoding`
    /// is other than a single `chunked`.
    Malformed,
}

/// Read until the end of the response head. The body bytes that arrived with it
/// stay in `buf[head.body_start..head.filled]`; the rest is still in `session`.
pub async fn read_head<S: Read>(
    session: &mut S,
    buf: &mut [u8],
) -> Result<Head, HeadError<S::Error>> {
    let mut filled = 0;
    let body_start = loop {
        if filled == buf.len() {
            return Err(HeadError::TooLarge);
        }
        let n = session
            .read(&mut buf[filled..])
            .await
            .map_err(HeadError::Io)?;
        if n == 0 {
            return Err(HeadError::Closed);
        }
        // Rescan the last three bytes of the previous read too.
        let from = filled.saturating_sub(3);
        filled += n;
        if let Some(pos) = buf[from..filled].windows(4).position(|w| w == b"\r\n\r\n") {
            break from + pos + 4;
        }
    };

    let mut parsed = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut response = httparse::Response::new(&mut parsed);
    match response.parse(&buf[..body_start]) {
        Ok(httparse::Status::Complete(_)) => {}
        Err(httparse::Error::TooManyHeaders) => return Err(HeadError::TooManyHeaders),
        _ => return Err(HeadError::Malformed),
    }
    let mut head = Head {
        status: response.code.ok_or(HeadError::Malformed)?,
        content_length: None,
        chunked: false,
        location: None,
        body_start,
        filled,
    };
    let mut transfer_encodings = 0;
    for h in response.headers.iter() {
        let Ok(value) = core::str::from_utf8(h.value) else {
            continue;
        };
        let value = value.trim();
        if h.name.eq_ignore_ascii_case("content-length") {
            head.content_length = value.parse().ok();
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            // Only a lone `chunked` is understood. Several headers, another
            // coding or a list is refused rather than guessed: a body framed
            // differently from what the caller decodes would desynchronise it.
            transfer_encodings += 1;
            if transfer_encodings > 1 || !value.eq_ignore_ascii_case("chunked") {
                return Err(HeadError::Malformed);
            }
            head.chunked = true;
        } else if h.name.eq_ignore_ascii_case("location") {
            if value.len() > MAX_LOCATION || value.contains(['\r', '\n']) {
                return Err(HeadError::Malformed);
            }
            head.location = Some(String::from(value));
        }
    }
    // Chunked framing wins over any `Content-Length` (RFC 9112 6.3).
    if head.chunked {
        head.content_length = None;
    }
    Ok(head)
}

/// Where a request goes: what a URL or a redirect resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Why a URL or a redirect was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    /// Neither `http://` nor `https://`.
    Scheme,
    /// No host, a userinfo, a bad port or a control character.
    Malformed,
    /// A redirect from `https` to `http`: the guarantee would be lost silently.
    Downgrade,
}

impl Target {
    /// Parse an absolute `http://` or `https://` URL.
    pub fn parse(url: &str) -> Result<Self, TargetError> {
        let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(TargetError::Scheme);
        };
        if rest.chars().any(|c| c.is_ascii_control() || c == ' ') {
            return Err(TargetError::Malformed);
        }
        // A fragment is never sent to the server.
        let rest = rest.split('#').next().unwrap_or(rest);
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], &rest[i..]),
            Some(i) => (&rest[..i], &rest[i..]), // query without path
            None => (rest, "/"),
        };
        // A userinfo would make `https://trusted@evil/` read as the wrong host.
        if authority.is_empty() || authority.contains('@') || authority.starts_with('[') {
            return Err(TargetError::Malformed);
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>().map_err(|_| TargetError::Malformed)?,
            ),
            None => (authority, if tls { 443 } else { 80 }),
        };
        if host.is_empty() || port == 0 {
            return Err(TargetError::Malformed);
        }
        let path = if path.starts_with('/') {
            String::from(path)
        } else {
            format!("/{path}")
        };
        Ok(Self {
            tls,
            host: String::from(host),
            port,
            path,
        })
    }

    /// The `Host` header value: the port only when it is not the scheme's default.
    pub fn authority(&self) -> String {
        if self.port == if self.tls { 443 } else { 80 } {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The target a `Location` designates, seen from `self`.
    ///
    /// Absolute URLs and absolute paths (`/x`) are followed; a relative path
    /// is refused as `Malformed` (not worth the resolution rules for a stream).
    /// `https` to `http` is [`TargetError::Downgrade`], whatever the host. The
    /// caller connects to the result with a new connection, so the new host is
    /// verified against its own certificate, never against the previous one.
    pub fn redirect(&self, location: &str) -> Result<Self, TargetError> {
        if location.starts_with('/') && !location.starts_with("//") {
            if location.chars().any(|c| c.is_ascii_control() || c == ' ') {
                return Err(TargetError::Malformed);
            }
            let location = location.split('#').next().unwrap_or(location);
            return Ok(Self {
                path: String::from(location),
                ..self.clone()
            });
        }
        if !location.contains("://") {
            return Err(TargetError::Malformed);
        }
        let next = Self::parse(location)?;
        if self.tls && !next.tls {
            return Err(TargetError::Downgrade);
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Duplex, block_on};
    use alloc::vec::Vec;

    fn peer(bytes: &[u8]) -> Duplex {
        let d = Duplex::default();
        d.state.borrow_mut().input.extend(bytes.iter().copied());
        d
    }

    #[test]
    fn get_writes_the_exact_request() {
        let mut d = Duplex::default();
        block_on(get(
            &mut d,
            "radio.example",
            "/live.mp3",
            &[("Icy-MetaData", "0")],
        ))
        .unwrap();
        assert_eq!(
            d.state.borrow().output,
            b"GET /live.mp3 HTTP/1.1\r\nHost: radio.example\r\nConnection: close\r\nIcy-MetaData: 0\r\n\r\n"
        );
    }

    #[test]
    fn get_rejects_injection_and_relative_paths() {
        let mut d = Duplex::default();
        for (host, path, h) in [
            ("a\r\nX: y", "/", ("A", "b")),
            ("a", "/p\r\n", ("A", "b")),
            ("a", "p", ("A", "b")),
            ("a", "/", ("A", "b\nC: d")),
        ] {
            assert!(block_on(get(&mut d, host, path, &[h])).is_err());
        }
        assert!(d.state.borrow().output.is_empty());
    }

    #[test]
    fn the_head_leaves_the_body_start_for_the_caller() {
        let mut d = peer(b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nicy-br: 128\r\n\r\nABC");
        let mut buf = [0u8; 256];
        let head = block_on(read_head(&mut d, &mut buf)).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.content_length, None);
        assert!(!head.chunked);
        assert_eq!(&buf[head.body_start..head.filled], b"ABC");
    }

    #[test]
    fn the_head_reports_redirect_length_and_chunking() {
        let mut d = peer(b"HTTP/1.1 302 Found\r\nlocation: https://b.example/x\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n");
        let mut buf = [0u8; 256];
        let head = block_on(read_head(&mut d, &mut buf)).unwrap();
        assert_eq!(head.status, 302);
        assert_eq!(head.location.as_deref(), Some("https://b.example/x"));
        // Chunked framing wins over Content-Length.
        assert_eq!(head.content_length, None);
        assert!(head.chunked);
    }

    #[test]
    fn only_a_lone_chunked_transfer_encoding_is_understood() {
        let mut buf = [0u8; 256];
        for ok in ["chunked", "Chunked", " chunked "] {
            let raw = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: {ok}\r\n\r\n");
            assert!(
                block_on(read_head(&mut peer(raw.as_bytes()), &mut buf))
                    .unwrap()
                    .chunked
            );
        }
        for bad in [
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: gzip\r\n\r\n",
        ] {
            assert_eq!(
                block_on(read_head(&mut peer(bad.as_bytes()), &mut buf)),
                Err(HeadError::Malformed),
                "{bad}"
            );
        }
    }

    #[test]
    fn host_carries_the_port_only_when_it_is_not_the_default() {
        let auth = |u: &str| Target::parse(u).unwrap().authority();
        assert_eq!(auth("https://a.example/"), "a.example");
        assert_eq!(auth("https://a.example:443/"), "a.example");
        assert_eq!(auth("https://a.example:8443/x"), "a.example:8443");
        assert_eq!(auth("http://a.example:8000"), "a.example:8000");
        assert_eq!(auth("http://a.example:443/"), "a.example:443");
        assert_eq!(auth("https://a.example:80/"), "a.example:80");
    }

    #[test]
    fn a_fragment_is_dropped_and_never_sent() {
        let t = Target::parse("https://a.example/live.mp3?x=1#top").unwrap();
        assert_eq!(t.path, "/live.mp3?x=1");
        assert_eq!(Target::parse("https://a.example#f").unwrap().path, "/");
        assert_eq!(Target::parse("https://a.example:81#f").unwrap().port, 81);
        let r = t.redirect("/next#frag").unwrap();
        assert_eq!(r.path, "/next");
        assert_eq!(t.redirect("https://b.example/y#z").unwrap().path, "/y");
    }

    #[test]
    fn a_terminator_split_across_reads_is_found() {
        struct Dribble(Vec<u8>);
        impl embedded_io_async::ErrorType for Dribble {
            type Error = core::convert::Infallible;
        }
        impl Read for Dribble {
            async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                out[0] = self.0.remove(0);
                Ok(1)
            }
        }
        let mut d = Dribble(b"HTTP/1.1 204 No Content\r\n\r\nxy".to_vec());
        let mut buf = [0u8; 64];
        let head = block_on(read_head(&mut d, &mut buf)).unwrap();
        assert_eq!(head.status, 204);
        assert_eq!(head.body_start, head.filled);
    }

    #[test]
    fn head_errors_are_distinguished() {
        let mut buf = [0u8; 32];
        assert_eq!(
            block_on(read_head(&mut peer(b"HTTP/1.1 200 OK\r\n"), &mut buf)),
            Err(HeadError::Closed)
        );
        assert_eq!(
            block_on(read_head(&mut peer(&[b'a'; 64]), &mut buf)),
            Err(HeadError::TooLarge)
        );
        assert_eq!(
            block_on(read_head(
                &mut peer(b"garbage garbage garbage\r\n\r\n"),
                &mut buf
            )),
            Err(HeadError::Malformed)
        );
    }

    /// A complete head with `n` filler headers, the interesting ones last.
    fn head_with(n: usize, tail: &str) -> Vec<u8> {
        let mut v = b"HTTP/1.1 200 OK\r\n".to_vec();
        for i in 0..n {
            v.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        v.extend_from_slice(tail.as_bytes());
        v.extend_from_slice(b"\r\n");
        v
    }

    #[test]
    fn the_header_limit_is_exact_and_refused_beyond_it() {
        let mut buf = [0u8; 4096];
        // Fillers plus the one `Content-Length` that closes the list.
        for (fillers, expected) in [
            (23, Ok(())),                                  // 24 headers: the old limit
            (25, Ok(())),                                  // 26 (walmradio)
            (MAX_HEADERS - 1, Ok(())),                     // 48
            (MAX_HEADERS, Err(HeadError::TooManyHeaders)), // 49
            (MAX_HEADERS + 30, Err(HeadError::TooManyHeaders)),
        ] {
            let bytes = head_with(fillers, "Content-Length: 7\r\n");
            let got = block_on(read_head(&mut peer(&bytes), &mut buf));
            match expected {
                Ok(()) => assert_eq!(got.unwrap().content_length, Some(7), "{fillers}"),
                Err(e) => assert_eq!(got, Err(e), "{fillers}"),
            }
        }
    }

    #[test]
    fn a_header_past_the_limit_is_never_silently_dropped() {
        // Framing hidden after the limit must not be skipped: the whole head is
        // refused instead of read as a body-less or length-less response.
        let mut buf = [0u8; 4096];
        let bytes = head_with(MAX_HEADERS, "Transfer-Encoding: chunked\r\n");
        assert_eq!(
            block_on(read_head(&mut peer(&bytes), &mut buf)),
            Err(HeadError::TooManyHeaders)
        );
        // At the limit, a trailing `Location` and framing are still seen.
        let bytes = head_with(
            MAX_HEADERS - 2,
            "Location: /next\r\nTransfer-Encoding: chunked\r\n",
        );
        let head = block_on(read_head(&mut peer(&bytes), &mut buf)).unwrap();
        assert_eq!(head.location.as_deref(), Some("/next"));
        assert!(head.chunked);
    }

    #[test]
    fn a_head_larger_than_the_buffer_stays_too_large_whatever_its_headers() {
        // Few headers but too big for the buffer, and many headers in a buffer
        // that cannot hold them: both are `TooLarge`, not a parse error.
        let big = format!("X-Big: {}\r\n", "a".repeat(600));
        let bytes = head_with(3, &big);
        let mut small = [0u8; 256];
        assert_eq!(
            block_on(read_head(&mut peer(&bytes), &mut small)),
            Err(HeadError::TooLarge)
        );
        let bytes = head_with(MAX_HEADERS + 10, "");
        let mut small = [0u8; 256];
        assert_eq!(
            block_on(read_head(&mut peer(&bytes), &mut small)),
            Err(HeadError::TooLarge)
        );
        // The same many-header head fits and is refused for its count, not its size.
        let mut room = [0u8; 2048];
        assert_eq!(
            block_on(read_head(&mut peer(&bytes), &mut room)),
            Err(HeadError::TooManyHeaders)
        );
    }

    #[test]
    fn existing_protections_hold_with_many_headers() {
        let mut buf = [0u8; 4096];
        // Duplicate or non-chunked Transfer-Encoding, long Location: still refused.
        let dup = head_with(
            30,
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n",
        );
        assert_eq!(
            block_on(read_head(&mut peer(&dup), &mut buf)),
            Err(HeadError::Malformed)
        );
        let gzip = head_with(30, "Transfer-Encoding: gzip, chunked\r\n");
        assert_eq!(
            block_on(read_head(&mut peer(&gzip), &mut buf)),
            Err(HeadError::Malformed)
        );
        let long = head_with(30, &format!("Location: /{}\r\n", "a".repeat(MAX_LOCATION)));
        assert_eq!(
            block_on(read_head(&mut peer(&long), &mut buf)),
            Err(HeadError::Malformed)
        );
        // Chunked still wins over Content-Length.
        let both = head_with(30, "Content-Length: 5\r\nTransfer-Encoding: chunked\r\n");
        let h = block_on(read_head(&mut peer(&both), &mut buf)).unwrap();
        assert!(h.chunked && h.content_length.is_none());
    }

    #[test]
    fn urls_parse_with_default_ports_and_paths() {
        let t = Target::parse("https://icecast.example/live.mp3?x=1").unwrap();
        assert_eq!(
            (t.tls, t.host.as_str(), t.port, t.path.as_str()),
            (true, "icecast.example", 443, "/live.mp3?x=1")
        );
        let t = Target::parse("http://a.example:8000").unwrap();
        assert_eq!((t.tls, t.port, t.path.as_str()), (false, 8000, "/"));
        let t = Target::parse("http://a.example?q").unwrap();
        assert_eq!(t.path, "/?q");
    }

    #[test]
    fn urls_that_could_be_misread_are_refused() {
        assert_eq!(Target::parse("ftp://a/"), Err(TargetError::Scheme));
        assert_eq!(Target::parse("a.example/x"), Err(TargetError::Scheme));
        for bad in [
            "https://trusted@evil.example/",
            "https:///x",
            "https://a:0/",
            "https://a:x/",
            "https://a b/",
            "https://[::1]/",
        ] {
            assert_eq!(Target::parse(bad), Err(TargetError::Malformed), "{bad}");
        }
    }

    #[test]
    fn a_redirect_to_another_host_is_a_new_target_never_a_reuse() {
        let a = Target::parse("https://a.example/s").unwrap();
        let b = a.redirect("https://b.example:8443/t").unwrap();
        assert_eq!((b.host.as_str(), b.port, b.tls), ("b.example", 8443, true));
        let same = a.redirect("/other").unwrap();
        assert_eq!(
            (same.host.as_str(), same.path.as_str()),
            ("a.example", "/other")
        );
        // Scheme-relative and relative locations are not followed.
        assert_eq!(a.redirect("//evil.example/x"), Err(TargetError::Malformed));
        assert_eq!(a.redirect("x/y"), Err(TargetError::Malformed));
    }

    #[test]
    fn a_redirect_never_downgrades_https_but_may_upgrade_http() {
        let secure = Target::parse("https://a.example/").unwrap();
        assert_eq!(
            secure.redirect("http://a.example/"),
            Err(TargetError::Downgrade)
        );
        assert_eq!(
            secure.redirect("http://other.example/"),
            Err(TargetError::Downgrade)
        );
        let plain = Target::parse("http://a.example/").unwrap();
        assert!(plain.redirect("https://a.example/").unwrap().tls);
        assert!(!plain.redirect("http://b.example/").unwrap().tls);
    }
}
