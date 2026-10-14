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
