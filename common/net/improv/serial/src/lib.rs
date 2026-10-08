#![cfg_attr(not(test), no_std)]

//! Minimal Improv Serial implementation (<https://www.improv-wifi.com/serial/>).
//!
//! The crate contains only protocol framing and parsing. It has no dependency
//! on ESP hardware, a serial driver, an async runtime, Wi-Fi, or an application
//! framework. Callers feed received bytes to [`Parser::feed`] and write the
//! returned frame bytes using whatever transport they own.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

const HEADER: &[u8; 6] = b"IMPROV";
const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum FrameType {
    CurrentState = 0x01,
    ErrorState = 0x02,
    Rpc = 0x03,
    RpcResponse = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Stopped = 0x00,
    Authorized = 0x02,
    Provisioning = 0x03,
    Provisioned = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ImprovError {
    UnknownRpc = 0x02,
    UnableToConnect = 0x03,
}

/// RPC command IDs. `GetCurrentState` doubles as "Identify" on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    WifiSettings = 0x01,
    GetCurrentState = 0x02,
    GetDeviceInfo = 0x03,
    GetWifiNetworks = 0x04,
    GetNetworkState = 0x07,
}

impl Command {
    fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0x01 => Self::WifiSettings,
            0x02 => Self::GetCurrentState,
            0x03 => Self::GetDeviceInfo,
            0x04 => Self::GetWifiNetworks,
            0x07 => Self::GetNetworkState,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiSettings {
    pub ssid: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    WifiSettings(WifiSettings),
    GetCurrentState,
    GetDeviceInfo,
    GetWifiNetworks,
    GetNetworkState,
    Unsupported(u8),
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

fn frame(frame_type: FrameType, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len() + 2);
    out.extend_from_slice(HEADER);
    out.push(VERSION);
    out.push(frame_type as u8);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    out.push(checksum(&out));
    out.push(b'\n');
    out
}

pub fn state_frame(state: State) -> Vec<u8> {
    frame(FrameType::CurrentState, &[state as u8])
}

pub fn error_frame(error: ImprovError) -> Vec<u8> {
    frame(FrameType::ErrorState, &[error as u8])
}

/// Builds an RPC response carrying length-prefixed string entries.
pub fn rpc_response_frame(command: Command, strings: &[&[u8]]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(command as u8);
    payload.push(0);
    let strings_start = payload.len();
    for s in strings {
        payload.push(s.len() as u8);
        payload.extend_from_slice(s);
    }
    payload[1] = (payload.len() - strings_start) as u8;
    payload.push(0);
    frame(FrameType::RpcResponse, &payload)
}

/// Byte-by-byte Improv Serial parser.
pub struct Parser {
    buffer: Vec<u8>,
}

impl Parser {
    pub const fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn feed(&mut self, byte: u8) -> Option<ParsedCommand> {
        let position = self.buffer.len();
        let synced = match position {
            0 => byte == b'I',
            1 => byte == b'M',
            2 => byte == b'P',
            3 => byte == b'R',
            4 => byte == b'O',
            5 => byte == b'V',
            6 => byte == VERSION,
            7 | 8 => true,
            _ => {
                let data_len = self.buffer[8] as usize;
                if position <= 8 + data_len {
                    true
                } else if position == 8 + data_len + 1 {
                    checksum(&self.buffer) == byte
                } else {
                    false
                }
            }
        };

        if !synced {
            self.buffer.clear();
            return None;
        }

        self.buffer.push(byte);
        if self.buffer.len() < 9 {
            return None;
        }

        let data_len = self.buffer[8] as usize;
        if self.buffer.len() != 9 + data_len + 1 {
            return None;
        }

        let frame_type = self.buffer[7];
        let result = (frame_type == FrameType::Rpc as u8)
            .then(|| Self::parse_rpc_payload(&self.buffer[9..9 + data_len]))
            .flatten();
        self.buffer.clear();
        result
    }

