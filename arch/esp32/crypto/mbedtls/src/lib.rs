#![no_std]

//! MbedTLS implementation of the portable TLS-material crypto contract
//! (`iobewi_crypto_core::TlsCrypto`): PEM/X.509 parsing and validation,
//! P-256 identity generation, server session configuration, and the
//! MbedTLS wall-clock/timer hooks.
//!
//! Owns no sockets, persistence or HTTP. Randomness is injected as an
//! `iobewi_entropy::EntropySource`; the clock as a plain function.
//!
//! mbedtls-rs-sys does not build for the x86_64 host (its bindgen setup
//! asserts a 32-bit pointer width), so this crate lives in the provisional
//! ESP workspace and is exercised by cross-builds, not host tests.

extern crate alloc;

pub use mbedtls_rs;

use alloc::boxed::Box;
use alloc::ffi::CString;

use iobewi_crypto_core::{Identity, PairError, TlsCrypto};
use iobewi_entropy::EntropySource;
use mbedtls_rs::sys::hook::timer::{hook_timer, MbedtlsTimer};
use mbedtls_rs::sys::hook::wall_clock::{hook_wall_clock, MbedtlsWallClock};
use mbedtls_rs::sys::{mbedtls_ms_time_t, tm};
use mbedtls_rs::{Certificate, Credentials, PrivateKey, ServerSessionConfig, SessionConfig, X509};
use static_cell::StaticCell;

/// Function used by MbedTLS to obtain Unix epoch seconds.
///
/// Returning `None` makes certificate-date validation fail closed.
pub type UnixTimeFn = fn() -> Option<u64>;

/// Unix epoch seconds -> MbedTLS broken-down UTC time. The calendar math is
/// `iobewi_time::epoch_to_utc_tm`; this only maps it onto MbedTLS' `tm`.
pub fn epoch_to_tm(epoch: u64) -> Option<tm> {
    let t = iobewi_time::epoch_to_utc_tm(epoch)?;
    Some(tm {
        tm_sec: t.sec,
        tm_min: t.min,
        tm_hour: t.hour,
        tm_mday: t.mday,
        tm_mon: t.mon,
        tm_year: t.year,
        tm_wday: t.wday,
        tm_yday: t.yday,
        tm_isdst: 0,
    })
}

struct WallClock {
    now: UnixTimeFn,
}

impl MbedtlsWallClock for WallClock {
    fn instant(&self) -> Option<tm> {
        epoch_to_tm((self.now)()?)
    }
}

struct UptimeTimer;

impl MbedtlsTimer for UptimeTimer {
    fn now(&self) -> mbedtls_ms_time_t {
        embassy_time::Instant::now().as_millis() as mbedtls_ms_time_t
    }
}

static TIMER: UptimeTimer = UptimeTimer;
static WALL_CLOCK: StaticCell<WallClock> = StaticCell::new();

/// Installs the MbedTLS monotonic and wall-clock hooks. Call exactly once,
/// before any MbedTLS X.509/session use. `now` may return `None` until the
/// application has synchronized its wall clock; X.509 validity checks then
/// fail closed.
pub fn install_hooks(now: UnixTimeFn) {
    let wall_clock = WALL_CLOCK.init(WallClock { now });
    // SAFETY: both hooks receive process-lifetime statics and are installed
    // before the first MbedTLS X.509/session use.
    unsafe {
        hook_timer(Some(&TIMER));
        hook_wall_clock(Some(wall_clock));
    }
}

/// MbedTLS `f_rng` callback; `ctx` points at a `&dyn EntropySource`.
unsafe extern "C" fn mbedtls_rng(
    ctx: *mut core::ffi::c_void,
    out: *mut u8,
    len: usize,
) -> core::ffi::c_int {
    // SAFETY: every caller in this crate passes a pointer to a live
    // `&dyn EntropySource`, and MbedTLS provides a writable buffer of `len`.
    let rng = unsafe { *(ctx as *const &dyn EntropySource) };
    rng.fill_random(unsafe { core::slice::from_raw_parts_mut(out, len) });
    0
}

