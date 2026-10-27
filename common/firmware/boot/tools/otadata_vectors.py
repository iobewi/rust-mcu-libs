#!/usr/bin/env python3
"""Independent encoder of the 32-byte boot metadata entry, to cross-check the
golden vectors of `iobewi-firmware-boot` (`the_entry_format_is_frozen_by_golden_vectors`).

Layout (little endian): seq, magic "BOOT", format version, ext_crc, reserved
(erased), commit word, ota_state, crc32(seq).
"""
import struct
import sys
import zlib

MAGIC, VERSION, COMMIT = b"BOOT", 2, 0x5AC3A53C
INIT = 0xFFFFFFFF  # ROM esp_rom_crc32_le(UINT32_MAX, ..) == zlib.crc32(data, INIT)
STATES = {"new": 0, "pending": 1, "valid": 2, "invalid": 3, "aborted": 4}


def encode(seq: int, state: int) -> bytes:
    ext = zlib.crc32(struct.pack("<II", seq, state) + MAGIC + struct.pack("<I", VERSION), INIT)
    return struct.pack("<I4sIIIIII", seq, MAGIC, VERSION, ext, 0xFFFFFFFF, COMMIT,
                       state, zlib.crc32(struct.pack("<I", seq), INIT))


if __name__ == "__main__":
    pairs = [(1, "valid"), (2, "new")] if len(sys.argv) == 1 else [(int(a.split(":")[0]), a.split(":")[1]) for a in sys.argv[1:]]
    for seq, name in pairs:
        print(f"seq={seq} state={name}: {encode(seq, STATES[name]).hex()}")
