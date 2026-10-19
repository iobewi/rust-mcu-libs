# iobewi-http-client

## Summary

Outbound HTTP/1.1 and optional WebSocket primitives over connected async I/O.

## Responsibilities

Serialize authenticated JSON POST requests, consume one HTTP response body and report safe connection reuse; implement the optional outbound WebSocket upgrade/frame helpers.

## Non-responsibilities

DNS, dialing, TLS, certificates, server routes, request cadence and retry policy.

## Architecture

Portable protocol layer over embedded_io_async Read/Write. TLS service and log streaming inject established connections.

## Public API

`post_json(session, host, path, bearer, json)` writes and flushes without closing. `get(session, host, path, headers)` writes a `GET` (`Connection: close`); `read_head(session, buf)` returns the status, `Content-Length`, chunked flag and `Location` and leaves the body to the caller (a stream). `Target::parse` / `Target::redirect` parse `http(s)` URLs and apply the redirect policy: `https` to `http` is refused, and a followed redirect is a new target the caller connects to afresh, so its host is verified on its own. `drain_response(session, scratch)` returns `(status, reusable)`. Feature `websocket` exports upgrade, frame processing and text-send helpers.

## Validation

`cargo test -p iobewi-http-client --features websocket` covers request bytes, framing, truncation and WebSocket behaviour.

## Known limitations

Uses alloc. `read_head` does not decode a chunked body (it reports the flag); relative `Location` paths other than absolute ones (`/x`) are refused; IPv6 literals are refused. Requests reject CR/LF in host/path/bearer and require an absolute path. Response headers must fit scratch and at most 16 parsed headers. Content-Length and chunked bodies are discarded incrementally. Requests/responses are lock-step: pipelined bytes can be discarded. Without body framing, reuse is false. Caller owns deadlines and connection closure; this is not a general browser HTTP client.

## Related components

Future log streaming and platform TLS connections.
