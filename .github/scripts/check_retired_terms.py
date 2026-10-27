#!/usr/bin/env python3
"""Fail if the retired boot-metadata identifier appears in any tracked file.

The word is assembled from parts so that this script does not contain it.
"""
import pathlib
import subprocess
import sys

TERM = ("ew" + "bt").encode()
ROOT = pathlib.Path(__file__).resolve().parents[2]

hits = []
for name in subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0"):
    if not name:
        continue
    path = ROOT / name
    if not path.is_file():
        continue
    data = path.read_bytes().lower()
    if TERM in data:
        hits.append(name)
    # The persistent magic must not survive in any encoding either: hex-dumped vectors.
    if TERM.hex().encode() in data:
        hits.append(name + " (hex)")
if hits:
    sys.exit("retired boot-metadata identifier found in: " + ", ".join(sorted(set(hits))))
print("No retired boot-metadata identifier in tracked files")
