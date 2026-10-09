# tcp

Embassy TCP listener and stream implementing the portable net/io interfaces.

This adapter is hardware-specific. The portable contracts and certificate persistence live in `common/`. A successful cross-build does not replace hardware checks for secure entropy, certificate verification, fail-closed handshake or transport shutdown.