/// Parses the leaf certificate and private key and verifies that they form a
/// pair. No persistence is performed.
pub fn validate_cert_key_pair(rng: &dyn EntropySource, cert_pem: &str, key_pem: &str) -> Result<(), PairError> {
    use mbedtls_rs::sys::{
        mbedtls_pk_check_pair, mbedtls_pk_context, mbedtls_pk_free, mbedtls_pk_init, mbedtls_pk_parse_key,
        mbedtls_x509_crt, mbedtls_x509_crt_free, mbedtls_x509_crt_init, mbedtls_x509_crt_parse,
    };

    struct Contexts {
        crt: Box<mbedtls_x509_crt>,
        pk: Box<mbedtls_pk_context>,
    }

    impl Drop for Contexts {
        fn drop(&mut self) {
            // SAFETY: both contexts were initialized below and are freed once.
            unsafe {
                mbedtls_x509_crt_free(&mut *self.crt);
                mbedtls_pk_free(&mut *self.pk);
            }
        }
    }

    let rng_ref: &dyn EntropySource = rng;
    let rng_ctx = (&rng_ref as *const &dyn EntropySource).cast_mut().cast::<core::ffi::c_void>();
    let cert_c = CString::new(cert_pem).map_err(|_| PairError::Invalid)?;
    let key_c = CString::new(key_pem).map_err(|_| PairError::Invalid)?;
    let mut ctx = Contexts { crt: Box::default(), pk: Box::default() };

    // SAFETY: fresh contexts and NUL-terminated PEM buffers valid for the
    // duration of each MbedTLS call.
    unsafe {
        mbedtls_x509_crt_init(&mut *ctx.crt);
        mbedtls_pk_init(&mut *ctx.pk);

        if mbedtls_x509_crt_parse(&mut *ctx.crt, cert_c.as_ptr().cast(), cert_c.count_bytes() + 1) != 0 {
            return Err(PairError::Invalid);
        }

        if mbedtls_pk_parse_key(
            &mut *ctx.pk,
            key_c.as_ptr().cast(),
            key_c.count_bytes() + 1,
            core::ptr::null(),
            0,
            Some(mbedtls_rng),
            rng_ctx,
        ) != 0
        {
            return Err(PairError::Invalid);
        }

        if mbedtls_pk_check_pair(&ctx.crt.pk, &*ctx.pk, Some(mbedtls_rng), rng_ctx) != 0 {
            return Err(PairError::Mismatch);
        }
    }

    Ok(())
}


#[derive(Debug)]
pub enum IdentityGenerationError {
    InvalidName,
    PsaInit(i32),
    KeyGeneration(i32),
    KeySetup(i32),
    KeyEncoding(i32),
    CertificateSetup(i32),
    CertificateEncoding(i32),
    InvalidUtf8,
}

/// Fresh P-256 server identity generated locally.
///
/// The key is created as a volatile PSA key, wrapped by MbedTLS only for
/// certificate construction/export, then destroyed after its PEM form has
/// been produced. This crate deliberately does not persist it: persistence,
/// lifecycle and enrollment are application policy.
///
/// The self-signed certificate uses a deliberately wide fixed validity
/// window because bootstrap identity creation can happen before networking
/// and therefore before the application has synchronized a wall clock.
pub struct GeneratedIdentity {
    pub cert_pem: alloc::string::String,
    pub key_pem: alloc::string::String,
}

