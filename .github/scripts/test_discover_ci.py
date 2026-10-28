"""Domain partition and CI selection scenarios (fail-closed, no network)."""
from unittest.mock import patch
import discover_ci as d

packages, profiles, features, reverse = d.load_workspace()
names = set(packages)
ota = {n for n, p in packages.items() if d.in_domain(p, "firmware-ota")}
general = {n for n, p in packages.items() if d.in_domain(p, "general")}

# Partition: every crate belongs to exactly one domain.
assert ota | general == names and not ota & general
assert {n for n, p in packages.items() if d.in_domain(p, "all")} == names
# Pinned so the OTA perimeter cannot silently shrink or drift.
assert ota == {
    "iobewi-firmware-image", "iobewi-firmware-auth", "iobewi-firmware-layout", "iobewi-firmware-slots",
    "iobewi-firmware-boot", "iobewi-firmware-update", "iobewi-workload",
    "iobewi-workload-abi", "iobewi-workload-image", "iobewi-workload-native",
    "iobewi-esp-boot", "iobewi-esp-firmware-a", "iobewi-esp-firmware-storage",
    "iobewi-esp-firmware-supervisor", "iobewi-esp-bootloader",
    "example-firmware-ota-system", "example-firmware-ota-supervised",
    "example-supervised-component", "iobewi-workload-pack",
}, sorted(ota)
# The two special OTA test suites must be reachable from the OTA domain.
assert {"iobewi-firmware-boot", "iobewi-firmware-update"} <= ota
print("Domain partition passed:", len(ota), "OTA +", len(general), "general")

def split(changed, base=None):
    chosen = d.select(changed, packages, reverse, base)
    return chosen & ota, chosen & general

def check(label, changed, want_ota, want_general, base=None):
    got_ota, got_general = split(changed, base)
    assert bool(got_ota) == want_ota and bool(got_general) == want_general, (label, sorted(got_ota), sorted(got_general))
    print(f"ok  {label}: OTA={len(got_ota)} general={len(got_general)}")
    return got_ota, got_general

check("OTA crate only", ["common/firmware/update/src/lib.rs"], True, False)
check("OTA example only", ["examples/firmware/ota-system/src/main.rs"], True, False)
check("non-OTA crate only", ["common/state/src/lib.rs"], False, True)
o, g = check("shared dependency (net-io)", ["common/net/io/src/lib.rs"], True, True)
check("crypto library used by OTA", ["common/crypto/core/src/lib.rs"], True, True)
o, g = check("ESP32 flash", ["arch/esp32/flash/src/lib.rs"], True, True)
assert {"iobewi-esp-config-space", "example-config-manager"} <= g and "iobewi-esp-firmware-storage" in o
check("OTA and non-OTA together", ["common/firmware/update/src/lib.rs", "common/state/src/lib.rs"], True, True)
check("no change", [], False, False)

# Workspace manifest: member list untouched -> nothing; other keys -> everything.
with patch.object(d, "workspace_members_only", return_value=True):
    check("Cargo.toml members unchanged", ["Cargo.toml"], False, False, base="HEAD")
with patch.object(d, "workspace_members_only", return_value=False):
    check("Cargo.toml dependencies changed", ["Cargo.toml"], True, True, base="HEAD")

# Global CI tooling and unknown root files validate both domains in full.
for path in (".github/scripts/discover_ci.py", ".github/workflows/firmware-ota.yml", "Cargo.lock", "AGENTS.md", "unknown-root-file"):
    got = split([path])
    assert got == (ota, general), path
print("ok  CI tooling / global / unknown files select both domains in full")

# Full mode is fail-closed: an empty domain is an error, never a green no-op.
for domain in ("firmware-ota", "general"):
    result, selected, count = d.discover(full=True, domain=domain)
    assert result["include"] and selected and count == len(selected)
with patch.object(d, "FIRMWARE_OTA_DIRS", ("nonexistent/",)):
    try:
        d.discover(full=True, domain="firmware-ota")
    except ValueError:
        print("ok  empty domain rejected")
    else:
        raise AssertionError("empty domain accepted")
print("Selection scenarios passed")
