#!/usr/bin/env python3
"""Flash geometry of the ESP32 reference firmware: one setting, checked everywhere (issue #91).

The partition table offset is the one value the second stage, the application and the generated
table must share. Its source is ``ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET`` (default
0x8000), the variable ``esp-bootloader-esp-idf`` already reads for the application. Everything
here derives from it and refuses what does not agree: a CSV declaring another offset, a
bootloader that ends after the table, a binary stamped with another geometry, a code window that
does not fit its RAM region.

  esp_geometry.py offset
  esp_geometry.py limits CHIP
  esp_geometry.py csv FILE [--flash-size N]
  esp_geometry.py image CHIP BOOTLOADER.bin [--elf BOOTLOADER.elf] [--profile unsigned|verified]
  esp_geometry.py stamp BINARY            (prints the geometry stamped in an app or a bootloader)
"""
import argparse
import os
from pathlib import Path
import re
import struct
import sys

VARIABLE = "ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET"
HISTORICAL = 0x8000
SECTOR = 0x1000
OTA_0_MIN = 0x20000                      # every reference layout starts ota_0 there
MAX_OFFSET = OTA_0_MIN - 0x5000          # table + otadata (8 KiB) + ota_meta (8 KiB) in front of it
STAMP = re.compile(rb"IOBEWI-GEOMETRY table=0x([0-9a-f]{8})\x00")

# Flash budgets of the second stage, in bytes, per (chip, table offset).
#   hard      nothing may exceed it: the image must end before the table (with a 4 KiB signature
#             sector kept free in the wide geometry, for a future Secure Boot V2 profile);
#   unsigned  drift budget of the default profile: blocking, set above the measured size;
#   verified  drift budget of the verified profile; None while it does not fit (report only).
LIMITS = {
    ("esp32c3", 0x8000): {"hard": 0x8000, "unsigned": 23744, "verified": 0x8000 - 512},
    ("esp32s3", 0x8000): {"hard": 0x8000, "unsigned": 30720, "verified": None},
    ("esp32s3", 0x10000): {"hard": 0x10000 - SECTOR, "unsigned": 32768, "verified": 40960},
}

# RAM the second stage is linked into (see arch/esp32/firmware/boot/linker): the instruction
# window ("iram") and the data window ("dram") must not share physical bytes, and both stay inside
# the boot window applications are never loaded into. "alias" maps an instruction-bus address to
# the data-bus address of the same SRAM byte.
RAM = {
    "esp32s3": {"iram": (0x403C4000, 0x403D4000), "dram": (0x3FCE4000, 0x3FCEC000),
                "alias": 0x6F0000, "boot_window": (0x3FCC8000, 0x3FD00000)},
    "esp32c3": {"iram": (0x403CB000, 0x403D4000), "dram": (0x3FCD4000, 0x3FCDC000),
                "alias": 0x700000, "boot_window": (0x3FCCB000, 0x3FCE0000)},
}
MIN_STACK = 0x2000


class GeometryError(ValueError):
    pass


def parse_int(text):
    text = text.strip()
    return int(text, 16) if text.lower().startswith("0x") else int(text)


def table_offset(environ=None):
    environ = os.environ if environ is None else environ
    text = environ.get(VARIABLE)
    if text is None or not text.strip():
        return HISTORICAL
    try:
        offset = parse_int(text)
    except ValueError:
        raise GeometryError(f"{VARIABLE}={text!r} is not an integer") from None
    if offset % SECTOR or not HISTORICAL <= offset <= MAX_OFFSET:
        raise GeometryError(
            f"{VARIABLE}={offset:#x}: the table sits on a 4 KiB boundary between {HISTORICAL:#x} and {MAX_OFFSET:#x}")
    return offset


def limits(chip, offset=None):
    offset = table_offset() if offset is None else offset
    try:
        return LIMITS[(chip, offset)]
    except KeyError:
        supported = ", ".join(f"{c}@{o:#x}" for c, o in LIMITS)
        raise GeometryError(f"{chip} is not supported with the table at {offset:#x} (supported: {supported})") from None


# ---- partition files --------------------------------------------------------------------------

def parse_csv(path):
    text = Path(path).read_text()
    declared = re.search(r"^#\s*table-offset:\s*(\S+)", text, re.M)
    flash = re.search(r"^#\s*flash-size:\s*(\S+)", text, re.M)
    if not declared:
        raise GeometryError(f"{path}: no '# table-offset:' header, the geometry would be implicit")
    rows = []
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = [f.strip() for f in line.split(",")]
        if len(fields) < 5:
            raise GeometryError(f"{path}: malformed row {line!r}")
        rows.append({"name": fields[0], "type": fields[1], "subtype": fields[2],
                     "offset": parse_int(fields[3]), "size": parse_int(fields[4])})
    return parse_int(declared.group(1)), (parse_int(flash.group(1)) if flash else None), rows


