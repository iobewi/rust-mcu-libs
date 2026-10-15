#![no_std]
#![doc = include_str!("../README.md")]
#![cfg(feature = "esp32s3")]
pub use esp_hal::usb::otg::embassy_usb_device::Driver;
use esp_hal::{
    peripherals::{GPIO19, GPIO20, USB_FS},
    usb::otg::{Usb, embassy_usb_device::Config},
};
/// Construct native full-speed OTG. Caller must not have initialized Serial/JTAG.
/// Protocol descriptors, buffers above the driver and readiness are product-owned.
pub fn device(
    usb: USB_FS<'static>,
    dp: GPIO20<'static>,
    dm: GPIO19<'static>,
    out: &'static mut [u8],
) -> Driver<'static> {
    Driver::new(Usb::new_fs(usb, dp, dm), out, Config::default())
}
