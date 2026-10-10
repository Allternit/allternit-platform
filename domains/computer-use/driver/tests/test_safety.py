"""Safety helpers that need no OS access (context/ocr themselves are macOS).
Run from domains/computer-use/driver: python -m unittest discover -s tests"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from allternit_driver import safety  # noqa: E402


class SafetyHelpersTest(unittest.TestCase):
    def test_word_spans_are_character_offsets(self):
        self.assertEqual(safety._word_spans("Email  ada@example.com"), [(0, 5), (7, 22)])
        self.assertEqual(safety._word_spans("   "), [])

    def test_vision_rect_to_top_left_pixels(self):
        # Vision: normalized, bottom-left origin. A box in the top-left
        # quarter of a 200x100 image.
        self.assertEqual(safety._px(((0.0, 0.5), (0.5, 0.5)), 200, 100), [0, 0, 101, 51])

    def test_host_of(self):
        self.assertEqual(safety.host_of("https://Mail.Google.com/mail/u/0"), "mail.google.com")
        self.assertIsNone(safety.host_of(None))
        self.assertIsNone(safety.host_of("about:blank"))

    def test_png_size_and_rejects_other_formats(self):
        header = b"\x89PNG\r\n\x1a\n" + b"\x00\x00\x00\rIHDR" + (640).to_bytes(4, "big") + (480).to_bytes(4, "big")
        self.assertEqual(safety._png_size(header), (640, 480))
        with self.assertRaises(ValueError):
            safety._png_size(b"GIF89a")

    def test_unsupported_off_macos(self):
        if sys.platform == "darwin":
            self.skipTest("macOS runs the real implementation")
        with self.assertRaises(safety.Unsupported):
            safety.context({})
        with self.assertRaises(safety.Unsupported):
            safety.ocr({"png": ""})


if __name__ == "__main__":
    unittest.main()
