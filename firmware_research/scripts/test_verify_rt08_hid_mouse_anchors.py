from __future__ import annotations

import unittest
from pathlib import Path

from verify_rt08_hid_mouse_anchors import verify_hid_helpers


ROOT = Path(__file__).resolve().parents[2]
STOCK = (
    ROOT
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.48_260309.bin"
)


class HidMouseAnchorTests(unittest.TestCase):
    @unittest.skipUnless(STOCK.exists(), "exact stock image not present")
    def test_exact_stock_separates_mouse_and_keyboard_reports(self) -> None:
        report = verify_hid_helpers(STOCK.read_bytes())
        self.assertEqual(len(report["mouse_helpers"]), 3)
        self.assertEqual(len(report["preserved_keyboard_helpers"]), 2)
        self.assertEqual(report["mouse_attribute_index"], 4)
        self.assertEqual(report["keyboard_attribute_index"], 0x18)
        self.assertTrue(report["mouse_only_suppression_boundary_proven"])
        self.assertFalse(report["flash_authorized"])

    @unittest.skipUnless(STOCK.exists(), "exact stock image not present")
    def test_rejects_changed_mouse_helper(self) -> None:
        data = bytearray(STOCK.read_bytes())
        data[0x3FC4] ^= 1
        with self.assertRaisesRegex(ValueError, "stock SHA-256 mismatch"):
            verify_hid_helpers(bytes(data))


if __name__ == "__main__":
    unittest.main()
