from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from analyze_rt08_thumb import load_image
from emulate_rt08_touch_indicator import validate_touch_indicator


UNICORN_AVAILABLE = importlib.util.find_spec("unicorn") is not None
V8 = (
    Path(__file__).resolve().parents[2]
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.52_260827_imu_touch_v8_final.NON_FLASHABLE.bin"
)
V9 = (
    Path(__file__).resolve().parents[2]
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.53_260827_imu_touch_v9_final.NON_FLASHABLE.bin"
)


@unittest.skipUnless(UNICORN_AVAILABLE and V8.exists(), "analysis dependency/image missing")
class TouchIndicatorEmulationTests(unittest.TestCase):
    def test_v8_calls_the_stock_touch_indicator_with_three_repeats(self) -> None:
        report = validate_touch_indicator(load_image(V8))
        self.assertEqual(
            report["classification"],
            "INSTRUCTION_LEVEL_V8_TOUCH_INDICATOR_VALIDATION",
        )
        self.assertEqual(
            report["captured_arguments"],
            {"r0": 20, "r1": 1, "r2": 1, "r3": 3},
        )
        self.assertTrue(report["stock_touch_indicator_path_reached"])
        self.assertFalse(report["optical_sensor_function_reached"])
        self.assertFalse(report["flash_allowed"])

    @unittest.skipUnless(V9.exists(), "v9 image missing")
    def test_v9_preserves_the_three_repeat_touch_indicator(self) -> None:
        report = validate_touch_indicator(load_image(V9))
        self.assertEqual(
            report["classification"],
            "INSTRUCTION_LEVEL_V9_TOUCH_INDICATOR_VALIDATION",
        )
        self.assertEqual(report["firmware_profile"], "v9")
        self.assertEqual(report["repeat_count"], 3)
        self.assertTrue(report["stock_touch_indicator_path_reached"])
        self.assertFalse(report["optical_sensor_function_reached"])


if __name__ == "__main__":
    unittest.main()
