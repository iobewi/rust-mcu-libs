# iobewi-esp-watchdog

A minimal hardware-specific ESP32 watchdog primitive using the TIMG0 MWDT implemented by `esp-hal`. Unlike application lifecycle supervisors, it only exposes `arm_ms(timeout_ms)`, `feed()`, and `disable()`.

## Hardware ownership and ordering

`esp_hal::init()` disables the hardware watchdogs. **Initialize TIMG0 for `esp-rtos` before arming**; constructing the timer group after arming resets the peripheral and clears its watchdog settings. Only one firmware composition must own this peripheral and coordinate feeds. The caller chooses deadlines and handles reboot/OTA/self-check policy. Do not assume watchdog expiry alone verifies rollback.

Features `esp32c3` and `esp32s3` select the target. No HAL-free portable watchdog abstraction is introduced; no application-specific boot lifecycle resides here.

## Qualification

Cross-compile each supported target using the repository's ESP32 CI; subsequently check on real hardware: timeout expiry, repeated feed, disable, and interactions with the actual `esp-rtos` initialization sequence. The hardware gate is **not** implied by CI build success.
