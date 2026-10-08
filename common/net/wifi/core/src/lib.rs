#![no_std]
#![allow(async_fn_in_trait)]

//! Portable Wi-Fi contracts. No persistence, provisioning policy, radio,
//! DHCP or network-stack types: a platform driver implements
//! [`WifiTransport`] (station) and, when it can host one, [`WifiAccessPoint`];
//! a manager (see `iobewi-wifi-manager`) consumes them and exposes
//! [`WifiProvisioning`] to provisioning workflows.
//!
//! "Online" means what the transport reports through [`WifiTransport::is_online`]:
//! for the ESP driver, associated *and* IPv4 configured by DHCP.

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

pub struct Network {
    pub ssid: String,
    pub signal_strength: i8,
    pub secured: bool,
}

/// The platform supplies radio and network mechanics; the service controls
/// which credentials become authoritative after a connection attempt.
pub trait WifiTransport {
    type Address;
    /// Opaque handle to whatever network stack the platform runs once
    /// online (an `embassy_net::Stack`, a different runtime's socket
    /// manager, ...). This crate never interprets it -- it only carries it
    /// from the transport up to [`WifiManager`]'s own caller.
    type NetworkHandle: Copy;

    async fn connect(&mut self, ssid: &str, password: String) -> bool;
    async fn scan(&mut self) -> Vec<Network>;
    /// Resolves when the link/IP configuration is lost (immediately if it is
    /// already down). A pure event primitive: it neither retries nor
    /// reconnects -- that policy belongs to the manager.
    async fn wait_down(&mut self);
    fn ip(&self) -> Option<Self::Address>;
    fn network_handle(&self) -> Option<Self::NetworkHandle>;
    fn is_online(&self) -> bool;
}

/// Consumer-facing capability: the functional operations a Wi-Fi
/// provisioning workflow (e.g. Improv Serial) needs. Deliberately narrower
/// than [`WifiTransport`] (the platform-facing port `WifiManager` itself
/// consumes) -- a provisioning UI has no business touching durable-config
/// internals, only scanning, provisioning, and reading the resulting state.
#[allow(async_fn_in_trait)]
pub trait WifiProvisioning {
    type Address: core::fmt::Display;
    type NetworkHandle: Copy;

    async fn scan(&mut self) -> Vec<Network>;
    async fn provision(&mut self, ssid: &str, password: String) -> bool;

    fn address(&self) -> Option<Self::Address>;
    fn network_handle(&self) -> Option<Self::NetworkHandle>;
    fn is_online(&self) -> bool;
}

/// Why an [`AccessPointConfig`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPointConfigError {
    /// The SSID is empty or longer than 32 bytes.
    Ssid,
    /// The WPA2 passphrase is not 8 to 63 bytes.
    Password,
    /// The 2.4 GHz channel is not 1 to 13.
    Channel,
}

impl core::fmt::Display for AccessPointConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Ssid => "access point SSID must be 1 to 32 bytes",
            Self::Password => "access point passphrase must be 8 to 63 bytes",
            Self::Channel => "access point channel must be 1 to 13",
        })
    }
}

/// Settings of a soft access point, supplied by the product and kept in RAM.
///
/// Always WPA2-protected: an open access point is deliberately not
/// representable. Choosing a unique passphrase is product policy. `Debug`
/// never prints the passphrase.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessPointConfig {
    ssid: String,
    password: String,
    channel: u8,
}

impl AccessPointConfig {
    pub fn new(ssid: &str, password: &str, channel: u8) -> Result<Self, AccessPointConfigError> {
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(AccessPointConfigError::Ssid);
        }
        if !(8..=63).contains(&password.len()) {
            return Err(AccessPointConfigError::Password);
        }
        if !(1..=13).contains(&channel) {
            return Err(AccessPointConfigError::Channel);
        }
        Ok(Self {
            ssid: String::from(ssid),
            password: String::from(password),
            channel,
        })
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// Preferred channel. When a station is associated the radio follows its
    /// access point's channel, so this is a hint, not a guarantee.
    pub fn channel(&self) -> u8 {
        self.channel
    }
}

