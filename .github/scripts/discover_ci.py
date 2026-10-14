#!/usr/bin/env python3
"""Fail-closed workspace CI discovery and reverse dependency selection."""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
ALLOWED = {"host", "esp32", "esp32-bins", "host-esp32", "esp32-bootloader", "esp32-linked-bins", "host-esp32-linked-bins", "host-esp32s3-linked-bins"}
GLOBAL = {".github/", "Cargo.lock", "rust-toolchain.toml", ".cargo/", "AGENTS.md", "ARCHITECTURE.md"}

def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()

def workspace_members_only(base):
    try:
        old = tomllib.loads(git('show', base + ':Cargo.toml'))
        new = tomllib.loads((ROOT / 'Cargo.toml').read_text())
    except (subprocess.CalledProcessError, OSError, ValueError):
        return False
    for workspace in (old, new):
        workspace['workspace'].pop('members', None)
        workspace['workspace'].pop('default-members', None)
    return old == new

def select(changed, packages, reverse, base=None):
    affected = set()
    if any(any(p == prefix or p.startswith(prefix) for prefix in GLOBAL) for p in changed):
        return set(packages)
    for path in changed:
        if path == 'Cargo.toml' and base and workspace_members_only(base):
            old = tomllib.loads(git('show', base + ':Cargo.toml'))['workspace']['members']
            new = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['members']
            affected.update(name for name, directory in packages.items() if directory in set(old) ^ set(new))
            continue
        owners = [name for name, directory in packages.items() if path.startswith(directory + "/")]
        if not owners:
            return set(packages)  # unknown root-level change: fail safe
        affected.update(owners)
    pending = list(affected)
    while pending:
        for consumer in reverse.get(pending.pop(), ()):
            if consumer not in affected:
                affected.add(consumer)
                pending.append(consumer)
    return affected

def discover(base=None, head="HEAD", full=False):
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    members = workspace["workspace"]["members"]
    profiles, packages, features = {}, {}, {}
    for member in members:
        directory = ROOT / member
        manifest = directory / "Cargo.toml"
        config_path = directory / "ci.json"
        if not manifest.is_file() or not config_path.is_file():
            raise ValueError(f"{member}: every workspace crate requires Cargo.toml and ci.json")
        name = tomllib.loads(manifest.read_text())["package"]["name"]
        config = json.loads(config_path.read_text())
        if (not isinstance(config, dict) or "profile" not in config
                or not set(config) <= {"profile", "features"}
                or not isinstance(config["profile"], str)
                or config["profile"] not in ALLOWED):
            raise ValueError(f"{member}: invalid ci.json profile")
        extra = config.get("features", [])
        if not isinstance(extra, list) or any(not isinstance(f, str) or not f or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-" for c in f) for f in extra):
            raise ValueError(f"{member}: invalid ci.json features")
        features[name] = ",".join(extra)
        if name in packages:
            raise ValueError(f"duplicate package name: {name}")
        packages[name], profiles[name] = member, config["profile"]

    # Resolve effective Cargo package IDs, including renamed/path dependencies,
    # using Cargo's authoritative graph, including optional consumers. All
    # features are for resolution only, never compiled together across MCUs.
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--all-features", "--format-version", "1"], cwd=ROOT, text=True
    ))
    ids = {pkg["id"]: pkg["name"] for pkg in metadata["packages"] if pkg["name"] in packages}
    reverse = {name: set() for name in packages}
    for node in metadata["resolve"]["nodes"]:
        consumer = ids.get(node["id"])
        if consumer:
            for dep in node["deps"]:
                dependency = ids.get(dep["pkg"])
                if dependency:
                    reverse[dependency].add(consumer)
    if set(ids.values()) != set(packages):
        raise ValueError("workspace packages missing from Cargo metadata")

    if full or not base:
        selected = set(packages)
    else:
        changed = git("diff", "--name-only", "--no-renames", base, head).splitlines()
        selected = select(changed, packages, reverse, base)
    matrix = []
    for name in sorted(selected):
        profile = profiles[name]
        chips = ("host", "esp32s3") if profile == "host-esp32s3-linked-bins" else ("host",) if profile == "host" else (("host", "esp32c3", "esp32s3") if profile in ("host-esp32", "host-esp32-linked-bins") else ("esp32c3", "esp32s3"))
        for chip in chips:
            matrix.append({"package": name, "chip": chip, "targets": "bins" if profile in ("esp32-bins", "esp32-bootloader", "esp32-linked-bins", "host-esp32-linked-bins", "host-esp32s3-linked-bins") and chip != "host" else "lib", "profile": profile, "features": features[name]})
    return {"include": matrix}, selected, len(packages)

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--github-output")
    parser.add_argument("--base")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--full", action="store_true")
    args = parser.parse_args()
    try:
        result, selected, count = discover(args.base, args.head, args.full)
        encoded = json.dumps(result, separators=(",", ":"))
        xtensa = {"include": [item for item in result["include"] if item["chip"] == "esp32s3"]}
        print(f"Selected {len(selected)}/{count} crates: {', '.join(sorted(selected))}")
        print(encoded)
        if args.github_output:
            with open(args.github_output, "a") as output:
                host = {"include": [item for item in result["include"] if item["chip"] == "host"]}
                mcu = {"include": [item for item in result["include"] if item["chip"] != "host"]}
                output.write("host_matrix=" + json.dumps(host, separators=(",", ":")) + "\n")
                output.write("mcu_matrix=" + json.dumps(mcu, separators=(",", ":")) + "\n")
                output.write("has_host=" + str(bool(host["include"])).lower() + "\n")
                output.write("has_mcu=" + str(bool(mcu["include"])).lower() + "\n")
                for chip in ("esp32c3", "esp32s3"):
                    entries = [item for item in mcu["include"] if item["chip"] == chip]
                    output.write(chip + "_packages=" + json.dumps(entries, separators=(",", ":")) + "\n")
                    output.write("has_" + chip + "=" + str(bool(entries)).lower() + "\n")
                output.write("matrix=" + encoded + "\n")
                output.write("xtensa_matrix=" + json.dumps(xtensa, separators=(",", ":")) + "\n")
                output.write("has_jobs=" + str(bool(result["include"])).lower() + "\n")
                output.write("has_xtensa=" + str(bool(xtensa["include"])).lower() + "\n")
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        sys.exit(f"CI discovery error: {error}")
