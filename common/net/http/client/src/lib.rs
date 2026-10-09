#![cfg_attr(not(test), no_std)]

//! Outbound HTTP/1.1 primitives over any connected async transport.
//! TLS connection setup and certificate policy belong to the platform adapter.
//! Knows nothing about an HTTP server, `picoserve` or any listener.

extern crate alloc;

#[cfg(test)]
mod testutil;

/// Outbound WebSocket upgrade and frame handling (feature `websocket`).
#[cfg(feature = "websocket")]
pub mod websocket;

use alloc::{format, string::String};
use embedded_io_async::{Read, Write};

/// Send a JSON POST while leaving the connection open for another request.
pub async fn post_json<S>(
    session: &mut S,
    host: &str,
    path: &str,
    bearer: &str,
    json: &str,
) -> Result<(), String>
where
    S: Read + Write,
    S::Error: core::fmt::Display,
{
    if [host, path, bearer]
        .iter()
        .any(|v| v.contains('\r') || v.contains('\n'))
        || !path.starts_with('/')
    {
        return Err(String::from("invalid HTTP request field"));
    }
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}",
        json.len()
    );
    session
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write failed: {e}"))?;
    session
        .flush()
        .await
        .map_err(|e| format!("flush failed: {e}"))?;
    Ok(())
}

