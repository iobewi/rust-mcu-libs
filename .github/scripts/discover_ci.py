#!/usr/bin/env python3
"""Build the GitHub Actions matrix from each workspace member's ci.json."""
import json
import pathlib
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
ALLOWED = {"host", "esp32", "esp32-bins"}

def discover():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    members = workspace["workspace"]["members"]
    matrix = []
    for member in members:
        directory = ROOT / member
        manifest = directory / "Cargo.toml"
        config_path = directory / "ci.json"
        if not manifest.is_file() or not config_path.is_file():
            raise ValueError(f"{member}: every workspace crate requires Cargo.toml and ci.json")
        name = tomllib.loads(manifest.read_text())["package"]["name"]
        config = json.loads(config_path.read_text())
        if set(config) != {"profile"} or config["profile"] not in ALLOWED:
            raise ValueError(f"{member}: invalid ci.json profile")
        profile = config["profile"]
        chips = ("host",) if profile == "host" else ("esp32c3", "esp32s3")
        for chip in chips:
            matrix.append({"package": name, "chip": chip, "targets": "bins" if profile == "esp32-bins" else "lib"})
    return {"include": matrix}

if __name__ == "__main__":
    try:
        result = discover()
        if not result["include"]:
            raise ValueError("empty CI matrix")
        encoded = json.dumps(result, separators=(",", ":"))
        print(encoded)
        if len(sys.argv) == 3 and sys.argv[1] == "--github-output":
            with open(sys.argv[2], "a") as output:
                output.write("matrix=" + encoded + "\n")
                xtensa = {"include": [item for item in result["include"] if item["chip"] == "esp32s3"]}
                output.write("xtensa_matrix=" + json.dumps(xtensa, separators=(",", ":")) + "\n")
    except (ValueError, KeyError, OSError, json.JSONDecodeError) as error:
        sys.exit(f"CI discovery error: {error}")
