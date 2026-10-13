#!/usr/bin/env python3
"""Build/package the reference scenario without touching any device."""
import argparse
import os
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chip", required=True, choices=("esp32c3", "esp32s3"))
    parser.add_argument("--partition-tool", required=True, type=Path,
                        help="official ESP-IDF components/partition_table/gen_esp32part.py")
    parser.add_argument("--fault", choices=("reject-self-check", "hang-self-check"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    os.chdir(root)
    compiler, target = {"esp32c3": ("nightly", "riscv32imc-unknown-none-elf"),
                        "esp32s3": ("esp", "xtensa-esp32s3-none-elf")}[args.chip]
    output = root / "target" / "ota-system" / args.chip
    output.mkdir(parents=True, exist_ok=True)
    features = args.chip + ("," + args.fault if args.fault else "")
    def run(*command):
        subprocess.run(command, check=True)
    run("cargo", "+" + compiler, "build", "-Z", "build-std=core,alloc", "--target", target,
        "-p", "iobewi-esp-bootloader", "--features", args.chip, "--profile", "bootloader", "--bins")
    run("cargo", "+" + compiler, "build", "-Z", "build-std=core,alloc", "--target", target,
        "-p", "example-firmware-ota-system", "--features", features, "--release", "--bins")
    for name, profile, binary in (("bootloader", "bootloader", "iobewi-bootloader-esp"),
                                  ("app", "release", "firmware-ota-system-esp32")):
        # Resolve the binary's actual name from Cargo metadata, for the bootloader.
        if name == "bootloader":
            import json
            metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"]))
            package = next(p for p in metadata["packages"] if p["name"] == "iobewi-esp-bootloader")
            binary = next(t["name"] for t in package["targets"] if "bin" in t["kind"])
        # DIO, as ESP-IDF does: the ROM applies the boot header's mode before loading the
        # second stage, and QIO reads garbage on flash whose QE bit is not already set.
        run(sys.executable, "-m", "esptool", "--chip", args.chip, "elf2image", "--flash-mode", "dio",
            "--flash-size", "4MB", "--output", str(output / (name + ".bin")),
            str(root / "target" / target / profile / binary))
    if not 0 < (output / "bootloader.bin").stat().st_size <= 0x8000:
        raise RuntimeError("bootloader exceeds the partition-table boundary")
    for name in ("bootloader", "app"):
        if (output / (name + ".bin")).read_bytes()[2] != 0x02:
            raise RuntimeError(name + " image header is not DIO")
    for offset in ("20000", "1a0000"):
        run("cargo", "run", "-p", "iobewi-esp-boot", "--example", "validate_image", "--features", args.chip,
            "--", str(output / "app.bin"), offset, "180000")
    run("cargo", "run", "-p", "iobewi-esp-boot", "--example", "factory_otadata", "--", str(output / "otadata.bin"), "0")
    run(sys.executable, str(args.partition_tool.resolve()),
        "examples/firmware/ota-system/partitions.csv", str(output / "partitions.bin"))
    print("Prepared:", output)
    print("Initial flash only: bootloader @0, partitions @0x8000, otadata @0xd000, app @0x20000")


if __name__ == "__main__":
    main()