/// Consume exactly one response and report whether the stream can be reused.
/// The buffer must hold the complete response headers; bodies are discarded
/// incrementally, including Content-Length and chunked transfer encoding.
/// Requests and responses are strictly lock-step: bytes pipelined after the
/// response may be read into the scratch buffer and dropped.
pub async fn drain_response<S>(session: &mut S, resp_buf: &mut [u8]) -> Result<(u16, bool), String>
where
    S: Read + Write,
    S::Error: core::fmt::Display,
{
    let mut filled = 0;
    let header_end = loop {
        if filled >= resp_buf.len() {
            return Err(String::from(
                "response headers too large for the scratch buffer",
            ));
        }
        let n = session
            .read(&mut resp_buf[filled..])
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            return Err(String::from(
                "connection closed while reading response headers",
            ));
        }
        filled += n;
        if let Some(pos) = resp_buf[..filled].windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let mut httparse_headers = [httparse::EMPTY_HEADER; 16];
    let mut response = httparse::Response::new(&mut httparse_headers);
    match response.parse(&resp_buf[..header_end]) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => {
            return Err(String::from("response headers unexpectedly incomplete"));
        }
        Err(e) => return Err(format!("response parse failed: {e:?}")),
    }
    let status = response.code.unwrap_or(0);

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut keep_alive = true; // HTTP/1.1 default, absent a `Connection` header saying otherwise.
    for h in response.headers.iter() {
        let Ok(value) = core::str::from_utf8(h.value) else {
            continue;
        };
        if h.name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().ok();
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        } else if h.name.eq_ignore_ascii_case("connection")
            && value.to_ascii_lowercase().contains("close")
        {
            keep_alive = false;
        }
    }

    // Small scratch space for discarding body bytes this loop has no use
    // for reading into `resp_buf` itself -- the point is just to advance
    // past them on the wire, not to keep them.
    let mut discard = [0u8; 128];

    if let Some(len) = content_length {
        let body_so_far = filled - header_end;
        let mut remaining = len.saturating_sub(body_so_far);
        while remaining > 0 {
            let to_read = remaining.min(discard.len());
            let n = session
                .read(&mut discard[..to_read])
                .await
                .map_err(|e| format!("body read failed: {e}"))?;
            if n == 0 {
                return Err(String::from("connection closed mid-body"));
            }
            remaining -= n;
        }
    } else if chunked {
        // Compacts whatever body bytes already arrived with the headers to
        // the front of `resp_buf`, then decodes in place: `carry_len` is
        // how many *unprocessed* bytes are sitting at `resp_buf[..carry_len]`
        // (chunk-size lines, chunk data, or trailers not yet consumed).
        // Capacity matches `resp_buf` exactly, so nothing already read off
        // the wire can be dropped the way a separately-sized buffer might.
        let mut carry_len = filled - header_end;
        resp_buf.copy_within(header_end..filled, 0);

        loop {
            let line_end = loop {
                if let Some(pos) = resp_buf[..carry_len].windows(2).position(|w| w == b"\r\n") {
                    break pos;
                }
                if carry_len >= resp_buf.len() {
                    return Err(String::from(
                        "chunk size line too long for the scratch buffer",
                    ));
                }
                let n = session
                    .read(&mut resp_buf[carry_len..])
                    .await
                    .map_err(|e| format!("chunk read failed: {e}"))?;
                if n == 0 {
                    return Err(String::from("connection closed mid-chunk-size"));
                }
                carry_len += n;
            };
            let size_field = core::str::from_utf8(&resp_buf[..line_end])
                .map_err(|_| String::from("chunk size line isn't valid UTF-8"))?;
            let size_field = size_field.split(';').next().unwrap_or(""); // drop chunk extensions, if any
            let size = usize::from_str_radix(size_field.trim(), 16)
                .map_err(|_| format!("bad chunk size {size_field:?}"))?;

            let after_line = line_end + 2; // the chunk-size line's own trailing CRLF
            resp_buf.copy_within(after_line..carry_len, 0);
            carry_len -= after_line;

            if size == 0 {
                // Final chunk: consume the trailer section (usually just
                // one more CRLF, but RFC 7230 allows trailer headers) up
                // to its terminating blank line, then this response is
                // fully drained.
                loop {
                    if resp_buf[..carry_len]
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .is_some()
                        || (carry_len >= 2 && &resp_buf[..2] == b"\r\n")
                    {
                        break;
                    }
                    if carry_len >= resp_buf.len() {
                        return Err(String::from(
                            "chunked trailer too long for the scratch buffer",
                        ));
                    }
                    let n = session
                        .read(&mut resp_buf[carry_len..])
                        .await
                        .map_err(|e| format!("trailer read failed: {e}"))?;
                    if n == 0 {
                        return Err(String::from("connection closed mid-trailer"));
                    }
                    carry_len += n;
                }
                break;
            }

            let mut remaining = size + 2; // chunk data plus its own trailing CRLF
            let take = remaining.min(carry_len);
            resp_buf.copy_within(take..carry_len, 0);
            carry_len -= take;
            remaining -= take;
            while remaining > 0 {
                let to_read = remaining.min(discard.len());
                let n = session
                    .read(&mut discard[..to_read])
                    .await
                    .map_err(|e| format!("chunk data read failed: {e}"))?;
                if n == 0 {
                    return Err(String::from("connection closed mid-chunk-data"));
                }
                remaining -= n;
            }
        }
    } else {
        // Neither `Content-Length` nor `Transfer-Encoding: chunked`: this
        // response's body (if any) is only delimited by the connection
        // closing, which this side can't safely wait for without breaking
        // its own `PERIOD` cadence. Not an error -- just not safe to keep
        // this connection open for another request.
        keep_alive = false;
    }

    Ok((status, keep_alive))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Duplex, block_on};

    fn response(bytes: &[u8]) -> Duplex {
        let d = Duplex::default();
        d.state.borrow_mut().input.extend(bytes.iter().copied());
        d
    }

    #[test]
    fn post_json_writes_the_exact_request_and_keeps_the_connection_open() {
        let mut d = Duplex::default();
        block_on(post_json(
            &mut d,
            "core.example",
            "/v1/heartbeat",
            "tok",
            "{\"a\":1}",
        ))
        .unwrap();
        let out = alloc::string::String::from_utf8(d.state.borrow().output.clone()).unwrap();
        assert_eq!(
            out,
            "POST /v1/heartbeat HTTP/1.1\r\nHost: core.example\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}"
        );
        assert!(
            !d.state.borrow().closed,
            "post_json must not close the connection"
        );
    }

    #[test]
    fn post_json_rejects_header_injection_and_relative_paths() {
        let mut d = Duplex::default();
        for (host, path, bearer) in [
            ("h\r\nX: y", "/p", "t"),
            ("h", "/p\n", "t"),
            ("h", "/p", "t\r\n"),
            ("h", "p", "t"),
        ] {
            assert!(block_on(post_json(&mut d, host, path, bearer, "{}")).is_err());
        }
        assert!(
            d.state.borrow().output.is_empty(),
            "nothing is written for a rejected request"
        );
    }

    #[test]
    fn drain_response_content_length_status_and_keep_alive() {
        let mut d = response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        let mut buf = [0u8; 256];
        assert_eq!(
            block_on(drain_response(&mut d, &mut buf)).unwrap(),
            (200, true)
        );
        assert!(
            d.state.borrow().input.is_empty(),
            "the whole body is consumed"
        );
    }

    #[test]
    fn drain_response_connection_close_and_missing_length_end_keep_alive() {
        let mut buf = [0u8; 256];
        let mut d =
            response(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert_eq!(
            block_on(drain_response(&mut d, &mut buf)).unwrap(),
            (204, false)
        );
        let mut d = response(b"HTTP/1.1 200 OK\r\n\r\n");
        assert_eq!(
            block_on(drain_response(&mut d, &mut buf)).unwrap(),
            (200, false)
        );
    }

    #[test]
    fn drain_response_chunked_body_with_extension_is_fully_consumed() {
        let mut d = response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n4;ext=1\r\ndefg\r\n0\r\n\r\n",
        );
        let mut buf = [0u8; 256];
        assert_eq!(
            block_on(drain_response(&mut d, &mut buf)).unwrap(),
            (200, true)
        );
        assert!(d.state.borrow().input.is_empty());
    }

    #[test]
    fn drain_response_reports_truncation_and_oversized_headers() {
        let mut buf = [0u8; 256];
        let mut d = response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc");
        assert!(block_on(drain_response(&mut d, &mut buf)).is_err());
        let mut d =
            response(b"HTTP/1.1 200 OK\r\nX-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n");
        let mut small = [0u8; 16];
        assert!(block_on(drain_response(&mut d, &mut small)).is_err());
        let mut d = response(b"garbage\r\n\r\n");
        assert!(block_on(drain_response(&mut d, &mut buf)).is_err());
    }
}
