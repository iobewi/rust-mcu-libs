#!/usr/bin/env python3
"""Console transport matrix: accepted selections build, refused selections fail with their reason.

Every case is a `cargo check` of the connector alone or of a consumer, with
IOBEWI_CONSOLE_PRIMARY and `primary-*` features. A refused case must fail *and* say why, so a
build that breaks for another reason does not pass as a refusal.
"""
import argparse
import os
import subprocess
import sys

TOOLCHAIN = {
    "esp32c3": ("nightly", "riscv32imc-unknown-none-elf"),
    "esp32s3": ("esp", "xtensa-esp32s3-none-elf"),
}
CONNECTOR = "iobewi-esp-console"
BOOTLOADER = "iobewi-esp-bootloader"
APP = "example-firmware-ota-system"

# (name, package, features, extra args, IOBEWI_CONSOLE_PRIMARY, expected failure text(s) or None;
# esp-println's own build script may refuse a missing or doubled transport before this crate does)
def cases(chip):
    def connector(name, features, env, refused=None):
        return (name, CONNECTOR, [chip, *features], ["--no-default-features"], env, refused)
    out = [
        connector("connector uart0", ["primary-uart0"], None),
        connector("connector uart0 explicit env", ["primary-uart0"], "uart0"),
        connector("connector usb-serial-jtag", ["primary-usb-serial-jtag"], "usb-serial-jtag"),
        connector("connector none", ["primary-none"], "none"),
        connector("connector uart0+secondary", ["primary-uart0", "uart-secondary", "lock"], None),
        connector("connector uart0 ring", ["primary-uart0", "ring"], None),
        connector("connector uart0 ring+stats+secondary", ["primary-uart0", "ring-stats", "uart-secondary"], None),
        connector("connector uart0 ring+fault-injection", ["primary-uart0", "fault-injection"], None),
        connector("refused: ring with lock", ["primary-uart0", "ring", "lock"], None,
                  "`ring` and `lock` exclude each other"),
        connector("refused: ring on usb-serial-jtag", ["primary-usb-serial-jtag", "ring"], "usb-serial-jtag",
                  "`ring` is the UART0 console"),
        connector("refused: ring with no console", ["primary-none", "ring"], "none",
                  "`ring` is the UART0 console"),
        connector("refused: env usb-serial-jtag, feature uart0", ["primary-uart0"], "usb-serial-jtag",
                  "does not match the primary-* feature"),
        connector("refused: env none, feature uart0", ["primary-uart0"], "none",
                  "does not match the primary-* feature"),
        connector("refused: env default, feature none", ["primary-none"], None,
                  "does not match the primary-* feature"),
        connector("refused: env uart1 as primary", ["primary-uart0"], "uart1",
                  "only UART0 can be the primary console"),
        connector("refused: unknown env value", ["primary-uart0"], "usb",
                  "IOBEWI_CONSOLE_PRIMARY"),
        connector("refused: no primary feature", [], None,
                  ("select a primary console feature", "Exactly one of the following features")),
        connector("refused: two primary features", ["primary-uart0", "primary-usb-serial-jtag"], None,
                  ("both enabled", "Exactly one of the following features")),
    ]
    for label, feature, env in (("usb-serial-jtag", "usb-serial-jtag", "usb-serial-jtag"), ("none", "none", "none")):
        out.append((f"bootloader {label}", BOOTLOADER, [chip, f"console-{feature}"],
                    ["--no-default-features"], env, None))
        out.append((f"application {label}", APP, [chip, f"console-{feature}"],
                    ["--no-default-features"], env, None))
    out.append(("application ring", APP, [chip, "console-ring"], ["--no-default-features"], None, None))
    out.append(("bootloader/application mismatch refused", BOOTLOADER, [chip, "console-none"],
                ["--no-default-features"], None, "does not match the primary-* feature"))
    return out

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--chip", required=True, choices=sorted(TOOLCHAIN))
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()
    toolchain, target = TOOLCHAIN[args.chip]
    failures = []
    for name, package, features, extra, env, refused in cases(args.chip):
        if args.list:
            print(name)
            continue
        cmd = ["cargo", "+" + toolchain, "check", "-Z", "build-std=core,alloc", "--target", target,
               "-p", package, *extra, "--features", ",".join(features)]
        if package == APP:
            cmd.append("--bins")
        environment = {k: v for k, v in os.environ.items() if k != "IOBEWI_CONSOLE_PRIMARY"}
        if env is not None:
            environment["IOBEWI_CONSOLE_PRIMARY"] = env
        result = subprocess.run(cmd, env=environment, capture_output=True, text=True)
        output = result.stdout + result.stderr
        if refused is None:
            ok = result.returncode == 0
        else:
            reasons = (refused,) if isinstance(refused, str) else refused
            ok = result.returncode != 0 and any(r in output for r in reasons)
        print(("PASS " if ok else "FAIL ") + f"[{args.chip}] {name}", flush=True)
        if not ok:
            failures.append(name)
            print(output[-2000:])
    if failures:
        sys.exit("console matrix failed: " + ", ".join(failures))

if __name__ == "__main__":
    main()
