# Historical binary fixtures

Generated from the unmodified source checkout
`iobewi/iobewi_old@345378ed1ae11a4814c32ee513f20438291cfba7` by:

```sh
python3 docs/firmware/check_parity.py /path/to/iobewi_old --write-fixtures
```

Each fixture file contains 24 records, each preceded by a **test-only** 2-byte
little-endian length. This prefix does not belong to OTM1 or OTM2 flash bytes.

Normal tests compare current encoding to these frozen bytes. Do not regenerate
fixtures to make a changed codec pass: binary changes require an approved format
migration. The generator also compares old/new encoding and decoding output,
plus 4096 boot-policy scenarios, and fails on any mismatch.