def check_csv(path, flash_size=None, offset=None):
    """Errors found in a partition file, as strings (empty when it is consistent)."""
    offset = table_offset() if offset is None else offset
    errors = []
    declared, declared_flash, rows = parse_csv(path)
    if declared != offset:
        errors.append(f"declares a table at {declared:#x} but the build is configured for {offset:#x} ({VARIABLE})")
    flash_size = flash_size or declared_flash
    first = declared + SECTOR
    ordered = sorted(rows, key=lambda r: r["offset"])
    for r in rows:
        if r["offset"] < first:
            errors.append(f"{r['name']} starts at {r['offset']:#x}, before the first usable offset {first:#x}")
        if r["offset"] % SECTOR or r["size"] % SECTOR:
            errors.append(f"{r['name']}: offset and size are multiples of 4 KiB")
        if r["type"] == "app" and r["offset"] % 0x10000:
            errors.append(f"{r['name']}: an application starts on a 64 KiB boundary")
        if flash_size and r["offset"] + r["size"] > flash_size:
            errors.append(f"{r['name']} ends at {r['offset'] + r['size']:#x}, beyond the flash ({flash_size:#x})")
    for a, b in zip(ordered, ordered[1:]):
        if a["offset"] + a["size"] > b["offset"]:
            errors.append(f"{a['name']} overlaps {b['name']}")
    by = {r["name"]: r for r in rows}
    for needed in ("otadata", "ota_0", "ota_1"):
        if needed not in by:
            errors.append(f"missing {needed}")
    if not errors:
        if by["otadata"]["size"] < 0x2000:
            errors.append("otadata is at least 0x2000 bytes")
        if by["ota_0"]["size"] != by["ota_1"]["size"]:
            errors.append("ota_0 and ota_1 differ in size")
        if by["ota_0"]["size"] <= SECTOR:
            errors.append("a slot needs room for an image besides its proof sector")
    return errors


# ---- images and ELF files ---------------------------------------------------------------------

def stamp_of(data):
    found = {int(m.group(1), 16) for m in STAMP.finditer(data)}
    return found


def check_image(chip, path, profile="unsigned", offset=None):
    """Flash-size and geometry checks of a built second stage. Returns (size, errors, notes)."""
    offset = table_offset() if offset is None else offset
    lim = limits(chip, offset)
    data = Path(path).read_bytes()
    errors, notes = [], []
    size = len(data)
    if size == 0:
        errors.append("empty image")
    if size > lim["hard"]:
        errors.append(f"{size} bytes exceed the hard limit {lim['hard']} (table at {offset:#x})")
    budget = lim.get(profile)
    if budget is None:
        notes.append(f"{profile} profile has no blocking budget in this geometry (report only)")
    elif size > budget:
        errors.append(f"{size} bytes exceed the {profile} budget {budget}")
    stamps = stamp_of(data)
    if stamps != {offset}:
        errors.append(f"geometry stamp {sorted(hex(s) for s in stamps) or 'absent'} does not match the table offset {offset:#x}")
    notes.append(f"{size} bytes; hard {lim['hard']} (margin {lim['hard'] - size}); {profile} budget {budget}")
    return size, errors, notes


