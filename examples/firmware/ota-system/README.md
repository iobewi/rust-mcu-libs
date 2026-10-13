# OTA du domaine système A

**État actuel : démarrage, probation et confirmation raccordés et construits.**
La réception OTA, sa publication et l'activation depuis un transfert restent à
raccorder avant de déclarer cet exemple comme étalon de chaîne complète.
Le code existant est repris, sans copie du moteur ni modification du protocole.

La cible est réception → écriture inactive → vérification → publication OTM1 →
activation → reboot → probation → confirmation ou rollback. Les variantes refus
et blocage servent à qualifier la chaîne. La table `partitions.csv` définit les
régions du scénario, avec une flash d'au moins 4 Mio. Elle reste un profil
explicite de l'exemple ; la bibliothèque découvre les partitions.

# Raccordement de confirmation existant

This C3/S3 executable complements examples/firmware/bootloader. It is a small
real composition, not an OTA HTTP application: it runs the existing portable
boot gate through SystemBoot and the existing TIMG0 watchdog primitive.

Startup: HAL init -> TimerGroup TIMG0 -> esp-rtos start -> watchdog arm (20 s) ->
single SharedFlash init -> table/NVS/OTM1 discovery -> boot reconciliation.
No later TimerGroup is constructed. The system owner is exclusively borrowed
by BootRuntime during reconciliation/self-check; its physical flash mutex is
released before ConfigSpace. The self-check performs the existing NVS canary
write/read/delete. Products can supply another BootSelfCheck or compose further
health checks; the library does not assume Wi-Fi/HTTPS readiness.

The example reserves its own `ota` ConfigSpace claim with a 2048-byte budget.
A product must reuse its existing OTM1 claim/schema and budget. No transaction
is fabricated and no invalid configuration is erased. An unaccounted pending
image is rejected by the existing gate. A matching Activating OTM1 record is
required to enter probation; merely writing PendingVerify does not authorize
confirmation.

Probation: self-check deadline 15 s; success calls confirm_pending, which writes
and verifies the boot record before disabling the watchdog. Failed self-check
rejects and resets after the historical 200 ms settling delay. Timeout resets
with the entry still pending. Failed confirm/reject never disarms. If the boot
record is durably confirmed but OTM1 completion fails, the gate reports Degraded
and retries completion at next boot. A stable image is self-checked without
probation reset policy. Unexpected startup/discovery failure halts with the
watchdog still armed. The application owns all reset decisions.

## Build

```sh
cargo +nightly build -Z build-std=core,alloc --release   --target riscv32imc-unknown-none-elf -p example-firmware-ota-system --features esp32c3 --bins
cargo +esp build -Z build-std=core,alloc --release   --target xtensa-esp32s3-none-elf -p example-firmware-ota-system --features esp32s3 --bins
```

Locally tested with esp-hal 1.2.2, esp-rtos 0.4.0, esp-alloc 0.11.0 and the
current Embassy versions from the workspace. Refusal/hang variants also linked
on both targets. S3 needs Xtensa GCC on PATH. Upstream esp-hal linkall.x is the application linker;
do not use the second-stage linker for this image. CI profile esp32-linked-bins
requires a real release link on both targets. The existing upstream S3 RWX LOAD
warning is retained; a link does not prove execution or memory isolation.

Add `reject-self-check` for refusal or `hang-self-check` for a cooperative hung
check. These fault features are mutually exclusive. Tests must begin from a
proper accounted activation, with a confirmed fallback, using the historical
bootloader and compatible ota_0/ota_1/otadata/NVS table. No automatic deployment,
flash or two-image setup is done by this example.

## Validate a generated image on host

Convert ELF with esptool 5.5.0 elf2image, then run the actual common validator
with the bootloader's chip MemoryMap, for each slot:

```sh
cargo run -p iobewi-esp-boot --example validate_image --features esp32c3 -- app.bin 20000 180000
cargo run -p iobewi-esp-boot --example validate_image --features esp32c3 -- app.bin 1a0000 180000
```

Use esp32s3 for S3. Offsets/capacity are hexadecimal example layout values, not
hardcoded production discovery. The utility checks both Structure and Full;
the second-stage policy itself remains Structure. Nominal generated C3/S3 images
were accepted on host for both slots. This does not validate signature/authenticity,
physical flash geometry, stack consumption, cache/MMU handover or boot timing.

## Qualification still required

No board is attached. On hardware: first boot, stable reboot, valid probation,
self-check refusal and timeout, a blocked executor/watchdog expiry, record write
interruption and post-confirm OTM1 failure; observe real fallback and verify the
confirmed slot/configuration remain intact. No rescue image or reset of persisted
configuration is added. Product OTA endpoints, upload composition and supervised
B/C runtime/backend qualification remain separate work.