    fn parse_rpc_payload(data: &[u8]) -> Option<ParsedCommand> {
        let command_byte = *data.first()?;
        let data_length = *data.get(1)? as usize;
        if data.len() != 2 + data_length {
            return None;
        }

        let command = Command::from_byte(command_byte);
        if command != Some(Command::WifiSettings) {
            return Some(match command {
                Some(Command::GetCurrentState) => ParsedCommand::GetCurrentState,
                Some(Command::GetDeviceInfo) => ParsedCommand::GetDeviceInfo,
                Some(Command::GetWifiNetworks) => ParsedCommand::GetWifiNetworks,
                Some(Command::GetNetworkState) => ParsedCommand::GetNetworkState,
                _ => ParsedCommand::Unsupported(command_byte),
            });
        }

        let ssid_len = *data.get(2)? as usize;
        let ssid_start = 3;
        let ssid_end = ssid_start + ssid_len;
        let pass_len = *data.get(ssid_end)? as usize;
        let pass_start = ssid_end + 1;
        let pass_end = pass_start + pass_len;
        if pass_end > data.len() {
            return None;
        }

        Some(ParsedCommand::WifiSettings(WifiSettings {
            ssid: String::from_utf8_lossy(&data[ssid_start..ssid_end]).into_owned(),
            password: String::from_utf8_lossy(&data[pass_start..pass_end]).into_owned(),
        }))
    }
}

impl Default for Parser {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc_frame(command: u8, body: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(body.len() + 2);
        payload.push(command);
        payload.push(body.len() as u8);
        payload.extend_from_slice(body);
        frame(FrameType::Rpc, &payload)
    }

    fn parse(bytes: &[u8]) -> Option<ParsedCommand> {
        let mut parser = Parser::new();
        bytes.iter().find_map(|&b| parser.feed(b))
    }

    #[test]
    fn parses_wifi_settings() {
        let mut body = Vec::new();
        body.push(4);
        body.extend_from_slice(b"test");
        body.push(6);
        body.extend_from_slice(b"secret");
        assert_eq!(
            parse(&rpc_frame(Command::WifiSettings as u8, &body)),
            Some(ParsedCommand::WifiSettings(WifiSettings {
                ssid: String::from("test"),
                password: String::from("secret"),
            }))
        );
    }

    #[test]
    fn parses_known_rpc_commands() {
        assert_eq!(parse(&rpc_frame(Command::GetCurrentState as u8, &[])), Some(ParsedCommand::GetCurrentState));
        assert_eq!(parse(&rpc_frame(Command::GetDeviceInfo as u8, &[])), Some(ParsedCommand::GetDeviceInfo));
        assert_eq!(parse(&rpc_frame(Command::GetWifiNetworks as u8, &[])), Some(ParsedCommand::GetWifiNetworks));
        assert_eq!(parse(&rpc_frame(Command::GetNetworkState as u8, &[])), Some(ParsedCommand::GetNetworkState));
    }

    #[test]
    fn reports_unknown_rpc() {
        assert_eq!(parse(&rpc_frame(0x55, &[])), Some(ParsedCommand::Unsupported(0x55)));
    }

    #[test]
    fn ignores_bad_checksum() {
        let mut bytes = rpc_frame(Command::GetCurrentState as u8, &[]);
        let checksum_index = bytes.len() - 2;
        bytes[checksum_index] ^= 0x01;
        assert_eq!(parse(&bytes), None);
    }

    #[test]
    fn resynchronizes_after_noise() {
        let frame = rpc_frame(Command::GetDeviceInfo as u8, &[]);
        let mut parser = Parser::new();
        for &b in b"log line\nnoise" {
            assert_eq!(parser.feed(b), None);
        }
        assert_eq!(frame.into_iter().find_map(|b| parser.feed(b)), Some(ParsedCommand::GetDeviceInfo));
    }

    #[test]
    fn emitted_frames_have_valid_checksum_and_newline() {
        for bytes in [
            state_frame(State::Authorized),
            error_frame(ImprovError::UnableToConnect),
            rpc_response_frame(Command::GetDeviceInfo, &[b"device", b"1.0"]),
        ] {
            assert_eq!(bytes.last(), Some(&b'\n'));
            let checksum_index = bytes.len() - 2;
            assert_eq!(checksum(&bytes[..checksum_index]), bytes[checksum_index]);
        }
    }
}
