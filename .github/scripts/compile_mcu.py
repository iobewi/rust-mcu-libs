#!/usr/bin/env python3
"""Execute declarative MCU profiles, including actual boot image size gates."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

LIMIT = 0x8000  # historical table offset, boot image flashed at zero
# Blocking budget of the default (unsigned) second stage, per chip. Measured 22 928 (C3) and
# 29 920 (S3) bytes when this gate was set (#91); the budget leaves about 800 bytes of growth.
# Raising it eats the room the verified profile needs: change it only with a measurement.
BUDGET = {"esp32c3": 23744, "esp32s3": 30720}

def check_size(image, limit=LIMIT):
    size = Path(image).stat().st_size
    if size > limit or size == 0:
        raise ValueError(f"boot image {size} bytes does not fit in {limit} bytes")
    print(f"Boot image: {size} bytes; margin before the limit {limit}: {limit-size} bytes", flush=True)
    return size

def compile_selected(packages, chip):
    compiler, target = {
        "esp32c3": ("nightly", "riscv32imc-unknown-none-elf"),
        "esp32s3": ("esp", "xtensa-esp32s3-none-elf"),
    }[chip]
    if any(p["profile"] == "esp32-bootloader" for p in packages):
        subprocess.run([sys.executable, "-m", "pip", "install", "esptool==5.5.0"], check=True)
    for item in packages:
        boot = item["profile"] == "esp32-bootloader"
        linked = boot or item["profile"] in ("esp32-linked-bins", "host-esp32-linked-bins", "host-esp32s3-linked-bins")
        selected_features = ",".join(f for f in (chip,item.get("features", "")) if f)
        cmd = ["cargo", "+"+compiler, "build" if linked else "check", "-Z", "build-std=core,alloc",
               "--target", target, "-p", item["package"], "--features", selected_features, "--"+item["targets"]]
        if boot:
            cmd += ["--profile", "bootloader"]
        elif linked:
            cmd += ["--release"]
        print("::group::"+item["package"], flush=True)
        try:
            subprocess.run(cmd, check=True)
            if boot:
                # Select binaries from Cargo metadata, not a hardcoded crate name.
                metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], text=True))
                package = next(p for p in metadata["packages"] if p["name"] == item["package"])
                binaries = [t for t in package["targets"] if "bin" in t["kind"]]
                if not binaries:
                    raise ValueError("bootloader profile requires an executable")
                for binary in binaries:
                    elf = Path(metadata["target_directory"])/target/"bootloader"/binary["name"]
                    image = elf.with_suffix(".bin")
                    # elf2image defaults to QIO, which the ESP32-S3 ROM cannot load after the
                    # first segment on the qualification board (ets_loader.c 78, reset loop).
                    mode = ["--flash-mode", "dio"] if chip == "esp32s3" else []
                    subprocess.run([sys.executable, "-m", "esptool", "--chip", chip,
                                    "elf2image", *mode, "--flash-size", "16MB", "--output", str(image), str(elf)], check=True)
                    check_size(image)
                    check_size(image, BUDGET[chip])
                    if chip == "esp32s3" and image.read_bytes()[2] != 2:
                        raise ValueError("ESP32-S3 boot image must use flash mode DIO (header byte 2 == 2)")
                    subprocess.run([sys.executable,"-m","esptool","--chip",chip,"image-info",str(image)],check=True)
        finally:
            print("::endgroup::", flush=True)

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--chip", required=True, choices=("esp32c3", "esp32s3"))
    args = parser.parse_args()
    compile_selected(json.loads(os.environ["PACKAGES"]), args.chip)
