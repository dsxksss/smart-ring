from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from analyze_rt08_thumb import load_image
from emulate_rt08_stock_activation import validate_activation


UNICORN_AVAILABLE = importlib.util.find_spec("unicorn") is not None
STOCK = (
    Path(__file__).resolve().parents[2]
    / "research_artifacts"
    / "firmware"
    / "RT08_3.10.48_260309.bin"
)


@unittest.skipUnless(UNICORN_AVAILABLE and STOCK.exists(), "analysis dependency/image missing")
class StockActivationEmulationTests(unittest.TestCase):
    def test_address_gates_and_commit_paths(self) -> None:
        report = validate_activation(load_image(STOCK))
        self.assertEqual(report["scenario_count"], 5)
        self.assertTrue(report["resolver_failure_blocks_validation_and_commit"])
        self.assertTrue(report["validation_failure_blocks_commit"])
        self.assertTrue(report["validation_success_commits_checked_address"])
        self.assertTrue(report["second_argument_is_conditional_address_offset"])
        self.assertTrue(report["rom_api_names_proven"])
        self.assertTrue(report["staged_not_ready_clear_path_proven"])
        self.assertFalse(report["installed_application_flag_transition_proven"])
        self.assertFalse(report["ota_bank_header_update_proven"])
        self.assertFalse(report["runtime_rollback_proven"])
        self.assertFalse(report["flash_authorized"])

    def test_tampered_stock_is_rejected_before_execution(self) -> None:
        stock = bytearray(load_image(STOCK))
        stock[-1] ^= 1
        with self.assertRaisesRegex(ValueError, "stock SHA-256 mismatch"):
            validate_activation(bytes(stock))


if __name__ == "__main__":
    unittest.main()
