#!/usr/bin/env python3
"""Execute declarative MCU profiles, including actual boot image size gates."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

LIMIT = 0x8000  # historical table offset, boot image flashed at zero

def check_size(image):
    size = Path(image).stat().st_size
    if size > LIMIT or size == 0:
        raise ValueError(f"boot image {size} bytes does not fit in {LIMIT} bytes")
    print(f"Boot image: {size} bytes; margin before partition table: {LIMIT-size} bytes", flush=True)
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
        linked = boot or item["profile"] == "esp32-linked-bins"
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
                    subprocess.run([sys.executable, "-m", "esptool", "--chip", chip,
                                    "elf2image", "--flash-size", "16MB", "--output", str(image), str(elf)], check=True)
                    check_size(image)
                    subprocess.run([sys.executable,"-m","esptool","--chip",chip,"image-info",str(image)],check=True)
        finally:
            print("::endgroup::", flush=True)

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--chip", required=True, choices=("esp32c3", "esp32s3"))
    args = parser.parse_args()
    compile_selected(json.loads(os.environ["PACKAGES"]), args.chip)
