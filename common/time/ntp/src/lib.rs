#![no_std]

//! SNTP synchronization feeding the Unix epoch clock of `iobewi-time`.
//! The clock is absent until the first valid response; network failures retain
//! the last synchronized value while the service retries.

use core::net::{IpAddr, SocketAddr};

use embassy_net::Stack;
use embassy_net::dns::DnsQueryType;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_time::{Duration, Timer, with_timeout};
use log::{info, warn};

use sntpc::{NtpContext, get_time};
use sntpc_net_embassy::UdpSocketWrapper;
use sntpc_time_embassy::EmbassyTimestampGenerator;

/// Network endpoint and validation/retry policy owned by the caller.
#[derive(Clone, Copy)]
pub struct SyncOptions {
    pub server: &'static str,
    pub resync_period: Duration,
    pub retry_period: Duration,
    /// Maximum time for DNS and one UDP exchange, including a dropped reply.
    pub exchange_timeout: Duration,
    pub plausible_epoch_floor: u64,
}

/// Resync forever; a network failure retains the last estimate.
#[embassy_executor::task]
pub async fn sync_task(stack: Stack<'static>, options: SyncOptions) -> ! {
    loop {
        let attempt = with_timeout(options.exchange_timeout, sync_once(stack, options)).await
            .map_err(|_| SyncError::Timeout)
            .and_then(core::convert::identity);
        match attempt {
            Ok(epoch) => {
                iobewi_time::set_synced(epoch);
                info!("SNTP: synced, ts={epoch}");
                Timer::after(options.resync_period).await;
            }
            Err(e) => {
                warn!("SNTP: sync failed: {e:?}");
                Timer::after(options.retry_period).await;
            }
        }
    }
}

// Fields are read via the derived `Debug` impl (`warn!("... {e:?}")`), which
// rustc's dead-code lint doesn't count as a use.
#[derive(Debug)]
#[allow(dead_code)]
enum SyncError {
    Dns(embassy_net::dns::Error),
    NoAddress,
    Bind(embassy_net::udp::BindError),
    Ntp(sntpc::Error),
    Timeout,
    Implausible(u64),
}

async fn sync_once(stack: Stack<'static>, options: SyncOptions) -> Result<u64, SyncError> {
    let addrs = stack
        .dns_query(options.server, DnsQueryType::A)
        .await
        .map_err(SyncError::Dns)?;
    let addr: IpAddr = (*addrs.first().ok_or(SyncError::NoAddress)?).into();

    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buffer = [0u8; 128];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buffer = [0u8; 128];
    let mut socket =
        UdpSocket::new(stack, &mut rx_meta, &mut rx_buffer, &mut tx_meta, &mut tx_buffer);
    socket.bind(0).map_err(SyncError::Bind)?;
    let socket = UdpSocketWrapper::new(socket);

    let context = NtpContext::new(EmbassyTimestampGenerator::default());
    let result = get_time(SocketAddr::from((addr, 123)), &socket, context)
        .await
        .map_err(SyncError::Ntp)?;

    let epoch = result.sec();
    if epoch < options.plausible_epoch_floor {
        return Err(SyncError::Implausible(epoch));
    }
    Ok(epoch)
}
