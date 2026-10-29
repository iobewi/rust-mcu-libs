#!/usr/bin/env python3
"""Build/package the reference scenario without touching any device."""
import argparse
import os
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=("ota-system", "ota-supervised"), default="ota-system")
    parser.add_argument("--chip", required=True, choices=("esp32c3", "esp32s3"))
    parser.add_argument("--partition-tool", required=True, type=Path,
                        help="official ESP-IDF components/partition_table/gen_esp32part.py")
    parser.add_argument("--fault", choices=("reject-self-check", "hang-self-check"))
    parser.add_argument("--flash-size", choices=("4MB", "16MB"), default="4MB",
                        help="16MB selects the versioned qualification-board table (ESP32-S3, ota-system) "
                             "that matches the configured table offset")
    args = parser.parse_args()
    if args.scenario == "ota-supervised" and args.chip != "esp32s3":
        parser.error("native ota-supervised requires ESP32-S3")
    root = Path(__file__).resolve().parents[3]
    sys.path.insert(0, str(root / ".github" / "scripts"))
    import esp_geometry
    # One geometry for everything this script builds: the second stage, the application (which
    # takes the same variable through esp-bootloader-esp-idf), the table and the checks below.
    try:
        offset = esp_geometry.table_offset()
        esp_geometry.limits(args.chip, offset)
    except esp_geometry.GeometryError as error:
        parser.error(str(error))
    os.environ[esp_geometry.VARIABLE] = hex(offset)
    os.chdir(root)
    if args.flash_size == "16MB":
        if args.chip != "esp32s3" or args.scenario != "ota-system":
            parser.error("the 16 MiB board table exists for ota-system on ESP32-S3")
        kind = "wide" if offset != esp_geometry.HISTORICAL else "legacy"
        csv = Path("examples/firmware/ota-system") / f"partitions-s3-16mb-{kind}.csv"
    else:
        csv = Path("examples/firmware") / args.scenario / "partitions.csv"
    errors = esp_geometry.check_csv(csv)
    if errors:
        parser.error(f"{csv}: " + "; ".join(errors))
    _, _, rows = esp_geometry.parse_csv(csv)
    by = {row["name"]: row for row in rows}
    compiler, target = {"esp32c3": ("nightly", "riscv32imc-unknown-none-elf"),
                        "esp32s3": ("esp", "xtensa-esp32s3-none-elf")}[args.chip]
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", str(root / "target"))).resolve()
    output = target_dir / args.scenario / args.chip
    output.mkdir(parents=True, exist_ok=True)
    features = args.chip + ("," + args.fault if args.fault else "")
    def run(*command):
        subprocess.run(command, check=True)
    run("cargo", "+" + compiler, "build", "-Z", "build-std=core,alloc", "--target", target,
        "-p", "iobewi-esp-bootloader", "--features", args.chip, "--profile", "bootloader", "--bins")
    run("cargo", "+" + compiler, "build", "-Z", "build-std=core,alloc", "--target", target,
        "-p", "example-firmware-" + args.scenario, "--features", features, "--release", "--bins")
    boot_elf = None
    for name, profile, binary in (("bootloader", "bootloader", "iobewi-bootloader-esp"),
                                  ("app", "release", "firmware-" + args.scenario + "-esp32")):
        # Resolve the binary's actual name from Cargo metadata, for the bootloader.
        if name == "bootloader":
            import json
            metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"]))
            package = next(p for p in metadata["packages"] if p["name"] == "iobewi-esp-bootloader")
            binary = next(t["name"] for t in package["targets"] if "bin" in t["kind"])
            boot_elf = target_dir / target / profile / binary
        # DIO, as ESP-IDF does: the ROM applies the boot header's mode before loading the
        # second stage, and QIO reads garbage on flash whose QE bit is not already set.
        run(sys.executable, "-m", "esptool", "--chip", args.chip, "elf2image", "--flash-mode", "dio",
            "--flash-size", args.flash_size, "--output", str(output / (name + ".bin")),
            str(target_dir / target / profile / binary))
    # The second stage must end before the table, be stamped with this geometry, and fit its RAM.
    _, boot_errors, _ = esp_geometry.check_image(args.chip, output / "bootloader.bin", "unsigned", offset)
    ram_errors, _ = esp_geometry.check_ram(args.chip, boot_elf)
    if boot_errors or ram_errors:
        raise RuntimeError("bootloader: " + "; ".join(boot_errors + ram_errors))
    for name in ("bootloader", "app"):
        if (output / (name + ".bin")).read_bytes()[2] != 0x02:
            raise RuntimeError(name + " image header is not DIO")
    # The application finds the table where its build was told it is: it must be the same place.
    if esp_geometry.stamp_of((output / "app.bin").read_bytes()) != {offset}:
        raise RuntimeError(f"app.bin was not built for a table at {offset:#x}")
    for slot in ("ota_0", "ota_1"):
        run("cargo", "run", "-p", "iobewi-esp-boot", "--example", "validate_image", "--features", args.chip,
            "--", str(output / "app.bin"), f"{by[slot]['offset']:x}", f"{by[slot]['size']:x}")
    run("cargo", "run", "-p", "iobewi-esp-boot", "--example", "factory_otadata", "--", str(output / "otadata.bin"), "0")
    run(sys.executable, str(args.partition_tool.resolve()), "--offset", hex(offset),
        str(csv), str(output / "partitions.bin"))
    (output / "config.bin").write_bytes(bytes([0xff]) * by["config"]["size"])
    # Factory image from the real codecs: an OTSF frame holding the empty OTM1 in copy 0.
    run("cargo", "run", "-q", "-p", "iobewi-firmware-update", "--example", "ota_meta_image", "--",
        str(output / "ota_meta.bin"), "4096", "8192")
    if args.scenario == "ota-supervised":
        (output / "wl_meta.bin").write_bytes(bytes([0xff]) * 0x2000)
    # Where each file goes, from the table itself: the only place offsets are written down.
    import hashlib
    plan = [(0, "bootloader.bin"), (offset, "partitions.bin"), (by["otadata"]["offset"], "otadata.bin"),
            (by["ota_meta"]["offset"], "ota_meta.bin"), (by["ota_0"]["offset"], "app.bin"),
            (by["config"]["offset"], "config.bin")]
    if "wl_meta" in by:
        plan.append((by["wl_meta"]["offset"], "wl_meta.bin"))
    lines = ["# offset file bytes sha256 (initial flash of a development board; config.bin is a virgin config)"]
    for at, name in sorted(plan):
        data = (output / name).read_bytes()
        lines.append(f"{at:#x} {name} {len(data)} {hashlib.sha256(data).hexdigest()}")
    (output / "flash-plan.txt").write_text("\n".join(lines) + "\n")
    print("Prepared:", output, f"(table at {offset:#x}, {csv})")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
