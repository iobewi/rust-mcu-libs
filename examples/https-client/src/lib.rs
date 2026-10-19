#![cfg_attr(not(test), no_std)]

//! The verdict logic of the HTTPS client qualification, free of any hardware so
//! that the host tests can pin it down.
//!
//! A refusal test only means something if the refusal is the *certificate
//! check*. A connection that fails because the DNS answered nothing, because
//! the stack lacks RSA or because the peer reset would otherwise pass as "the
//! bad certificate was refused". [`judge`] therefore accepts only the MbedTLS
//! certificate-verification error for a refusal test.

/// `MBEDTLS_ERR_X509_CERT_VERIFY_FAILED`: the handshake was aborted by the
/// certificate verification (unknown issuer, name mismatch, expired or not yet
/// valid: the stack does not say which).
pub const X509_CERT_VERIFY_FAILED: i32 = -0x2700;

/// What the dialer reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Refused before any network activity: the clock is not set.
    ClockUnsynced,
    /// Refused before any network activity: no trust anchor.
    NoCa,
    /// The trust anchors failed to parse.
    BadCa,
    Dns,
    Tcp,
    /// The certificate verification failed.
    CertificateRejected,
    /// Any other handshake failure, with the MbedTLS code (0: not an MbedTLS error).
    Handshake(i32),
    /// The server answered, but not with what was asked for.
    Http(u16),
    /// Nothing answered within the limit.
    Timeout,
    /// The connection broke after the handshake.
    Io,
}

impl Failure {
    /// Classify an MbedTLS handshake error code.
    pub fn from_handshake_code(code: i32) -> Self {
        if code == X509_CERT_VERIFY_FAILED {
            Self::CertificateRejected
        } else {
            Self::Handshake(code)
        }
    }

    /// Short name for the log.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClockUnsynced => "clock-unsynced",
            Self::NoCa => "no-ca",
            Self::BadCa => "bad-ca",
            Self::Dns => "dns",
            Self::Tcp => "tcp",
            Self::CertificateRejected => "certificate-rejected",
            Self::Handshake(_) => "handshake-other",
            Self::Http(_) => "http-status",
            Self::Timeout => "timeout",
            Self::Io => "io",
        }
    }
}

/// What a scenario must end with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Connected, and the server answered with this status.
    Status(u16),
    /// Refused by the certificate check, and by nothing else.
    CertificateRejected,
    /// Refused before any network activity because the clock is not set.
    ClockUnsynced,
}

/// Whether `observed` is what `expect` demands.
pub fn judge(expect: Expect, observed: &Result<u16, Failure>) -> bool {
    match (expect, observed) {
        (Expect::Status(want), Ok(got)) => want == *got,
        (Expect::CertificateRejected, Err(Failure::CertificateRejected)) => true,
        (Expect::ClockUnsynced, Err(Failure::ClockUnsynced)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_that_succeeds_never_passes_a_refusal_test() {
        assert!(!judge(Expect::CertificateRejected, &Ok(200)));
        assert!(!judge(Expect::ClockUnsynced, &Ok(200)));
    }

    #[test]
    fn only_the_certificate_check_passes_a_certificate_refusal() {
        assert!(judge(
            Expect::CertificateRejected,
            &Err(Failure::CertificateRejected)
        ));
        for other in [
            Failure::ClockUnsynced,
            Failure::NoCa,
            Failure::BadCa,
            Failure::Dns,
            Failure::Tcp,
            Failure::Handshake(-0x7780),
            Failure::Http(404),
            Failure::Timeout,
            Failure::Io,
        ] {
            assert!(
                !judge(Expect::CertificateRejected, &Err(other)),
                "{other:?}"
            );
        }
    }

    #[test]
    fn only_the_clock_guard_passes_a_clock_refusal() {
        assert!(judge(Expect::ClockUnsynced, &Err(Failure::ClockUnsynced)));
        assert!(!judge(
            Expect::ClockUnsynced,
            &Err(Failure::CertificateRejected)
        ));
    }

    #[test]
    fn a_status_test_needs_that_status() {
        assert!(judge(Expect::Status(200), &Ok(200)));
        assert!(!judge(Expect::Status(200), &Ok(301)));
        assert!(!judge(
            Expect::Status(200),
            &Err(Failure::CertificateRejected)
        ));
    }

    #[test]
    fn handshake_codes_are_classified() {
        assert_eq!(
            Failure::from_handshake_code(-0x2700),
            Failure::CertificateRejected
        );
        assert_eq!(
            Failure::from_handshake_code(-0x7780),
            Failure::Handshake(-0x7780)
        );
    }
}
