from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from analyze_rt08_thumb import load_image
from emulate_rt08_hid_mouse_block import validate_hid_mouse_block


UNICORN_AVAILABLE = importlib.util.find_spec("unicorn") is not None
V9 = (
    Path(__file__).resolve().parents[2]
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.53_260827_imu_touch_v9_final.NON_FLASHABLE.bin"
)


@unittest.skipUnless(UNICORN_AVAILABLE and V9.exists(), "analysis dependency/image missing")
class HidMouseBlockEmulationTests(unittest.TestCase):
    def test_v9_mouse_helpers_return_without_server_send_data(self) -> None:
        report = validate_hid_mouse_block(load_image(V9))
        self.assertEqual(len(report["mouse_helpers"]), 3)
        self.assertEqual(len(report["preserved_keyboard_helpers"]), 2)
        self.assertTrue(report["keyboard_helpers_untouched"])
        self.assertFalse(report["mouse_server_send_data_reached"])
        self.assertTrue(
            all(not item["server_send_data_reached"] for item in report["executions"])
        )
        self.assertFalse(report["flash_allowed"])


if __name__ == "__main__":
    unittest.main()
