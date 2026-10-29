#!/usr/bin/env python3
"""Tests of the flash geometry tool (esp_geometry.py): no toolchain, no hardware."""
import struct
import tempfile
from pathlib import Path

import esp_geometry as g

ROOT = Path(__file__).resolve().parents[2]
SYSTEM = ROOT / "examples/firmware/ota-system"
VAR = g.VARIABLE


def refuses(call, *args, **kwargs):
    try:
        call(*args, **kwargs)
    except g.GeometryError:
        return True
    return False


# --- the one setting -------------------------------------------------------------------------
assert g.table_offset({}) == 0x8000, "unset means the historical offset"
assert g.table_offset({VAR: "0x10000"}) == 0x10000
assert g.table_offset({VAR: "65536"}) == 0x10000, "decimal and hexadecimal agree, like esp-config"
for bad in ("junk", "0x10001", "0x4000", "0x1c000", "-1", "0x20000"):
    assert refuses(g.table_offset, {VAR: bad}), bad
print("Table offset setting passed")

# --- limits ---------------------------------------------------------------------------------
wide = g.limits("esp32s3", 0x10000)
assert wide["hard"] == 61440, "a 4 KiB signature sector stays free in front of the table"
assert (wide["unsigned"], wide["verified"]) == (32768, 40960)
assert g.limits("esp32s3", 0x8000)["verified"] is None, "does not fit the historical table: report only"
assert g.limits("esp32c3", 0x8000)["hard"] == 32768
assert refuses(g.limits, "esp32c3", 0x10000), "the C3 keeps the historical geometry in this phase"
for key, lim in g.LIMITS.items():
    assert lim["unsigned"] <= lim["hard"], key
    assert lim["verified"] is None or lim["verified"] <= lim["hard"], key
print("Limits passed")

# --- the versioned partition files -------------------------------------------------------------
for name, offset in (("partitions-s3-16mb-legacy.csv", 0x8000), ("partitions-s3-16mb-wide.csv", 0x10000)):
    path = SYSTEM / name
    assert g.check_csv(path, offset=offset) == [], name
    other = 0x10000 if offset == 0x8000 else 0x8000
    assert any("declares a table" in e for e in g.check_csv(path, offset=other)), "wrong build geometry is refused"
for path in (SYSTEM / "partitions.csv", ROOT / "examples/firmware/ota-supervised/partitions.csv",
             ROOT / "examples/config-manager/partitions.csv"):
    declared, _, _ = g.parse_csv(path)
    assert declared == 0x8000, path
print("Versioned partition files passed")

with tempfile.TemporaryDirectory() as directory:
    def variant(edit, offset="0x10000"):
        text = (SYSTEM / "partitions-s3-16mb-wide.csv").read_text()
        path = Path(directory) / "t.csv"
        path.write_text(edit(text))
        return g.check_csv(path, offset=int(offset, 16))

    assert variant(lambda t: t.replace("otadata, data, ota, 0x11000", "otadata, data, ota, 0xd000")), "before the table sector"
    assert variant(lambda t: t.replace("ota_1, app, ota_1, 0x1d0000", "ota_1, app, ota_1, 0x1c0000")), "overlap"
    assert variant(lambda t: t.replace("ota_1, app, ota_1, 0x1d0000", "ota_1, app, ota_1, 0x1d1000")), "app not 64 KiB aligned"
    assert variant(lambda t: t.replace("config, data, 0x40, 0x380000, 0x80000", "config, data, 0x40, 0x380000, 0xc90000")), "beyond the flash"
    assert variant(lambda t: t.replace("0x1b0000,\nota_1", "0x1a0000,\nota_1")), "slots of different sizes"
    assert variant(lambda t: t.replace("ota_meta, data, 0x41, 0x13000", "ota_meta, data, 0x41, 0x12000")), "otadata/ota_meta overlap"
    assert refuses(variant, lambda t: t.replace("# table-offset: 0x10000\n", ""))
print("Partition checks passed")

# --- images ---------------------------------------------------------------------------------
def stamped(offset, size):
    return (b"\x00" * 100 + f"IOBEWI-GEOMETRY table={offset:#010x}\0".encode()).ljust(size, b"\xff")