def read_elf(path):
    data = Path(path).read_bytes()
    if data[:4] != b"\x7fELF" or data[4] != 1 or data[5] != 1:
        raise GeometryError(f"{path}: not a little-endian ELF32")
    (e_entry, e_phoff, e_shoff) = struct.unpack_from("<III", data, 24)
    (e_phentsize, e_phnum, e_shentsize, e_shnum, e_shstrndx) = struct.unpack_from("<HHHHH", data, 42)
    loads = []
    for i in range(e_phnum):
        p_type, p_offset, p_vaddr, _, p_filesz, p_memsz, p_flags, _ = struct.unpack_from("<IIIIIIII", data, e_phoff + i * e_phentsize)
        if p_type == 1:
            loads.append({"vaddr": p_vaddr, "filesz": p_filesz, "memsz": p_memsz, "flags": p_flags})
    sections = []
    for i in range(e_shnum):
        sections.append(struct.unpack_from("<IIIIIIIIII", data, e_shoff + i * e_shentsize))
    symbols = {}
    for sh in sections:
        if sh[1] == 2:  # SHT_SYMTAB
            strtab = sections[sh[6]]
            names = data[strtab[4]:strtab[4] + strtab[5]]
            for j in range(sh[5] // 16):
                st_name, st_value, *_ = struct.unpack_from("<IIIBBH", data, sh[4] + j * 16)
                end = names.index(b"\0", st_name)
                symbols[names[st_name:end].decode()] = st_value
    symbols["__entry"] = e_entry
    return loads, symbols


def check_ram(chip, elf):
    """Placement of the linked second stage: every loaded byte inside its window, instruction and
    data windows on distinct SRAM, both inside the boot window, a usable stack."""
    w = RAM[chip]
    loads, symbols = read_elf(elf)
    errors, report = [], {}
    iram_end = w["iram"][0]
    dram_extent = 0
    for seg in loads:
        lo, hi = seg["vaddr"], seg["vaddr"] + seg["memsz"]
        if w["iram"][0] <= lo and hi <= w["iram"][1]:
            iram_end = max(iram_end, hi)
        elif w["dram"][0] <= lo and hi <= w["dram"][1]:
            dram_extent = max(dram_extent, hi - w["dram"][0])
        elif 0x50000000 <= lo < 0x60110000:
            pass  # RTC memory
        else:
            errors.append(f"segment {lo:#x}..{hi:#x} is in no window of the second stage")
    bw_lo, bw_hi = w["boot_window"]
    iram_data_bus = (w["iram"][0] - w["alias"], w["iram"][1] - w["alias"])
    if not (bw_lo <= iram_data_bus[0] and iram_data_bus[1] <= bw_hi):
        errors.append(f"code window {iram_data_bus[0]:#x}..{iram_data_bus[1]:#x} (data bus) leaves the boot window {bw_lo:#x}..{bw_hi:#x}")
    if not (w["dram"][0] >= bw_lo and w["dram"][1] <= bw_hi):
        errors.append("data window leaves the boot window")
    if iram_data_bus[0] < w["dram"][1] and w["dram"][0] < iram_data_bus[1]:
        errors.append("code and data windows share SRAM")
    stack = symbols.get("_stack_start", 0) - symbols.get("_stack_end", 0)
    if not (w["dram"][0] <= symbols.get("_stack_end", 0) and symbols.get("_stack_start", 0) <= w["dram"][1]):
        errors.append("stack outside the data window")
    if stack < MIN_STACK:
        errors.append(f"stack of {stack} bytes (needs {MIN_STACK})")
    entry_ok = w["iram"][0] <= symbols["__entry"] < w["iram"][1]
    if not entry_ok:
        errors.append("entry point outside the code window")
    report = {"iram_used": iram_end - w["iram"][0], "iram_size": w["iram"][1] - w["iram"][0],
              "dram_static": symbols.get("_stack_end", 0) - w["dram"][0], "dram_size": w["dram"][1] - w["dram"][0],
              "stack": stack}
    return errors, report


# ---- command line -----------------------------------------------------------------------------

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("offset")
    p = sub.add_parser("limits"); p.add_argument("chip")
    p = sub.add_parser("csv"); p.add_argument("file"); p.add_argument("--flash-size", type=parse_int)
    p = sub.add_parser("image"); p.add_argument("chip"); p.add_argument("bin"); p.add_argument("--elf")
    p.add_argument("--profile", choices=("unsigned", "verified"), default="unsigned")
    p.add_argument("--report-only", action="store_true")
    p = sub.add_parser("stamp"); p.add_argument("binary")
    args = parser.parse_args(argv)
    try:
        if args.command == "offset":
            print(hex(table_offset()))
        elif args.command == "limits":
            print(limits(args.chip))
        elif args.command == "csv":
            errors = check_csv(args.file, args.flash_size)
            for e in errors:
                print(f"{args.file}: {e}", file=sys.stderr)
            return 1 if errors else 0
        elif args.command == "stamp":
            for s in sorted(stamp_of(Path(args.binary).read_bytes())):
                print(hex(s))
        elif args.command == "image":
            size, errors, notes = check_image(args.chip, args.bin, args.profile)
            for n in notes:
                print(n)
            if args.elf:
                ram_errors, r = check_ram(args.chip, args.elf)
                errors += ram_errors
                print(f"IRAM {r['iram_used']}/{r['iram_size']} bytes, DRAM static {r['dram_static']}/{r['dram_size']}, stack {r['stack']}")
            for e in errors:
                print(f"error: {e}", file=sys.stderr)
            return 0 if args.report_only else (1 if errors else 0)
    except GeometryError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
