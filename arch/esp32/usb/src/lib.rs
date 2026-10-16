#![no_std]
#![doc = include_str!("../README.md")]
#![cfg(feature = "esp32s3")]

use core::future::Future;

use embassy_usb_driver as driver;
use embassy_usb_synopsys_otg::otg_v1::Otg;
use esp_hal::{
    peripherals::{GPIO19, GPIO20, USB_FS},
    usb::otg::{
        Usb,
        embassy_usb_device::{Config, Driver as EspDriver},
    },
};

/// Native full-speed OTG driver of esp-hal, wrapped so that a bus reset disarms the IN endpoints.
///
/// esp-hal / `embassy-usb-synopsys-otg` 0.4 leave an IN transfer that was armed when the host
/// disappeared armed through the next bus reset (see the README). The wrapper only adds that
/// clean-up; every other call is delegated unchanged.
pub struct Driver<'d> {
    inner: EspDriver<'d>,
}

/// Bus of [`Driver`]: esp-hal's bus plus the IN endpoint clean-up on reset.
pub struct Bus<'d> {
    inner: <EspDriver<'d> as driver::Driver<'d>>::Bus,
    regs: Otg,
}

impl<'d> driver::Driver<'d> for Driver<'d> {
    type EndpointOut = <EspDriver<'d> as driver::Driver<'d>>::EndpointOut;
    type EndpointIn = <EspDriver<'d> as driver::Driver<'d>>::EndpointIn;
    type ControlPipe = <EspDriver<'d> as driver::Driver<'d>>::ControlPipe;
    type Bus = Bus<'d>;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: driver::EndpointType,
        ep_addr: Option<driver::EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointOut, driver::EndpointAllocError> {
        self.inner
            .alloc_endpoint_out(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn alloc_endpoint_in(
        &mut self,
        ep_type: driver::EndpointType,
        ep_addr: Option<driver::EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointIn, driver::EndpointAllocError> {
        self.inner
            .alloc_endpoint_in(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe) {
        let (inner, control) = self.inner.start(control_max_packet_size);
        // SAFETY: the register block of the OTG FS peripheral this driver owns; the clean-up only
        // runs inside a critical section, as the upstream driver does for the same registers.
        let regs = unsafe { Otg::from_ptr(USB_FS::PTR.cast_mut().cast::<()>()) };
        (Bus { inner, regs }, control)
    }
}

impl driver::Bus for Bus<'_> {
    fn enable(&mut self) -> impl Future<Output = ()> {
        self.inner.enable()
    }

    fn disable(&mut self) -> impl Future<Output = ()> {
        self.inner.disable()
    }

    async fn poll(&mut self) -> driver::Event {
        let event = self.inner.poll().await;
        if event == driver::Event::Reset {
            disarm_in_endpoints(&self.regs);
        }
        event
    }

    fn endpoint_set_enabled(&mut self, ep_addr: driver::EndpointAddress, enabled: bool) {
        self.inner.endpoint_set_enabled(ep_addr, enabled)
    }

    fn endpoint_set_stalled(&mut self, ep_addr: driver::EndpointAddress, stalled: bool) {
        self.inner.endpoint_set_stalled(ep_addr, stalled)
    }

    fn endpoint_is_stalled(&mut self, ep_addr: driver::EndpointAddress) -> bool {
        self.inner.endpoint_is_stalled(ep_addr)
    }

    fn force_reset(&mut self) -> Result<(), driver::Unsupported> {
        self.inner.force_reset()
    }

    fn remote_wakeup(&mut self) -> impl Future<Output = Result<(), driver::Unsupported>> {
        self.inner.remote_wakeup()
    }
}

/// Highest IN endpoint index: esp-hal allocates the device state for 7 endpoints (0..=6), the same
/// count `embassy-usb-synopsys-otg` iterates over.
const LAST_IN_ENDPOINT: usize = 6;
/// Upper bound on the wait for the core to confirm a disable, inside a critical section: the core does
/// it within microseconds, and the bound only keeps the worst case (an endpoint that never confirms)
/// short, about a millisecond per endpoint.
const DISABLE_SPINS: u32 = 10_000;

/// Disable every non-control IN endpoint that still has a transfer armed (`EPENA`), the way
/// `embassy-usb-synopsys-otg` does when VBUS is lost (SNAK + EPDIS, wait for EPDISD).
///
/// Control endpoint 0 is left alone: its transfers restart with the next SETUP.
fn disarm_in_endpoints(regs: &Otg) {
    critical_section::with(|_| {
        for index in 1..=LAST_IN_ENDPOINT {
            let ctl = regs.diepctl(index);
            if !ctl.read().epena() {
                continue;
            }
            ctl.modify(|w| {
                w.set_snak(true);
                w.set_epdis(true);
            });
            let int = regs.diepint(index);
            let mut spins = DISABLE_SPINS;
            while !int.read().epdisd() && spins > 0 {
                spins -= 1;
            }
            int.modify(|w| w.set_epdisd(true));
        }
    });
}

/// Construct native full-speed OTG. Caller must not have initialized Serial/JTAG.
/// Protocol descriptors, buffers above the driver and readiness are product-owned.
pub fn device(
    usb: USB_FS<'static>,
    dp: GPIO20<'static>,
    dm: GPIO19<'static>,
    out: &'static mut [u8],
) -> Driver<'static> {
    Driver {
        inner: EspDriver::new(Usb::new_fs(usb, dp, dm), out, Config::default()),
    }
}

/// Soft-connect (`true`) or soft-disconnect (`false`) the device from the bus.
///
/// This toggles `DCTL.SDIS`, the bit `embassy-usb-synopsys-otg` itself sets while it initialises the
/// core: with it set the D+ pull-up is released and the host sees the device removed; clearing it
/// presents the device again and the host resets the bus and re-enumerates. The USB device stack
/// handles that reset like any other, so this is the way for a product to leave the bus while it has
/// nothing valid to serve and come back later. Call it after [`device`] has been handed to the USB
/// stack; it does not touch the endpoints (the bus-reset clean-up of [`Driver`] runs when the host
/// resets the bus after reconnection).
pub fn set_connected(connected: bool) {
    // SAFETY: the register block of the OTG FS peripheral owned by the driver; the read-modify-write is
    // inside a critical section, as the upstream driver does for the same register.
    let regs = unsafe { Otg::from_ptr(USB_FS::PTR.cast_mut().cast::<()>()) };
    critical_section::with(|_| regs.dctl().modify(|w| w.set_sdis(!connected)));
}
