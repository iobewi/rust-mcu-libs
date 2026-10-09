#![no_std]

//! Best-effort WebSocket streaming of the lines captured by `iobewi-log`,
//! over a platform supplied secure transport. Failed connections discard
//! stale logs; logging never waits on network I/O.

extern crate alloc;

use alloc::{format, string::String};
use core::fmt::{Debug, Display};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{ErrorType, Read, Write};
use iobewi_entropy::EntropySource;
use iobewi_log::{RING_CAPACITY, discard, pop_record};
use iobewi_net_tls_core::SecureClientTransport;
use log::{info, warn};
use serde::Serialize;

const DRAIN_PERIOD: Duration = Duration::from_millis(200);
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const BASE_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const STABLE_THRESHOLD: Duration = Duration::from_secs(30);

/// Application metadata attached to streamed log records.
#[allow(async_fn_in_trait)]
pub trait LogMetadata {
    async fn node_id(&self) -> String;
    fn workload(&self) -> &'static str;
    fn timestamp(&self) -> u64;
}

/// Where and how to stream logs to the Core. The service never knows the
/// application's persistence schema or endpoint version.
#[allow(async_fn_in_trait)]
pub trait StreamConfig {
    async fn ctrl_url(&self) -> String;
    async fn token(&self) -> String;
    fn path(&self) -> String;
}

/// One random 32-bit value (frame masking key, backoff jitter) drawn from the
/// platform's [`EntropySource`]. The WebSocket layer's nonce and RFC 6455
/// client-frame masking need the same unpredictable bytes TLS does, so it asks
/// for the shared capability rather than a private one.
fn random_u32<E: EntropySource>(entropy: &E) -> u32 {
    let mut bytes = [0u8; 4];
    entropy.fill_random(&mut bytes);
    u32::from_le_bytes(bytes)
}

#[derive(Serialize)]
struct LogFrame<'a> {
    ts: u64,
    node: &'a str,
    workload: &'static str,
    level: &'static str,
    target: &'a str,
    msg: &'a str,
}

fn level_name(level: log::Level) -> &'static str {
    match level {
        log::Level::Error => "error",
        log::Level::Warn => "warn",
        log::Level::Info => "info",
        log::Level::Debug => "debug",
        log::Level::Trace => "trace",
    }
}

fn split_host_port(ctrl_url: &str) -> Option<(&str, u16)> {
    let rest = ctrl_url
        .split_once("://")
        .map(|(_, value)| value)
        .unwrap_or(ctrl_url);
    let host_port = rest.split('/').next().unwrap_or(rest);
    match host_port.split_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host, port.parse().unwrap_or(443))),
        _ if !host_port.is_empty() => Some((host_port, 443)),
        _ => None,
    }
}

fn websocket_authority(host: &str, port: u16) -> String {
    if port == 443 {
        String::from(host)
    } else {
        format!("{host}:{port}")
    }
}

fn next_backoff(current: Duration) -> Duration {
    Duration::from_secs((current.as_secs() * 2).min(MAX_BACKOFF.as_secs()))
}

fn jittered(base: Duration, random: u32) -> Duration {
    let base_ms = base.as_millis() as i64;
    let percent = (random % 61) as i64 - 30;
    Duration::from_millis((base_ms + base_ms * percent / 100).max(1000) as u64)
}

#[allow(clippy::too_many_arguments)] // Transport buffers and handshake inputs are independently owned.
async fn connect_and_upgrade<'a, T: SecureClientTransport, E: EntropySource>(
    transport: &'a T,
    entropy: &E,
    rx: &'a mut [u8],
    tx: &'a mut [u8],
    host: &'a str,
    port: u16,
    path: &str,
    token: &str,
) -> Result<T::Connection<'a>, String> {
    let mut session = transport
        .connect(host, port, rx, tx)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    let mut nonce = [0u8; iobewi_http_client::websocket::NONCE_LENGTH];
    entropy.fill_random(&mut nonce);
    let authority = websocket_authority(host, port);
    iobewi_http_client::websocket::upgrade(&mut session, &authority, path, token, &nonce).await?;
    info!("logs: connected to {host}:{port}");
    Ok(session)
}

async fn pump_session<
    S: ErrorType + Read + Write,
    C: StreamConfig + LogMetadata,
    E: EntropySource,
