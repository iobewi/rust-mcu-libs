#!/usr/bin/env python3
"""Flash footprint of an ELF32 little-endian firmware: sum of allocated sections that occupy file bytes.

Usage: elf_size.py ELF [ELF ...]  ->  one line per file: <bytes> <path>
With --compare BASE SIGNED --max-growth BYTES, fails when SIGNED exceeds BASE by more than BYTES.
"""
import argparse
import struct
import sys

SHF_ALLOC = 0x2
SHT_NOBITS = 8


def footprint(path):
    data = open(path, "rb").read()
    if data[:4] != b"\x7fELF" or data[4] != 1 or data[5] != 1:
        raise ValueError(f"{path}: ELF32 little-endian expected")
    shoff, = struct.unpack_from("<I", data, 0x20)
    shentsize, shnum = struct.unpack_from("<HH", data, 0x2E)
    total = 0
    for i in range(shnum):
        _, kind, flags, _, _, size = struct.unpack_from("<IIIIII", data, shoff + i * shentsize)
        if flags & SHF_ALLOC and kind != SHT_NOBITS:
            total += size
    return total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("elf", nargs="*")
    parser.add_argument("--compare", nargs=2, metavar=("BASE", "SIGNED"))
    parser.add_argument("--max-growth", type=int)
    args = parser.parse_args()
    for path in args.elf:
        print(footprint(path), path)
    if args.compare:
        base, signed = (footprint(p) for p in args.compare)
        growth = signed - base
        print(f"unsigned {base} B, signed {signed} B, growth {growth} B ({growth / base:.1%})")
        if args.max_growth is not None and growth > args.max_growth:
            sys.exit(f"signed profile grows by {growth} B, more than the {args.max_growth} B budget")


if __name__ == "__main__":
    main()
