# Firmware image

Pure, hardware-independent, `no_std` firmware image primitives.

- `Digest`, `parse_digest` and alloc-gated `format_digest`: SHA-256 digest and its textual `sha256:<hex>` form.
- `esp::validate`: reads an ESP application image through a caller-provided reader, checks image header, segments, checksum, optional appended hash and caller-supplied memory-map constraints. The reader and memory map are platform-supplied; no flash peripheral is owned here.

**No binding to A or B.** Either firmware domain may use these primitives if its image format is ESP-compatible; the choice of image format, slot, activation, boot/rollback, transport and authenticity policy belongs to the caller. SHA-256 integrity alone does **not** authenticate a firmware image.

Tests are ported from `iobewi_old/firmware/image`. ESP32-C3/S3 hardware bootability qualification belongs to the later integration campaign.
