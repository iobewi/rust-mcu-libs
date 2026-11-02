import tempfile
from pathlib import Path
from compile_mcu import check_size, LIMIT

with tempfile.TemporaryDirectory() as directory:
    image = Path(directory)/"boot.bin"
    for size in (1, LIMIT):
        image.write_bytes(bytes(size))
        assert check_size(image) == size
    for size in (0, LIMIT+1, 33488):  # historical Full verification overflow
        image.write_bytes(bytes(size))
        try:
            check_size(image)
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted invalid size {size}")
print("Boot image gate boundaries passed")

# The default (unsigned) second stage is gated by the geometry tool, per chip and table offset.
import esp_geometry
for chip, offset, current in (("esp32c3", 0x8000, 23008), ("esp32s3", 0x8000, 30016), ("esp32s3", 0x10000, 30016)):
    limits = esp_geometry.limits(chip, offset)
    assert limits["unsigned"] - current >= 512, "budget keeps room for ordinary growth"
    assert limits["unsigned"] <= limits["hard"]
print("Per-chip boot budgets passed")

# Host coverage of a firmware must not turn its MCU link gate into cargo check.
from unittest.mock import patch
import compile_mcu
for chip in ("esp32c3", "esp32s3"):
    with patch.object(compile_mcu.subprocess, "run") as run:
        compile_mcu.compile_selected([{
            "package": "reference-test", "profile": "host-esp32-linked-bins",
            "features": "", "targets": "bins",
        }], chip)
        command = run.call_args.args[0]
        assert "build" in command and "--release" in command and "--bins" in command
        assert "check" not in command
print("Host/MCU combined profile preserves release link gates")

with patch.object(compile_mcu.subprocess, "run") as run:
    compile_mcu.compile_selected([{
        "package": "native-test", "profile": "host-esp32s3-linked-bins",
        "features": "", "targets": "bins",
    }], "esp32s3")
    command = run.call_args.args[0]
    assert "build" in command and "--release" in command and "--bins" in command
print("Native S3 profile preserves release link gate")
