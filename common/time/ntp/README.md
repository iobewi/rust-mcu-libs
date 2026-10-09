# iobewi-ntp

Portable SNTP synchronization feeding `common/time/core` (`iobewi-time`). Based on the previously implemented `iobewi_old/time/ntp` module; no direct HAL dependency and no network abstraction layer.

## Usage

Spawn `sync_task(stack, SyncOptions { ... })` from an Embassy executor after initializing the network stack. The caller owns the DNS server name, polling and retry intervals, request timeout, and minimum plausible Unix epoch. Read `iobewi_time::now()` independently of NTP; it returns `None` until the first sync. The cached clock keeps advancing during network interruptions and corrects on subsequent sync.

The UDP client binds an ephemeral local port (0); its remote port is 123. NTP exchange and DNS are bounded by `exchange_timeout`. Network errors cause retries; time is retained.

## Security and limitations

SNTP responses are unauthenticated. `plausible_epoch_floor` checks a lower bound, **not** authenticity or protection against rollback or spoofing. Consumers relying on accurate civil time for TLS certificate date checks must explicitly assess their time-trust model. The clock does not persist across reset and can drift between synchronizations. No NTS implementation is provided.

## Qualification

Host CI profile checks basic portability; firmware cross-compilation and ESP32-C3/S3 hardware synchronization remain to be demonstrated. Dependency versions are inherited from the historical implementation and must be revalidated with current upstream releases by CI before merge.
