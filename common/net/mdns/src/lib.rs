#![no_std]

//! mDNS hostname announcement over a caller-owned UDP stack.
//!
//! This is a concrete connection between an existing network-stack owner,
//! a DNS hostname and edge-mdns; it does not implement the DNS protocol.

use core::net::{Ipv4Addr, Ipv6Addr};
use edge_mdns::{
    buf::BufferAccess,
    domain::base::Ttl,
    host::Host,
    io::{self, MdnsIoError, DEFAULT_SOCKET},
    HostAnswersMdnsHandler,
};
use edge_nal::{UdpBind, UdpSplit};
use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

/// Run a .local hostname responder until cancelled or a socket error.
///
/// The application owns the connected network stack, network interface,
/// name, current IPv4 address, packet buffers, RNG and change notification.
/// Call it anew when the interface address changes; do not advertise an
/// address that is no longer assigned. One UDP socket is consumed.
///
/// This function does not allocate, manage Wi-Fi, or advertise DNS-SD
/// services. Applications needing custom records can use edge-mdns directly.
pub async fn respond<T, RB, SB, R, M>(
    stack: &T,
    receive_buffer: RB,
    send_buffer: SB,
    rng: R,
    change: &Signal<M, ()>,
    hostname: &str,
    ipv4: Ipv4Addr,
) -> Result<(), MdnsIoError<T::Error>>
where
    T: UdpBind,
    RB: BufferAccess<[u8]>,
    SB: BufferAccess<[u8]>,
    R: rand_core::RngCore,
    M: RawMutex,
{
    let mut socket = io::bind(stack, DEFAULT_SOCKET, Some(Ipv4Addr::UNSPECIFIED), Some(0)).await?;
    let (receive, send) = socket.split();
    let host = Host {
        hostname,
        ipv4,
        ipv6: Ipv6Addr::UNSPECIFIED,
        ttl: Ttl::from_secs(60),
    };
    let mdns = io::Mdns::new(
        Some(Ipv4Addr::UNSPECIFIED),
        Some(0),
        receive,
        send,
        receive_buffer,
        send_buffer,
        rng,
        change,
    );
    mdns.run(HostAnswersMdnsHandler::new(&host)).await
}