>(
    session: &mut S,
    config: &C,
    entropy: &E,
    token_snapshot: &str,
) -> String
where
    S::Error: Display + Debug,
{
    loop {
        if config.token().await != token_snapshot {
            return String::from("bearer token changed, reconnecting");
        }
        let mut first = [0u8; 1];
        match with_timeout(DRAIN_PERIOD, session.read(&mut first)).await {
            Err(_) => {} // Idle; no WebSocket frame byte was consumed.
            Ok(Ok(0)) => return String::from("server closed the connection"),
            Ok(Err(error)) => return format!("frame read failed: {error}"),
            Ok(Ok(_)) => match with_timeout(
                FRAME_TIMEOUT,
                iobewi_http_client::websocket::process_frame_after_first(
                    &mut *session,
                    first[0],
                    random_u32(entropy),
                ),
            )
            .await
            {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => return String::from("server closed the connection"),
                Ok(Err(error)) => return error,
                Err(_) => return String::from("incomplete WebSocket frame timed out"),
            },
        }
        for _ in 0..RING_CAPACITY {
            let Some(record) = pop_record() else {
                break;
            };
            let node_id = config.node_id().await;
            let frame = LogFrame {
                ts: config.timestamp(),
                node: &node_id,
                workload: config.workload(),
                level: level_name(record.level),
                target: &record.target,
                msg: &record.message,
            };
            let Ok(json) = serde_json::to_vec(&frame) else {
                continue;
            };
            if let Err(error) =
                iobewi_http_client::websocket::send_text(&mut *session, &json, random_u32(entropy))
                    .await
            {
                return error;
            }
        }
    }
}

/// Reconnect with capped jittered backoff. Fresh logs are sent only while a
/// connection is active; the ring is cleared on every failed session.
pub async fn run<C: StreamConfig + LogMetadata, T: SecureClientTransport, E: EntropySource>(
    config: &C,
    transport: &T,
    entropy: &E,
) -> ! {
    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 512];
    let mut backoff = BASE_BACKOFF;
    loop {
        let ctrl_url = config.ctrl_url().await;
        let Some((host, port)) = split_host_port(&ctrl_url) else {
            discard();
            Timer::after(BASE_BACKOFF).await;
            continue;
        };
        let token = config.token().await;
        if token.is_empty() {
            discard();
            Timer::after(BASE_BACKOFF).await;
            continue;
        }
        let path = config.path();
        let connected = with_timeout(
            HANDSHAKE_TIMEOUT,
            connect_and_upgrade(
                transport, entropy, &mut rx, &mut tx, host, port, &path, &token,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(String::from("WebSocket handshake timed out")));
        let stable = match connected {
            Ok(mut session) => {
                let connected_at = Instant::now();
                let error = pump_session(&mut session, config, entropy, &token).await;
                warn!("logs: session ended: {error}");
                connected_at.elapsed() >= STABLE_THRESHOLD
            }
            Err(error) => {
                warn!("logs: session ended: {error}");
                false
            }
        };
        discard();
        if stable {
            backoff = BASE_BACKOFF;
        }
        Timer::after(jittered(backoff, random_u32(entropy))).await;
        backoff = next_backoff(backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_preserves_levels_target_and_escaping() {
        for (level, expected) in [
            (log::Level::Error, "error"),
            (log::Level::Warn, "warn"),
            (log::Level::Info, "info"),
            (log::Level::Debug, "debug"),
            (log::Level::Trace, "trace"),
        ] {
            let frame = LogFrame {
                ts: 1,
                node: "node",
                workload: "app",
                level: level_name(level),
                target: "app::usb",
                msg: "quoted \"text\"\n",
            };
            let json = serde_json::to_vec(&frame).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
            assert_eq!(value["level"], expected);
            assert_eq!(value["target"], "app::usb");
            assert_eq!(value["msg"], "quoted \"text\"\n");
        }
    }

    #[test]
    fn reconnect_policy_and_url_parsing() {
        assert_eq!(
            split_host_port("https://core.example:8443/foo"),
            Some(("core.example", 8443))
        );
        assert_eq!(split_host_port("core.example"), Some(("core.example", 443)));
        assert_eq!(split_host_port(""), None);
        assert_eq!(
            websocket_authority("core.example", 8443),
            "core.example:8443"
        );
        assert_eq!(websocket_authority("core.example", 443), "core.example");
        assert_eq!(
            next_backoff(Duration::from_secs(40)),
            Duration::from_secs(60)
        );
        assert_eq!(
            jittered(Duration::from_secs(5), 0),
            Duration::from_millis(3500)
        );
    }
}

#[cfg(test)]
mod entropy_tests {
    use super::*;
    use core::cell::Cell;

    struct Counter(Cell<u8>);

    impl EntropySource for Counter {
        fn fill_random(&self, output: &mut [u8]) {
            for byte in output.iter_mut() {
                self.0.set(self.0.get().wrapping_add(1));
                *byte = self.0.get();
            }
        }
    }

    #[test]
    fn masking_keys_and_jitter_come_from_the_shared_entropy_source() {
        let source = Counter(Cell::new(0));
        assert_eq!(random_u32(&source), u32::from_le_bytes([1, 2, 3, 4]));
        assert_eq!(random_u32(&source), u32::from_le_bytes([5, 6, 7, 8]));
    }
}
