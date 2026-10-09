#![no_std]

//! ESP TLS transport glue between `esp-hal`/`embassy-net` and `mbedtls-rs`.
//!
//! Owns transport mechanics only:
//!
//! - the single global `mbedtls_rs::Tls` instance and its hook installation;
//! - an Embassy DNS/TCP/TLS client dialer ([`embassy::EspTlsDialer`], the
//!   `iobewi_net_tls_core::TlsDialer` implementation);
//! - the TLS server listener ([`listener::EspTlsListener`], the
//!   `iobewi_net_tls_core::TlsListener` implementation) over `iobewi-esp-tcp`;
//! - a connected TLS session as a `net/io` connection
//!   ([`embassy::TlsStream`]).
//!
//! Crypto (PEM/X.509, identity generation) is `iobewi-crypto-mbedtls`;
//! certificate persistence and trust policy are `iobewi-tls-service`; the
//! HTTP surface is the HTTP server. This crate knows none of them.

extern crate alloc;

pub use iobewi_crypto_mbedtls::mbedtls_rs;
pub use iobewi_crypto_mbedtls::UnixTimeFn;

use mbedtls_rs::TlsReference;
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
use mbedtls_rs::Tls;
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
use static_cell::StaticCell;

#[cfg(feature = "embassy-net")]
pub mod embassy;
#[cfg(feature = "embassy-net")]
pub mod listener;
#[cfg(feature = "embassy-net")]
pub use listener::EspTlsListener;

/// Named alias convenient for long-lived application structs.
pub type TlsReferenceStatic = TlsReference<'static>;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
static RNG: StaticCell<iobewi_esp_entropy::EspCryptoRng> = StaticCell::new();
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
static TLS: StaticCell<Tls<'static>> = StaticCell::new();

/// Initializes the process-global MbedTLS instance.
///
/// This must be called exactly once, before any MbedTLS session is created.
/// `now` may return `None` until the application has synchronized its wall
/// clock; X.509 validity checks then fail closed.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub fn init(now: UnixTimeFn) -> TlsReferenceStatic {
    iobewi_crypto_mbedtls::install_hooks(now);
    let rng = RNG.init(iobewi_esp_entropy::EspCryptoRng::new());
    let tls = TLS.init(Tls::new(rng).expect("iobewi-esp-tls::init() called more than once"));
    tls.reference()
}
