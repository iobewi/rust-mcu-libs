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