pub fn generate_self_signed_identity(
    rng: &dyn EntropySource,
    common_name: &str,
) -> Result<GeneratedIdentity, IdentityGenerationError> {
    use mbedtls_rs::sys::{
        mbedtls_md_type_t_MBEDTLS_MD_SHA256, mbedtls_pk_context, mbedtls_pk_copy_from_psa,
        mbedtls_pk_free, mbedtls_pk_init, mbedtls_pk_write_key_pem,
        mbedtls_x509write_cert, mbedtls_x509write_crt_free,
        mbedtls_x509write_crt_init, mbedtls_x509write_crt_pem,
        mbedtls_x509write_crt_set_basic_constraints,
        mbedtls_x509write_crt_set_issuer_key, mbedtls_x509write_crt_set_issuer_name,
        mbedtls_x509write_crt_set_md_alg, mbedtls_x509write_crt_set_serial_raw,
        mbedtls_x509write_crt_set_subject_key, mbedtls_x509write_crt_set_subject_name,
        mbedtls_x509write_crt_set_validity, psa_crypto_init, psa_destroy_key,
        psa_generate_key, psa_key_attributes_t,
    };

    // PSA encodings from the PSA Crypto specification. They are macros in C,
    // so bindgen does not expose constructors for them.
    const PSA_SUCCESS: i32 = 0;
    const PSA_ECC_FAMILY_SECP_R1: u16 = 0x12;
    const PSA_KEY_TYPE_ECC_KEY_PAIR_BASE: u16 = 0x7100;
    const PSA_KEY_USAGE_EXPORT: u32 = 0x0000_0001;
    const KEY_TYPE: u16 = PSA_KEY_TYPE_ECC_KEY_PAIR_BASE | PSA_ECC_FAMILY_SECP_R1;

    struct Contexts {
        pk: Box<mbedtls_pk_context>,
        crt: Box<mbedtls_x509write_cert>,
        key_id: u32,
    }

    impl Drop for Contexts {
        fn drop(&mut self) {
            // SAFETY: contexts were initialized below; an all-zero key id is
            // never a valid PSA key and is ignored.
            unsafe {
                mbedtls_x509write_crt_free(&mut *self.crt);
                mbedtls_pk_free(&mut *self.pk);
                if self.key_id != 0 {
                    let _ = psa_destroy_key(self.key_id);
                }
            }
        }
    }

    let rng_ref: &dyn EntropySource = rng;
    let rng_ctx = (&rng_ref as *const &dyn EntropySource).cast_mut().cast::<core::ffi::c_void>();
    let subject = alloc::format!("CN={common_name}");
    let subject = CString::new(subject).map_err(|_| IdentityGenerationError::InvalidName)?;
    let mut ctx = Contexts {
        pk: Box::default(),
        crt: Box::default(),
        key_id: 0,
    };

    // SAFETY: all calls receive initialized contexts and valid buffers.
    unsafe {
        let rc = psa_crypto_init();
        if rc != PSA_SUCCESS {
            return Err(IdentityGenerationError::PsaInit(rc));
        }

        // PSA attributes come from bindgen. Initialize the fields we own
        // directly and leave every other generated field at its zero default.
        let mut attributes = psa_key_attributes_t {
            private_type: KEY_TYPE,
            private_bits: 256,
            private_lifetime: 0, // PSA_KEY_LIFETIME_VOLATILE
            ..Default::default()
        };
        // Export is the only policy needed here: the PSA key is copied into
        // a normal MbedTLS PK context immediately below, after which signing
        // the self-signed certificate no longer depends on the PSA policy.
        attributes.private_policy.private_usage = PSA_KEY_USAGE_EXPORT;
        attributes.private_policy.private_alg = 0;
        attributes.private_policy.private_alg2 = 0;
        attributes.private_id = 0;

        let rc = psa_generate_key(&attributes, &mut ctx.key_id);
        if rc != PSA_SUCCESS || ctx.key_id == 0 {
            return Err(IdentityGenerationError::KeyGeneration(rc));
        }

        mbedtls_pk_init(&mut *ctx.pk);
        let rc = mbedtls_pk_copy_from_psa(ctx.key_id, &mut *ctx.pk);
        if rc != 0 {
            return Err(IdentityGenerationError::KeySetup(rc));
        }

        mbedtls_x509write_crt_init(&mut *ctx.crt);
        let setup = |rc: i32| {
            if rc == 0 {
                Ok(())
            } else {
                Err(IdentityGenerationError::CertificateSetup(rc))
            }
        };
        setup(mbedtls_x509write_crt_set_subject_name(
            &mut *ctx.crt,
            subject.as_ptr(),
        ))?;
        setup(mbedtls_x509write_crt_set_issuer_name(
            &mut *ctx.crt,
            subject.as_ptr(),
        ))?;
        mbedtls_x509write_crt_set_subject_key(&mut *ctx.crt, &mut *ctx.pk);
        mbedtls_x509write_crt_set_issuer_key(&mut *ctx.crt, &mut *ctx.pk);

        let mut serial = [0u8; 16];
        rng.fill_random(&mut serial);
        serial[0] &= 0x7f;
        serial[0] |= 0x01;
        setup(mbedtls_x509write_crt_set_serial_raw(
            &mut *ctx.crt,
            serial.as_mut_ptr(),
            serial.len(),
        ))?;

        setup(mbedtls_x509write_crt_set_validity(
            &mut *ctx.crt,
            c"20260101000000".as_ptr(),
            c"20991231235959".as_ptr(),
        ))?;
        mbedtls_x509write_crt_set_md_alg(
            &mut *ctx.crt,
            mbedtls_md_type_t_MBEDTLS_MD_SHA256,
        );
        setup(mbedtls_x509write_crt_set_basic_constraints(
            &mut *ctx.crt,
            0,
            -1,
        ))?;

        let mut key_buf = [0u8; 1024];
        let rc = mbedtls_pk_write_key_pem(
            &*ctx.pk,
            key_buf.as_mut_ptr(),
            key_buf.len(),
        );
        if rc != 0 {
            return Err(IdentityGenerationError::KeyEncoding(rc));
        }

        let mut cert_buf = [0u8; 2048];
        let rc = mbedtls_x509write_crt_pem(
            &mut *ctx.crt,
            cert_buf.as_mut_ptr(),
            cert_buf.len(),
            Some(mbedtls_rng),
            rng_ctx,
        );
        if rc != 0 {
            return Err(IdentityGenerationError::CertificateEncoding(rc));
        }

        fn pem_string(
            buf: &[u8],
        ) -> Result<alloc::string::String, IdentityGenerationError> {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let text = core::str::from_utf8(&buf[..end])
                .map_err(|_| IdentityGenerationError::InvalidUtf8)?;
            Ok(alloc::string::String::from(text))
        }

        Ok(GeneratedIdentity {
            cert_pem: pem_string(&cert_buf)?,
            key_pem: pem_string(&key_buf)?,
        })
    }
}

