from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from audit_r08_flash_readiness import (
    EXPECTED_FIRMWARE,
    EXPECTED_HARDWARE,
    EXPECTED_STOCK_SHA256,
    GATE_SPECS,
    NON_UNIQUE_SCOPE,
    audit,
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class FlashReadinessAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.candidate = self.root / "candidate.bin"
        self.candidate.write_bytes(b"non-flashable-test-candidate")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def manifest(self) -> dict:
        return {
            "schema_version": 1,
            "target": {
                "hardware_revision": EXPECTED_HARDWARE,
                "stock_firmware_revision": EXPECTED_FIRMWARE,
                "stock_sha256": EXPECTED_STOCK_SHA256,
            },
            "candidate": {
                "path": str(self.candidate),
                "sha256": digest(self.candidate),
            },
            "gates": {},
        }

    def write_manifest(self, value: dict) -> Path:
        path = self.root / "readiness.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def artifact(self, name: str, *, index: int, destructive: bool) -> dict:
        path = self.root / f"{name}-{index}.log"
        path.write_text(f"{name} evidence {index}", encoding="utf-8")
        result = {
            "path": str(path),
            "sha256": digest(path),
            "description": f"verified {name} artifact {index}",
        }
        if destructive:
            result["scope"] = NON_UNIQUE_SCOPE
            result["device_identity_sha256"] = "a" * 64
        return result

    def test_missing_gates_fail_closed(self) -> None:
        report = audit(self.write_manifest(self.manifest()))
        self.assertFalse(report["technical_gates_satisfied"])
        self.assertEqual(report["passed_gate_count"], 0)
        self.assertFalse(report["flash_authorized"])

    def test_proven_without_artifacts_is_not_proven(self) -> None:
        value = self.manifest()
        value["gates"] = {
            name: {"status": "proven", "evidence": []} for name in GATE_SPECS
        }
        report = audit(self.write_manifest(value))
        self.assertFalse(report["technical_gates_satisfied"])
        self.assertTrue(
            all("verified artifacts" in gate["reasons"][0] for gate in report["gates"])
        )

    def test_wrong_artifact_hash_is_rejected(self) -> None:
        value = self.manifest()
        artifact = self.artifact("sdk", index=0, destructive=False)
        artifact["sha256"] = "0" * 64
        value["gates"]["exact_rtl8762e_sdk_and_ota_semantics"] = {
            "status": "proven",
            "evidence": [artifact, self.artifact("sdk", index=1, destructive=False)],
        }
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            audit(self.write_manifest(value))

    def test_destructive_unique_target_evidence_is_rejected(self) -> None:
        value = self.manifest()
        name = "same_chip_readback_restore"
        artifacts = [
            self.artifact(name, index=index, destructive=True) for index in range(2)
        ]
        artifacts[0]["scope"] = "unique_target"
        value["gates"][name] = {"status": "proven", "evidence": artifacts}
        report = audit(self.write_manifest(value))
        gate = next(item for item in report["gates"] if item["name"] == name)
        self.assertFalse(gate["passed"])
        self.assertIn("non-unique hardware", gate["reasons"][0])

    def test_all_technical_gates_still_do_not_authorize_flash(self) -> None:
        value = self.manifest()
        for name, spec in GATE_SPECS.items():
            artifacts = [
                self.artifact(name, index=index, destructive=spec.destructive)
                for index in range(spec.minimum_artifacts)
            ]
            if spec.distinct_cold_boots:
                for index, artifact in enumerate(artifacts):
                    artifact["cold_boot_id"] = f"cold-boot-{index}"
            if spec.distinct_storage_media:
                for index, artifact in enumerate(artifacts):
                    artifact["storage_medium"] = f"physical-medium-{index}"
            value["gates"][name] = {"status": "proven", "evidence": artifacts}
        report = audit(self.write_manifest(value))
        self.assertTrue(report["technical_gates_satisfied"])
        self.assertEqual(report["passed_gate_count"], len(GATE_SPECS))
        self.assertFalse(report["flash_authorized"])


if __name__ == "__main__":
    unittest.main()
