from __future__ import annotations

import unittest
from pathlib import Path

from emulate_rt08_touch_wheel_v11_patch import validate_patch


ROOT = Path(__file__).resolve().parents[2]
STOCK = ROOT / "firmware_research" / "evidence" / "ota" / "RT08_3.10.48_260309.bin"
PATCH = (
    ROOT
    / "firmware_research"
    / "patches"
    / "r08_touch_wheel_v11"
    / "build"
    / "r08_touch_wheel_v11.bin"
)


class TouchWheelV11PatchTests(unittest.TestCase):
    @unittest.skipUnless(STOCK.exists() and PATCH.exists(), "local stock/patch missing")
    def test_reviewed_vertical_arrays_are_monotonic_and_slow(self) -> None:
        report = validate_patch(STOCK.read_bytes(), PATCH.read_bytes())
        self.assertTrue(report["calibration_and_release_samples_suppressed"])
        self.assertEqual(report["wheel_steps_per_reviewed_vertical_gesture"], 2)
        self.assertTrue(report["pointer_axes_always_zero"])
        self.assertTrue(report["mouse_buttons_always_zero"])
        self.assertFalse(report["raw_electrode_weight_available"])


if __name__ == "__main__":
    unittest.main()
