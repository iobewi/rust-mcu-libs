"""Check the ELF footprint tool on a synthetic ELF32: only allocated, non-NOBITS sections count."""
import pathlib, struct, subprocess, sys, tempfile

TOOL = pathlib.Path(__file__).resolve().parent / "elf_size.py"


def elf(sections):
    """sections: [(flags, type, size)] -> minimal ELF32 LE with a section header table."""
    header = bytearray(52)
    header[:4], header[4], header[5] = b"\x7fELF", 1, 1
    table_offset = 52
    struct.pack_into("<I", header, 0x20, table_offset)
    struct.pack_into("<HH", header, 0x2E, 40, len(sections))
    table = b"".join(struct.pack("<IIIIII", 0, kind, flags, 0, 0, size) + bytes(16) for flags, kind, size in sections)
    return bytes(header) + table


def run(*args):
    return subprocess.run([sys.executable, str(TOOL), *args], capture_output=True, text=True)


with tempfile.TemporaryDirectory() as d:
    base, signed, bad = (pathlib.Path(d) / n for n in ("base", "signed", "bad"))
    base.write_bytes(elf([(0, 0, 999), (2, 1, 1000), (2, 1, 500), (2, 8, 4000), (3, 1, 24)]))
    signed.write_bytes(elf([(2, 1, 1000), (2, 1, 21000)]))
    bad.write_bytes(b"not an elf")
    failures = []
    if run(str(base)).stdout.split()[0] != "1524":
        failures.append("footprint ignores non-alloc and NOBITS sections")
    if run("--compare", str(base), str(signed), "--max-growth", "20476").returncode != 0:
        failures.append("growth within budget must pass")
    if run("--compare", str(base), str(signed), "--max-growth", "20000").returncode == 0:
        failures.append("growth above budget must fail")
    if run(str(bad)).returncode == 0:
        failures.append("a non-ELF file must fail")
    print("\n".join(failures) or "elf_size ok")
    sys.exit(bool(failures))
