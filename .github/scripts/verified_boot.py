#!/usr/bin/env python3
"""Build the verified-boot second stage (issue #91) and gate its size.

Needs the toolchain of the chip (nightly for esp32c3, esp for esp32s3) and esptool. The pinned
key is a public test fixture (the SHA-256 of a throwaway RSA key encoding): the bootloader
trusts it, nothing is signed here.
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys

import esp_geometry
from compile_mcu import check_boot_image
PIN = "4f18ce4fa264053a0866f4faa6fb5028b57029b707f74160fd49d2adc6b79a63"
CHIPS = {
    "esp32c3": ("nightly", "riscv32imc-unknown-none-elf", []),
    "esp32s3": ("esp", "xtensa-esp32s3-none-elf", ["--flash-mode", "dio"]),
}

def build(chip, diag=False, sha="rom"):
    toolchain, target, mode = CHIPS[chip]
    features = f"{chip},verified-boot" + (f",sha-{sha}" if sha != "rom" else "") + (",diag-panic" if diag else "")
    env = dict(os.environ, IOBEWI_BOOT_PINNED_KEYS=PIN, IOBEWI_BOOT_PRODUCT="iobewi-ota-sys", IOBEWI_BOOT_VARIANTS="0,1")
    subprocess.run(["cargo", "+" + toolchain, "build", "-Z", "build-std=core,alloc", "--target", target, "--profile", "bootloader",
                    "-p", "iobewi-esp-bootloader", "--features", features, "--bins"], check=True, env=env)
    elf = Path(os.environ.get("CARGO_TARGET_DIR", "target")) / target / "bootloader" / "iobewi-esp-bootloader"
    image = elf.with_suffix(".bin")
    subprocess.run([sys.executable, "-m", "esptool", "--chip", chip, "elf2image", *mode, "--flash-size", "16MB",
                    "--output", str(image), str(elf)], check=True)
    return image, elf

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chip", choices=CHIPS, required=True)
    parser.add_argument("--diag", action="store_true",
                        help="the diagnostic build (diag-panic): where a panic or an exception happened; same limits")
    parser.add_argument("--sha", choices=("rom", "hal", "soft"), default="rom",
                        help="SHA-256 backend: the ROM's (default), the esp-hal driver or the software one; same limits")
    parser.add_argument("--report-only", action="store_true",
                        help="print sizes against the limits without failing (known overflow being worked on)")
    args = parser.parse_args()
    offset = esp_geometry.table_offset()
    lim = esp_geometry.limits(args.chip, offset)
    image, elf = build(args.chip, args.diag, args.sha)
    size = image.stat().st_size
    verdict = (f"{args.chip} verified second stage{f' (SHA-256: {args.sha})' if args.sha != 'rom' else ''}{' (diagnostic build)' if args.diag else ''}, table at {offset:#x}: {size} bytes, "
               f"hard limit {lim['hard']} (margin {lim['hard'] - size}), drift budget {lim['verified']}")
    print(verdict, flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as out:
            out.write("### " + verdict + "\n")
    try:
        check_boot_image(args.chip, image, elf, "verified")
    except ValueError as error:
        print(f"{'report' if args.report_only else 'error'}: {error}", flush=True)
        if not args.report_only:
            return 1
    return 0

if __name__ == "__main__":
    sys.exit(main())
