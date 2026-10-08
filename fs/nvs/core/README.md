# iobewi-nvs-core

## Summary

Host-testable ConfigSpace NVS record framing, key validation and entry accounting.

## Responsibilities

Encode/decode CSM1 records and translate payload budgets into reserved NVS entry counts.

## Non-responsibilities

Physical flash access, locking, generation advancement, ConfigBackend implementation and configuration policy.

## Integration

Portable helper below the ESP ConfigSpace backend; it has no fs/config dependency and accepts plain byte budgets.

## Public API

`encode_record`, `decode_record`, `valid_space_name`, `entries_for_blob`, `reservation_units` and `capacity_units`; constants define a 13-byte CSM1 header and 15-byte maximum NVS key.

### Partition geometry admission

validate_partition_geometry checks erase alignment, at least two erase pages, checked end arithmetic and physical flash capacity. It does not discover labels, validate partition contents or choose a fallback address. Host tests cover valid geometry, alignment, size, overflow and bounds errors.

## Testing

- `cargo test -p iobewi-nvs-core` checks golden bytes, corruption, key rules and accounting overflow.

## Known limitations

Uses alloc for encoding. Keys must be nonempty ASCII without NUL and at most 15 bytes. Decoder rejects short headers, wrong magic and unknown flag bits. Reservations include two versions of the largest record: 2 × (ceil((13 + budget)/32) + ceil((13 + budget)/4000) + 1); capacity keeps one page in reserve. This is entry accounting, not a guarantee against all storage failures.
