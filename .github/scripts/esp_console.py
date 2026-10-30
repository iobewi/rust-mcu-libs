#!/usr/bin/env python3
"""Primary console of the ESP32 reference firmware: one setting, checked across images (issue #102).

The platform's primary console is IOBEWI_CONSOLE_PRIMARY (uart0 by default). Every image that
links `iobewi-esp-console` keeps the stamp `IOBEWI-CONSOLE primary=<console>` and the bootloader
and the application of one platform must carry the same one: an application that talks on
another console than the second stage would leave the board silent where the operator looks.

  esp_console.py stamp BINARY             (prints the console stamped in an image)
  esp_console.py check BINARY [BINARY...] (every image carries a stamp, all of them the same)

Limit: this is a build and packaging check. An application distributed on its own (an OTA image
built elsewhere) is not covered; its console is whatever its builder chose.
"""
import argparse
from pathlib import Path
import re
import sys

STAMP = re.compile(rb"IOBEWI-CONSOLE primary=([a-z0-9-]+)\x00")


class ConsoleError(ValueError):
    pass


def stamp(path):
    """The console stamped in an image; refuses an image without a stamp or with two."""
    found = {m.group(1).decode() for m in STAMP.finditer(Path(path).read_bytes())}
    if not found:
        raise ConsoleError(f"{path}: no console stamp (image not built with iobewi-esp-console, or the stamp was dropped)")
    if len(found) > 1:
        raise ConsoleError(f"{path}: conflicting console stamps {sorted(found)}")
    return found.pop()


def check(paths):
    consoles = {str(p): stamp(p) for p in paths}
    if len(set(consoles.values())) > 1:
        detail = ", ".join(f"{p}: {c}" for p, c in consoles.items())
        raise ConsoleError(f"images disagree on the primary console ({detail})")
    return next(iter(consoles.values()))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("stamp").add_argument("binary")
    sub.add_parser("check").add_argument("binary", nargs="+")
    args = parser.parse_args(argv)
    try:
        if args.command == "stamp":
            print(stamp(args.binary))
        else:
            print(f"primary console {check(args.binary)}: {len(args.binary)} image(s) agree")
    except (ConsoleError, OSError) as e:
        sys.exit(f"esp_console: {e}")


if __name__ == "__main__":
    main()
