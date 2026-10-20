#![no_std]

//! A small set of public root CA certificates, as PEM text in flash.
//!
//! The intent is to be the trust anchors handed to a certificate-verifying TLS
//! dialer (`iobewi_net_tls_core::TlsDialer`, whose `ca_pem` argument accepts
//! several concatenated PEM certificates) when an outbound client talks to
//! public servers such as web radios. It holds data only: no parsing, no
//! policy, no platform type. Nothing here is a secret.
//!
//! This is deliberately **not** a copy of the Mozilla bundle (about 150 roots).
//! Each root costs flash and, on every connection, parse time and heap; each
//! one is a trust decision. The set is short and each entry records why it is
//! there. Adding or removing a root is a code change reviewed as such.
//!
//! Roots, not intermediates: public CAs rotate their intermediates often, the
//! roots change on a scale of decades. A server sends its intermediates in the
//! handshake.

/// One trust anchor.
#[derive(Debug, Clone, Copy)]
pub struct Root {
    /// Subject common name.
    pub name: &'static str,
    /// Why it is in the set.
    pub reason: &'static str,
    /// The certificate, PEM.
    pub pem: &'static str,
    /// SHA-256 of the DER certificate, as published by the CA.
    pub sha256: [u8; 32],
}

macro_rules! hex32 {
    ($($b:literal)*) => { [$($b),*] };
}

/// ISRG Root X1 (RSA 4096): Let's Encrypt, the issuer of most small stations
/// and of the Radio France streams.
pub const ISRG_ROOT_X1: Root = Root {
    name: "ISRG Root X1",
    reason: "Let's Encrypt RSA chains (Radio France, Revma and many others)",
    pem: include_str!("../pem/ISRG_Root_X1.pem"),
    sha256: hex32!(
        0x96 0xBC 0xEC 0x06 0x26 0x49 0x76 0xF3 0x74 0x60 0x77 0x9A 0xCF 0x28 0xC5 0xA7
        0xCF 0xE8 0xA3 0xC0 0xAA 0xE1 0x1A 0x8F 0xFC 0xEE 0x05 0xC0 0xBD 0xDF 0x08 0xC6
    ),
};

/// ISRG Root X2 (ECDSA P-384): Let's Encrypt ECDSA chains. Needs the `p384`
/// feature of `iobewi-crypto-mbedtls`.
pub const ISRG_ROOT_X2: Root = Root {
    name: "ISRG Root X2",
    reason: "Let's Encrypt ECDSA chains",
    pem: include_str!("../pem/ISRG_Root_X2.pem"),
    sha256: hex32!(
        0x69 0x72 0x9B 0x8E 0x15 0xA8 0x6E 0xFC 0x17 0x7A 0x57 0xAF 0xB7 0x17 0x1D 0xFC
        0x64 0xAD 0xD2 0x8C 0x2F 0xCA 0x8C 0xF1 0x50 0x7E 0x34 0x45 0x3C 0xCB 0x14 0x70
    ),
};

/// DigiCert Global Root G2 (RSA 2048): DigiCert and the CDNs and hosters that
/// use it.
pub const DIGICERT_GLOBAL_ROOT_G2: Root = Root {
    name: "DigiCert Global Root G2",
    reason: "DigiCert chains (many hosters and CDNs)",
    pem: include_str!("../pem/DigiCert_Global_Root_G2.pem"),
    sha256: hex32!(
        0xCB 0x3C 0xCB 0xB7 0x60 0x31 0xE5 0xE0 0x13 0x8F 0x8D 0xD3 0x9A 0x23 0xF9 0xDE
        0x47 0xFF 0xC3 0x5E 0x43 0xC1 0x14 0x4C 0xEA 0x27 0xD4 0x6A 0x5A 0xB1 0xCB 0x5F
    ),
};

/// GTS Root R1 (RSA 4096): Google Trust Services, used by Google and by
/// Cloudflare-fronted hosts.
pub const GTS_ROOT_R1: Root = Root {
    name: "GTS Root R1",
    reason: "Google Trust Services chains (Google, Cloudflare-fronted hosts)",
    pem: include_str!("../pem/GTS_Root_R1.pem"),
    sha256: hex32!(
        0xD9 0x47 0x43 0x2A 0xBD 0xE7 0xB7 0xFA 0x90 0xFC 0x2E 0x6B 0x59 0x10 0x1B 0x12
        0x80 0xE0 0xE1 0xC7 0xE4 0xE4 0x0F 0xA3 0xC6 0x88 0x7F 0xFF 0x57 0xA7 0xF4 0xCF
    ),
};

/// USERTrust RSA Certification Authority (RSA 4096): Sectigo (ex-Comodo)
/// chains, a very common commercial CA.
pub const USERTRUST_RSA: Root = Root {
    name: "USERTrust RSA Certification Authority",
    reason: "Sectigo / Comodo chains",
    pem: include_str!("../pem/USERTrust_RSA_Certification_Authority.pem"),
    sha256: hex32!(
        0xE7 0x93 0xC9 0xB0 0x2F 0xD8 0xAA 0x13 0xE2 0x1C 0x31 0x22 0x8A 0xCC 0xB0 0x81
        0x19 0x64 0x3B 0x74 0x9C 0x89 0x89 0x64 0xB1 0x74 0x6D 0x46 0xC3 0xD4 0xCB 0xD2
    ),
};

/// Every root of the set.
pub const ALL: &[Root] = &[
    ISRG_ROOT_X1,
    ISRG_ROOT_X2,
    DIGICERT_GLOBAL_ROOT_G2,
    GTS_ROOT_R1,
    USERTRUST_RSA,
];

/// All the roots as one concatenated PEM string, ready to be a dialer's
/// `ca_pem`. Built at compile time, no heap.
pub const BUNDLE: &str = concat!(
    include_str!("../pem/ISRG_Root_X1.pem"),
    include_str!("../pem/ISRG_Root_X2.pem"),
    include_str!("../pem/DigiCert_Global_Root_G2.pem"),
    include_str!("../pem/GTS_Root_R1.pem"),
    include_str!("../pem/USERTrust_RSA_Certification_Authority.pem"),
);

#[cfg(test)]
mod tests;
