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

from compile_mcu import LIMIT, check_size

# Same margin policy as the unsigned stage: keep at least 512 bytes below the table.
VERIFIED_LIMIT = LIMIT - 512
PIN = "4f18ce4fa264053a0866f4faa6fb5028b57029b707f74160fd49d2adc6b79a63"
CHIPS = {
    "esp32c3": ("nightly", "riscv32imc-unknown-none-elf", []),
    "esp32s3": ("esp", "xtensa-esp32s3-none-elf", ["--flash-mode", "dio"]),
}

def build(chip):
    toolchain, target, mode = CHIPS[chip]
    env = dict(os.environ, IOBEWI_BOOT_PINNED_KEYS=PIN, IOBEWI_BOOT_PRODUCT="iobewi-ota-sys", IOBEWI_BOOT_VARIANTS="0,1")
    subprocess.run(["cargo", "+" + toolchain, "build", "-Z", "build-std=core,alloc", "--target", target, "--profile", "bootloader",
                    "-p", "iobewi-esp-bootloader", "--features", f"{chip},verified-boot", "--bins"], check=True, env=env)
    elf = Path("target") / target / "bootloader" / "iobewi-esp-bootloader"
    image = elf.with_suffix(".bin")
    subprocess.run([sys.executable, "-m", "esptool", "--chip", chip, "elf2image", *mode, "--flash-size", "16MB",
                    "--output", str(image), str(elf)], check=True)
    return image

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chip", choices=CHIPS, required=True)
    parser.add_argument("--report-only", action="store_true",
                        help="print the size against the limit without failing (known overflow being worked on)")
    args = parser.parse_args()
    image = build(args.chip)
    size = image.stat().st_size
    verdict = f"{args.chip} verified second stage: {size} bytes, limit {VERIFIED_LIMIT}, margin {VERIFIED_LIMIT - size}"
    print(verdict, flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as out:
            out.write("### " + verdict + "\n")
    if args.report_only:
        return 0
    check_size(image, VERIFIED_LIMIT)
    return 0

if __name__ == "__main__":
    sys.exit(main())
