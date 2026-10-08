#![no_std]

//! Transport-free Improv Serial provisioning coordinator.
//! Parsing and framing belong to improv-serial; persisted credentials and
//! reconnection belong to the Wi-Fi manager behind WifiProvisioning.
//! The application owns serial I/O, authorization, UX and service startup.

extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use improv_serial::{self as improv, Command, ImprovError, ParsedCommand, Parser, State};
use iobewi_wifi_core::WifiProvisioning;

/// Product identity is supplied by the caller, never derived from a HAL.
pub struct DeviceInfo<'a> {
    pub firmware_name: &'a str,
    pub firmware_version: &'a str,
    pub chip_name: &'a str,
    pub device_name: &'a str,
}

/// Semantic event for the application to react to without coupling to its
/// status indicator, service supervisor, or reboot/recovery policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Scanning,
    Connecting,
    Connected,
    ConnectionFailed,
}

/// Responses are ordered; the caller writes each frame to the same transport
/// that delivered the command. Events contain no credentials.
pub struct Reply {
    pub frames: Vec<Vec<u8>>,
    pub event: Option<Event>,
}

/// Reusable state, independent of physical serial ports. One instance per
/// device provisioning session; consumers may route multiple ports to it.
pub struct Provisioning {
    parser: Parser,
    state: State,
}

impl Provisioning {
    pub fn new(online: bool) -> Self {
        Self {
            parser: Parser::new(),
            state: if online { State::Provisioned } else { State::Authorized },
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Returns a decoded RPC, if the incoming byte completes one.
    pub fn feed(&mut self, byte: u8) -> Option<ParsedCommand> {
        self.parser.feed(byte)
    }

    /// Should be invoked when the network manager reports a link transition.
    pub fn sync_online(&mut self, online: bool) {
        self.state = if online { State::Provisioned } else { State::Authorized };
    }

    /// Executes one RPC against the platform-neutral Wi-Fi capability.
    /// Caller controls authorization and response transmission.
    pub async fn handle<W: WifiProvisioning>(
        &mut self,
        command: ParsedCommand,
        wifi: &mut W,
        info: &DeviceInfo<'_>,
    ) -> Reply {
        let mut frames = Vec::new();
        let mut event = None;
        match command {
            ParsedCommand::GetCurrentState => {
                self.sync_online(wifi.is_online());
                frames.push(improv::state_frame(self.state));
                // ESP Web Tools waits for RPC result when already provisioned.
                if self.state == State::Provisioned {
                    let url = next_url(wifi);
                    frames.push(improv::rpc_response_frame(Command::GetCurrentState, &[url.as_bytes()]));
                }
            }
            ParsedCommand::GetDeviceInfo => {
                frames.push(improv::rpc_response_frame(Command::GetDeviceInfo, &[
                    info.firmware_name.as_bytes(),
                    info.firmware_version.as_bytes(),
                    info.chip_name.as_bytes(),
                    info.device_name.as_bytes(),
                ]));
            }
            ParsedCommand::GetWifiNetworks => {
                event = Some(Event::Scanning);
                for network in wifi.scan().await {
                    let strength = format!("{}", network.signal_strength);
                    let secured: &[u8] = if network.secured { b"YES" } else { b"NO" };
                    frames.push(improv::rpc_response_frame(Command::GetWifiNetworks, &[
                        network.ssid.as_bytes(), strength.as_bytes(), secured,
                    ]));
                }
                frames.push(improv::rpc_response_frame(Command::GetWifiNetworks, &[]));
            }
            ParsedCommand::GetNetworkState => {
                let flags = if wifi.is_online() { "3" } else { "2" };
                if wifi.is_online() {
                    let url = next_url(wifi);
                    frames.push(improv::rpc_response_frame(Command::GetNetworkState, &[
                        flags.as_bytes(), url.as_bytes(),
                    ]));
                } else {
                    frames.push(improv::rpc_response_frame(Command::GetNetworkState, &[flags.as_bytes()]));
                }
            }
            ParsedCommand::WifiSettings(settings) => {
                self.state = State::Provisioning;
                frames.push(improv::state_frame(self.state));
                if wifi.provision(&settings.ssid, settings.password).await {
                    self.state = State::Provisioned;
                    event = Some(Event::Connected);
                    frames.push(improv::state_frame(self.state));
                    let url = next_url(wifi);
                    frames.push(improv::rpc_response_frame(Command::WifiSettings, &[url.as_bytes()]));
                } else {
                    self.state = State::Authorized;
                    event = Some(Event::ConnectionFailed);
                    frames.push(improv::error_frame(ImprovError::UnableToConnect));
                    frames.push(improv::state_frame(self.state));
                }
            }
            ParsedCommand::Unsupported(_) => {
                frames.push(improv::error_frame(ImprovError::UnknownRpc));
            }
        }
        Reply { frames, event }
    }
}

fn next_url<W: WifiProvisioning>(wifi: &W) -> String {
    wifi.address().map(|address| format!("https://{address}/")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initialization_and_link_transitions() {
        let mut session = Provisioning::new(false);
        assert_eq!(session.state(), State::Authorized);
        session.sync_online(true);
        assert_eq!(session.state(), State::Provisioned);
        session.sync_online(false);
        assert_eq!(session.state(), State::Authorized);
    }
}
