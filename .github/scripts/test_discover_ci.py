"""Domain partition and CI selection scenarios (fail-closed, no network)."""
from unittest.mock import patch
import discover_ci as d

packages, profiles, features, reverse = d.load_workspace()
names = set(packages)
ota = {n for n, p in packages.items() if d.in_domain(p, "firmware-ota")}
general = {n for n, p in packages.items() if d.in_domain(p, "general")}

# Partition: the firmware/OTA sub-system moved to iobewi/rust-ota; every crate here is general.
assert general == names and not ota
assert {n for n, p in packages.items() if d.in_domain(p, "all")} == names
# iobewi-firmware-layout and iobewi-firmware-slots stay here: iobewi-esp-config-space depends on them, and
# rust-ota consumes them as git dependencies.
assert {"iobewi-firmware-layout", "iobewi-firmware-slots", "iobewi-esp-config-space"} <= general
print("Domain partition passed:", len(general), "general crates, firmware/OTA domain empty")

def check(label, changed, want, base=None):
    got = d.select(changed, packages, reverse, base)
    assert bool(got) == want, (label, sorted(got))
    print(f"ok  {label}: {len(got)}")
    return got

check("one crate only", ["common/state/src/lib.rs"], True)
check("shared dependency (net-io)", ["common/net/io/src/lib.rs"], True)
got = check("layout is consumed by the config adapter", ["common/firmware/layout/src/lib.rs"], True)
assert {"iobewi-firmware-layout", "iobewi-esp-config-space"} <= got
got = check("ESP32 flash", ["arch/esp32/flash/src/lib.rs"], True)
assert {"iobewi-esp-config-space", "example-config-manager"} <= got
check("no change", [], False)

# Workspace manifest: member list untouched -> nothing; other keys -> everything.
with patch.object(d, "workspace_members_only", return_value=True):
    check("Cargo.toml members unchanged", ["Cargo.toml"], False, base="HEAD")
with patch.object(d, "workspace_members_only", return_value=False):
    assert check("Cargo.toml dependencies changed", ["Cargo.toml"], True, base="HEAD") == names

# Global CI tooling and unknown root files validate everything.
for path in (".github/scripts/discover_ci.py", ".github/workflows/rust-mcu.yml", "Cargo.lock", "AGENTS.md", "unknown-root-file"):
    assert d.select([path], packages, reverse) == names, path
print("ok  CI tooling / global / unknown files select everything")

# Full mode is fail-closed: an empty domain is an error, never a green no-op.
result, selected, count = d.discover(full=True, domain="general")
assert result["include"] and selected and count == len(selected)
try:
    d.discover(full=True, domain="firmware-ota")
except ValueError:
    print("ok  the firmware/OTA domain owns nothing here and is rejected")
else:
    raise AssertionError("empty firmware/OTA domain accepted")
print("Selection scenarios passed")
