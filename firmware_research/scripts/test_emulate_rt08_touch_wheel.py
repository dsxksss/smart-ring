from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from analyze_rt08_thumb import load_image
from emulate_rt08_touch_wheel import validate_touch_wheel


UNICORN_AVAILABLE = importlib.util.find_spec("unicorn") is not None
V10 = (
    Path(__file__).resolve().parents[2]
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.54_260828_imu_touch_v10_final.NON_FLASHABLE.bin"
)


@unittest.skipUnless(UNICORN_AVAILABLE and V10.exists(), "analysis dependency/image missing")
class TouchWheelEmulationTests(unittest.TestCase):
    def test_v10_maps_y_sign_to_wheel_and_clears_pointer_fields(self) -> None:
        report = validate_touch_wheel(load_image(V10))
        self.assertEqual(
            [item["wheel"] for item in report["executions"]], [-1, 0, 1]
        )
        self.assertTrue(report["pointer_axes_always_zero"])
        self.assertTrue(report["mouse_buttons_always_zero"])
        self.assertTrue(report["extended_mouse_report_blocked"])
        self.assertFalse(report["flash_allowed"])


if __name__ == "__main__":
    unittest.main()
