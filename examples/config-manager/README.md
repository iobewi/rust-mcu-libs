# Config manager — ESP32 C3/S3 reference

The portable app claims demo32 and persists config-ready on first boot, then
verifies it on subsequent boots. The platform composes one SharedFlash owner,
EspConfigFlash, a static EKV Store and ConfigManager. No NVS or migration.

Flash an ESP-IDF compatible bootloader, the supplied partitions.csv (converted
with official gen_esp32part.py), and the application at 0x50000. The config
partition is data/0x40 at 0x9000, 256 KiB. For the first development installation,
explicitly erase this region or the test device. Subsequent reboots must retain
it. Only an entirely blank region is formatted; mount failures stop the app.
Do not flash this standalone table onto an OTA-system layout.

```sh
cargo +nightly build -Z build-std=core,alloc --release -p example-config-manager --features esp32c3 --target riscv32imc-unknown-none-elf --bins
cargo +esp build -Z build-std=core,alloc --release -p example-config-manager --features esp32s3 --target xtensa-esp32s3-none-elf --bins
```

Source app.rs stays generic over ConfigBackend; platform/esp32.rs owns physical
composition and virgin-region policy. RNG HAL supplies the per-boot allocation
seed (pseudo-random until RF activation). The heap is 64 KiB. Actual hardware
heap/timing and persistence need the board campaign; no RP/Teensy port is claimed.

Known power-fail recovery defect in EKV is tracked in #70. Only that host test
is ignored by explicit maintainer decision. Cross-linking and nominal tests do
not qualify interrupted writes. The example asserts and halts on errors; it does
not provide an implicit destructive recovery policy.

For the board campaign, add `exercise` to the chip feature. This performs one
replacement, a clear (checked absent) and a recommit, with consecutive generations
and readback assertions. Serial logs report retained/initialized generation and
`commit/clear/recommit PASS`. The final value is config-ready, so the next boot
checks persistence before exercising again. This mode deliberately writes three
times per boot; the default example only writes on first initialization.

Board procedure: [ConfigSpace and OTA campaign](../../docs/config/issue-68-board-campaign.md).