impl core::fmt::Debug for AccessPointConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AccessPointConfig")
            .field("ssid_len", &self.ssid.len())
            .field("channel", &self.channel)
            .finish_non_exhaustive()
    }
}

/// A soft access point hosted by the same radio as the station.
///
/// A platform implements this on the same object that implements
/// [`WifiTransport`]: one radio, one owner. Starting or stopping the access
/// point **may restart the whole radio** (the ESP driver does, because the
/// underlying radio library applies a mode change that way), so the station
/// link can drop. Recovery is the manager's reconnection loop, not this
/// port's. Starting never persists anything.
///
/// `NetworkHandle` is whatever the platform uses to reach the access point's
/// own network (an `embassy_net::Stack` on ESP); it is valid only while the
/// access point is active.
pub trait WifiAccessPoint {
    type NetworkHandle: Copy;

    /// Starts the access point and returns once it is ready to accept
    /// clients and hand out addresses, or `false` if it could not be started.
    /// Starting an already active access point with the same settings is a
    /// no-op that succeeds; with different settings it reconfigures it.
    async fn start_access_point(&mut self, config: &AccessPointConfig) -> bool;

    /// Stops the access point and every service bound to its network.
    /// Stopping an inactive access point succeeds trivially.
    async fn stop_access_point(&mut self);

    fn is_access_point_active(&self) -> bool;

    /// The access point's network handle while it is active, `None` otherwise.
    fn access_point_handle(&self) -> Option<Self::NetworkHandle>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_configuration_is_accepted_and_read_back() {
        let c = AccessPointConfig::new("IOBEWI-Setup", "12345678", 6).unwrap();
        assert_eq!(
            (c.ssid(), c.password(), c.channel()),
            ("IOBEWI-Setup", "12345678", 6)
        );
    }

    #[test]
    fn ssid_length_is_one_to_thirty_two_bytes() {
        assert_eq!(
            AccessPointConfig::new("", "12345678", 1),
            Err(AccessPointConfigError::Ssid)
        );
        assert!(AccessPointConfig::new(&"a".repeat(32), "12345678", 1).is_ok());
        assert_eq!(
            AccessPointConfig::new(&"a".repeat(33), "12345678", 1),
            Err(AccessPointConfigError::Ssid)
        );
        // Length is in bytes: 17 two-byte characters are 34 bytes.
        assert_eq!(
            AccessPointConfig::new(&"é".repeat(17), "12345678", 1),
            Err(AccessPointConfigError::Ssid)
        );
    }

    #[test]
    fn passphrase_is_wpa2_sized_so_an_open_access_point_cannot_exist() {
        assert_eq!(
            AccessPointConfig::new("x", "", 1),
            Err(AccessPointConfigError::Password)
        );
        assert_eq!(
            AccessPointConfig::new("x", "1234567", 1),
            Err(AccessPointConfigError::Password)
        );
        assert!(AccessPointConfig::new("x", "12345678", 1).is_ok());
        assert!(AccessPointConfig::new("x", &"p".repeat(63), 1).is_ok());
        assert_eq!(
            AccessPointConfig::new("x", &"p".repeat(64), 1),
            Err(AccessPointConfigError::Password)
        );
    }

    #[test]
    fn channel_is_one_to_thirteen() {
        assert_eq!(
            AccessPointConfig::new("x", "12345678", 0),
            Err(AccessPointConfigError::Channel)
        );
        assert!(AccessPointConfig::new("x", "12345678", 1).is_ok());
        assert!(AccessPointConfig::new("x", "12345678", 13).is_ok());
        assert_eq!(
            AccessPointConfig::new("x", "12345678", 14),
            Err(AccessPointConfigError::Channel)
        );
    }

    #[test]
    fn debug_never_prints_the_passphrase() {
        let c = AccessPointConfig::new("IOBEWI-Setup", "super-secret-pass", 1).unwrap();
        let text = std::format!("{c:?}");
        assert!(!text.contains("super-secret-pass"));
        assert!(!text.contains("IOBEWI-Setup"));
    }
}
