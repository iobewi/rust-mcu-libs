#!/usr/bin/env python3
"""Build a real S3 native component ELF and pack it with the loader's IWNI gate."""
import argparse
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--variant", choices=("a", "b"), default="a")
    parser.add_argument("--fault", choices=("panic", "ignore-stop", "log-flood"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    os.chdir(root)
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", root / "target")).resolve()
    features = ["esp32s3"]
    if args.variant == "b":
        features.append("variant-b")
    if args.fault:
        features.append("fault-" + args.fault)
    subprocess.run(["cargo", "+esp", "build", "-Z", "build-std=core,alloc",
                    "--target", "xtensa-esp32s3-none-elf", "-p", "example-supervised-component",
                    "--features", ",".join(features), "--release", "--bins"], check=True)
    output = target_dir / "ota-supervised" / "esp32s3"
    output.mkdir(parents=True, exist_ok=True)
    name = "native-" + args.variant + ("-" + args.fault if args.fault else "") + ".iwni"
    subprocess.run(["cargo", "run", "-p", "iobewi-workload-pack", "--",
                    str(target_dir / "xtensa-esp32s3-none-elf/release/supervised-component"),
                    "-o", str(output / name)], check=True)
    print(output / name)


if __name__ == "__main__":
    main()
