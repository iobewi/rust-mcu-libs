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
    parser.add_argument("--auth", choices=("unsigned", "signed", "signed-rsa"), default="unsigned",
                        help="authentication profile of the application (signed needs "
                             "IOBEWI_OTA_SIGNING_PUBKEY in the environment)")
    parser.add_argument("--boot", choices=("unsigned", "verified"), default="unsigned",
                        help="boot profile of the second stage; verified refuses any slot without a valid proof")
    parser.add_argument("--rsa-key", type=Path,
                        help="PKCS#8 PEM of the RSA-3072 key (iap1_pack keygen): needed by --auth signed-rsa and "
                             "--boot verified; the private key never leaves this machine")
    parser.add_argument("--security-version", type=int, default=0)
    args = parser.parse_args()
    if (args.auth == "signed-rsa" or args.boot == "verified") and not args.rsa_key:
        parser.error("--auth signed-rsa and --boot verified need --rsa-key")
    if args.auth == "signed-rsa" and args.scenario != "ota-system":
        parser.error("signed-rsa applies to ota-system only")
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
    import re
    product = re.search(r'PRODUCT: &str = "([^"]+)"',
                        (root / "examples/firmware/ota-system/src/lib.rs").read_text()).group(1)
    def iap1_pack(*arguments):
        return subprocess.check_output(
            ["cargo", "run", "-q", "-p", "iobewi-firmware-bootauth", "--features", "host-sign",
             "--example", "iap1_pack", "--", *map(str, arguments)], text=True)
    pin = None
    if args.rsa_key:
        # The application pins the key; the verified second stage pins its SHA-256. Both come from
        # the one key file here, so they cannot disagree.
        pin = dict(line.split("=") for line in iap1_pack("pin", args.rsa_key).split())["pin"]
        if args.auth == "signed-rsa":
            os.environ["IOBEWI_OTA_SIGNING_RSA_KEY"] = iap1_pack("keyhex", args.rsa_key).strip()
    if args.boot == "verified":
        os.environ["IOBEWI_BOOT_PINNED_KEYS"] = pin
        os.environ["IOBEWI_BOOT_PRODUCT"] = product
        os.environ["IOBEWI_BOOT_VARIANTS"] = "0"
        os.environ["IOBEWI_BOOT_MIN_SECURITY_VERSION"] = str(args.security_version)
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
    if args.auth != "unsigned":
        features += "," + args.auth
    def run(*command):
        subprocess.run(command, check=True)
    run("cargo", "+" + compiler, "build", "-Z", "build-std=core,alloc", "--target", target,
        "-p", "iobewi-esp-bootloader", "--features", args.chip + (",verified-boot" if args.boot == "verified" else ""),
        "--profile", "bootloader", "--bins")
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
    _, boot_errors, _ = esp_geometry.check_image(args.chip, output / "bootloader.bin", args.boot, offset)
    ram_errors, _ = esp_geometry.check_ram(args.chip, boot_elf)
    if boot_errors or ram_errors:
        raise RuntimeError("bootloader: " + "; ".join(boot_errors + ram_errors))
    for name in ("bootloader", "app"):
        if (output / (name + ".bin")).read_bytes()[2] != 0x02:
            raise RuntimeError(name + " image header is not DIO")
    # The application finds the table where its build was told it is: it must be the same place.
    if esp_geometry.stamp_of((output / "app.bin").read_bytes()) != {offset}:
        raise RuntimeError(f"app.bin was not built for a table at {offset:#x}")
    proof_file = None
    if args.auth == "signed-rsa":
        # The IAM1 envelope for X-Image-Manifest, and the 4 KiB boot proof of the image for the slot
        # it is installed in by hand (an OTA writes its own).
        iap1_pack("sign", args.rsa_key, output / "app.bin", product, args.chip,
                  0, args.security_version, output / "app")
        proof_file = "app.iap1"
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
    if proof_file:
        # The last sector of ota_0: its boot proof (needed by a verified second stage).
        plan.append((by["ota_0"]["offset"] + by["ota_0"]["size"] - 0x1000, proof_file))
    lines = ["# offset file bytes sha256 (initial flash of a development board; config.bin is a virgin config)"]
    for at, name in sorted(plan):
        data = (output / name).read_bytes()
        lines.append(f"{at:#x} {name} {len(data)} {hashlib.sha256(data).hexdigest()}")
    (output / "flash-plan.txt").write_text("\n".join(lines) + "\n")
    print("Prepared:", output, f"(table at {offset:#x}, {csv}; auth {args.auth}; boot {args.boot})")
    if pin:
        print(f"Pinned key SHA-256 (bootloader pin): {pin}")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
