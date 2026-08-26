from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from emulate_rt08_imu_stream_patch import validate_patch


UNICORN_AVAILABLE = importlib.util.find_spec("unicorn") is not None
PATCH = (
    Path(__file__).resolve().parents[1]
    / "patches"
    / "r08_imu_stream"
    / "build"
    / "r08_imu_stream.bin"
)


@unittest.skipUnless(UNICORN_AVAILABLE and PATCH.exists(), "analysis dependencies/build missing")
class ImuStreamPatchEmulationTests(unittest.TestCase):
    def test_all_safety_scenarios(self) -> None:
        report = validate_patch(PATCH.read_bytes())
        self.assertEqual(report["scenario_count"], 8)
        self.assertFalse(report["flash_allowed"])


if __name__ == "__main__":
    unittest.main()
