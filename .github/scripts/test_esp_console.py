import tempfile
import unittest
from pathlib import Path

import esp_console


def image(directory, name, *stamps):
    path = Path(directory) / name
    path.write_bytes(b"\x00junk" + b"".join(f"IOBEWI-CONSOLE primary={s}\0".encode() + b"pad" for s in stamps))
    return path


class ConsoleStampTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def test_reads_the_stamp(self):
        self.assertEqual(esp_console.stamp(image(self.dir.name, "a.bin", "uart0")), "uart0")
        self.assertEqual(esp_console.stamp(image(self.dir.name, "b.bin", "usb-serial-jtag")), "usb-serial-jtag")

    def test_agreeing_images_pass(self):
        a, b = image(self.dir.name, "a", "uart0"), image(self.dir.name, "b", "uart0")
        self.assertEqual(esp_console.check([a, b]), "uart0")

    def test_disagreement_is_refused(self):
        a, b = image(self.dir.name, "a", "uart0"), image(self.dir.name, "b", "usb-serial-jtag")
        with self.assertRaisesRegex(esp_console.ConsoleError, "disagree"):
            esp_console.check([a, b])

    def test_missing_stamp_is_refused(self):
        bare = Path(self.dir.name) / "bare"
        bare.write_bytes(b"no stamp here")
        with self.assertRaisesRegex(esp_console.ConsoleError, "no console stamp"):
            esp_console.stamp(bare)

    def test_truncated_stamp_is_not_a_stamp(self):
        cut = Path(self.dir.name) / "cut"
        cut.write_bytes(b"IOBEWI-CONSOLE primary=uart0")  # no NUL terminator
        with self.assertRaises(esp_console.ConsoleError):
            esp_console.stamp(cut)

    def test_two_stamps_in_one_image_are_refused(self):
        with self.assertRaisesRegex(esp_console.ConsoleError, "conflicting"):
            esp_console.stamp(image(self.dir.name, "x", "uart0", "none"))


if __name__ == "__main__":
    unittest.main()