with tempfile.TemporaryDirectory() as directory:
    image = Path(directory) / "boot.bin"
    def run(chip, profile, offset, data):
        image.write_bytes(data)
        return g.check_image(chip, image, profile, offset)[1]
    assert run("esp32s3", "unsigned", 0x10000, stamped(0x10000, 30016)) == []
    assert run("esp32s3", "verified", 0x10000, stamped(0x10000, 34352)) == []
    assert run("esp32c3", "verified", 0x8000, stamped(0x8000, 26912)) == []
    assert run("esp32s3", "verified", 0x8000, stamped(0x8000, 34352)), "over the historical table"
    assert run("esp32s3", "verified", 0x10000, stamped(0x10000, 40961)), "over the drift budget"
    assert run("esp32s3", "unsigned", 0x10000, stamped(0x10000, 61441)), "over the hard limit"
    assert run("esp32s3", "unsigned", 0x10000, stamped(0x10000, 61440)) != [], "unsigned budget is below the hard limit"
    assert run("esp32s3", "unsigned", 0x10000, stamped(0x8000, 30016)), "stamped for another geometry"
    assert run("esp32s3", "unsigned", 0x10000, b"\xff" * 30016), "not stamped at all"
    assert run("esp32s3", "unsigned", 0x10000, b""), "empty"
print("Image checks passed")

# --- RAM placement on a synthetic ELF ------------------------------------------------------------
def elf(loads, symbols, entry):
    names = b"\0" + b"".join(n.encode() + b"\0" for n in symbols)
    strtab_offsets, at = {}, 1
    for n in symbols:
        strtab_offsets[n] = at
        at += len(n) + 1
    symtab = b"\0" * 16 + b"".join(struct.pack("<IIIBBH", strtab_offsets[n], v, 0, 0, 0, 1) for n, v in symbols.items())
    phoff, shoff = 52, 52 + 32 * len(loads)
    body_at = shoff + 40 * 3
    header = b"\x7fELF" + bytes([1, 1, 1, 0]) + b"\0" * 8 + struct.pack("<HHIIIIIHHHHHH", 2, 94, 1, entry, phoff, shoff, 0, 52, 32, len(loads), 40, 3, 0)
    phdrs = b"".join(struct.pack("<IIIIIIII", 1, 0, v, v, 0, m, 5, 4) for v, m in loads)
    sections = struct.pack("<10I", *([0] * 10))
    sections += struct.pack("<10I", 0, 2, 0, 0, body_at, len(symtab), 2, 0, 4, 16)
    sections += struct.pack("<10I", 0, 3, 0, 0, body_at + len(symtab), len(names), 0, 0, 1, 0)
    return header + phdrs + sections + symtab + names

with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "boot.elf"
    s3 = g.RAM["esp32s3"]
    good = [(0x403C4000, 0x400), (0x403C4400, 0x7000), (0x3FCE4000, 0x8000)]
    syms = {"_stack_start": 0x3FCEC000, "_stack_end": 0x3FCE4E00}
    path.write_bytes(elf(good, syms, 0x403C4400))
    errors, report = g.check_ram("esp32s3", path)
    assert errors == [], errors
    assert report["iram_used"] == 0x400 + 0x7000 and report["stack"] == 0x7200
    path.write_bytes(elf(good + [(0x403BF000, 0x100)], syms, 0x403C4400))
    assert g.check_ram("esp32s3", path)[0], "code below the window"
    path.write_bytes(elf([(0x403C4000, 0x11000), (0x3FCE4000, 0x8000)], syms, 0x403C4400))
    assert g.check_ram("esp32s3", path)[0], "code past the window"
    path.write_bytes(elf(good, {"_stack_start": 0x3FCEC000, "_stack_end": 0x3FCEB000}, 0x403C4400))
    assert any("stack" in e for e in g.check_ram("esp32s3", path)[0]), "stack under 8 KiB"
    path.write_bytes(elf(good, syms, 0x40000000))
    assert any("entry" in e for e in g.check_ram("esp32s3", path)[0]), "entry outside the window"
for chip, w in g.RAM.items():
    # The windows are what the linker scripts say, and are disjoint on the SRAM.
    assert w["iram"][0] - w["alias"] >= w["boot_window"][0], chip
    assert w["iram"][1] - w["alias"] <= w["dram"][0], chip
print("RAM placement checks passed")
