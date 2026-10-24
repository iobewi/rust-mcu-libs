# ESP32-S3 native USB FS

`iobewi-esp-usb` preserves the native OTG construction used by StreamBeWI.
Enable `esp32s3` and compile for `xtensa-esp32s3-none-elf`. Other chips are not
implemented; without that feature the crate is empty and does not pull in esp-hal.

The pins are fixed by the chip: with the internal PHY of the ESP32-S3, D+ is GPIO20 and D- is
GPIO19, whatever a board's connector labels say. They are not selectable, which is why the
signature takes the concrete `GPIO20` and `GPIO19` types. The ESP32-C3 has no USB OTG controller
(only USB Serial/JTAG on GPIO18/GPIO19), so there is nothing to port there. See
[ESP32 GPIO reference](../../../docs/hardware/esp32-gpio.md).

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

## Bus-reset clean-up of IN endpoints

`device(...)` returns a thin wrapper around esp-hal's driver (`Driver<'static>`, same name and
construction). On every bus reset it disables the non-control IN endpoints that still have a
transfer armed (`EPENA`), with the same SNAK + EPDIS sequence `embassy-usb-synopsys-otg` uses when
VBUS is lost. Everything else is delegated unchanged.

Why: with esp-hal 1.2 / `embassy-usb-synopsys-otg` 0.4 an IN transfer that was armed when the host
disappeared (cable pulled during a read) stays armed through the next bus reset: `embassy-usb`
does not disable endpoints on reset, the driver rewrites `DIEPCTL` without clearing `EPENA`, and
`disable_all_endpoints` only runs with VBUS detection, which the S3 does not have. The class then
waits forever for "the previous transfer" before its first reply of the new session (the host
sees a command that never completes, resets the bus every few seconds and gives up) until the
chip is reset. Pulling the cable during a continuous read reproduces it every time.

This is a workaround for that upstream behaviour; it is not a protocol feature and can be removed
once the driver clears armed IN transfers on reset. It needs hardware to be exercised: host tests
cannot reach these registers.

## Leaving and rejoining the bus

`set_connected(false)` soft-disconnects the device (the host sees it removed); `set_connected(true)`
presents it again and the host re-enumerates. A product uses it when its backing data is gone and
comes back later, instead of answering reads with data it does not have. Hardware only.