/// Failure while constructing a server-side TLS configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerConfigError {
    InvalidCertificatePem,
    InvalidPrivateKeyPem,
    InvalidCertificate,
    InvalidPrivateKey,
}

/// Builds a server-side MbedTLS session configuration from a PEM pair.
pub fn server_config_from_pem(
    cert_pem: &str,
    key_pem: &str,
) -> Result<SessionConfig<'static>, ServerConfigError> {
    let cert_c =
        CString::new(cert_pem).map_err(|_| ServerConfigError::InvalidCertificatePem)?;
    let key_c =
        CString::new(key_pem).map_err(|_| ServerConfigError::InvalidPrivateKeyPem)?;
    let certificate = Certificate::new(X509::PEM(&cert_c))
        .map_err(|_| ServerConfigError::InvalidCertificate)?;
    let private_key = PrivateKey::new(X509::PEM(&key_c), None)
        .map_err(|_| ServerConfigError::InvalidPrivateKey)?;
    Ok(SessionConfig::Server(ServerSessionConfig::new(Credentials {
        certificate,
        private_key,
    })))
}

/// Verifies that a CA PEM parses as an X.509 certificate.
pub fn validate_ca_pem(ca_pem: &str) -> bool {
    let Ok(ca_c) = CString::new(ca_pem) else {
        return false;
    };
    Certificate::new(X509::PEM(&ca_c)).is_ok()
}

/// MbedTLS-backed [`TlsCrypto`], generic over the entropy source.
pub struct MbedtlsCrypto<R> {
    rng: R,
}

impl<R: EntropySource> MbedtlsCrypto<R> {
    pub const fn new(rng: R) -> Self {
        Self { rng }
    }
}

impl<R: EntropySource> TlsCrypto for MbedtlsCrypto<R> {
    type ServerConfig = SessionConfig<'static>;

    fn validate_pair(&self, cert: &str, key: &str) -> Result<(), PairError> {
        validate_cert_key_pair(&self.rng, cert, key)
    }

    fn server_config(&self, cert: &str, key: &str) -> Option<Self::ServerConfig> {
        server_config_from_pem(cert, key).ok()
    }

    fn generate_identity(&self, common_name: &str) -> Option<Identity> {
        let generated = generate_self_signed_identity(&self.rng, common_name)
            .map_err(|e| log::warn!("tls: identity generation failed: {e:?}"))
            .ok()?;
        Some(Identity { cert_pem: generated.cert_pem, key_pem: generated.key_pem })
    }

    fn validate_ca(&self, ca: &str) -> bool {
        validate_ca_pem(ca)
    }
}
