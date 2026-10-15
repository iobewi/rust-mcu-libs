# ESP32-S3 native USB FS

`iobewi-esp-usb` preserves the native OTG construction used by StreamBeWI.
Enable `esp32s3` and compile for `xtensa-esp32s3-none-elf`. Other chips are not
implemented; without that feature the crate is empty and does not pull in esp-hal.

`device(USB_FS, GPIO20, GPIO19, out)` consumes the peripheral and D+/D- pins,
borrows a caller-owned static OUT buffer and returns the upstream Embassy-compatible
`esp_hal::usb::otg::embassy_usb_device::Driver<'static>`. No allocation is added.
The OUT buffer size is selected for the actual endpoints by the caller, per esp-hal.

The caller initializes clocks and runtime, chooses PHY/JTAG ownership, descriptors,
VID/PID, power declaration, class and readiness. Do not initialize Serial/JTAG on
the same resources before constructing OTG. This crate does not perform a warm
handover, reboot, provisioning or stream policy.

Implementation ported from `iobewi/iobewi_old` commit `345378e`,
`drivers/usb/esp32`; only the optional feature gate and workspace metadata change.
It calls esp-hal directly; there is no in-house driver or universal HAL.

Validation:

```sh
cargo +esp check -Zbuild-std=core,alloc --target xtensa-esp32s3-none-elf \
  -p iobewi-esp-usb --features esp32s3
```

The `esp32s3` CI profile runs this library check. It does not prove physical
enumeration, reset/PHY behavior, Metronic compatibility or other MCU support.
See [migration evidence](../../../docs/migrations/66-usb-stream.md).
