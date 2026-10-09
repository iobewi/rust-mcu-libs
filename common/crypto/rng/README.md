# iobewi-entropy

Portable `no_std` capability for injecting random bytes into consumers. `EntropySource` does not implement a generator or certify cryptographic quality: concrete implementations must supply suitable entropy for key generation and TLS. The existing deterministic mock is only for host tests.

Validation: host tests, Rustfmt and Clippy. Hardware RNG and secure seeding require platform qualification.
