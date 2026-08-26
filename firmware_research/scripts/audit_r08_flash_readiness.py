#!/usr/bin/env python3
"""Fail-closed audit of the evidence required before an R08 test flash.

This tool never connects to hardware and never authorizes flashing.  Even when
all technical gates pass, a separate user approval for one exact device and one
exact candidate hash is still required.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
EXPECTED_HARDWARE = "RT08_V3.1"
EXPECTED_FIRMWARE = "RT08_3.10.48_260309"
EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
NON_UNIQUE_SCOPE = "equivalent_non_unique_hardware"


@dataclass(frozen=True)
class GateSpec:
    minimum_artifacts: int
    destructive: bool = False
    distinct_cold_boots: bool = False
    distinct_storage_media: bool = False


GATE_SPECS: dict[str, GateSpec] = {
    "exact_rtl8762e_sdk_and_ota_semantics": GateSpec(2),
    "mcu_flash_identity_and_protection_read_twice": GateSpec(2),
    "complete_flash_map_and_security_policy": GateSpec(2),
    "complete_cold_boot_readback_pair": GateSpec(2, distinct_cold_boots=True),
    "dual_physical_backup": GateSpec(2, distinct_storage_media=True),
    "same_chip_readback_restore": GateSpec(2, destructive=True),
    "independent_recovery_with_broken_application": GateSpec(2, destructive=True),
    "stock_restore_after_erase": GateSpec(2, destructive=True),
    "copy_activation_power_loss_matrix": GateSpec(3, destructive=True),
    "candidate_runtime_ble_power_validation": GateSpec(3, destructive=True),
    "candidate_failure_and_recovery_matrix": GateSpec(3, destructive=True),
    "candidate_diff_and_offline_regression": GateSpec(2),
}


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _require_sha256(value: Any, field: str) -> str:
    if not isinstance(value, str) or len(value) != 64:
        raise ValueError(f"{field} must be a 64-character SHA-256")
    try:
        int(value, 16)
    except ValueError as error:
        raise ValueError(f"{field} is not hexadecimal") from error
    return value.lower()


def _resolve_artifact(manifest_path: Path, artifact: dict[str, Any]) -> dict[str, Any]:
    raw_path = artifact.get("path")
    if not isinstance(raw_path, str) or not raw_path.strip():
        raise ValueError("evidence artifact path is missing")
    path = Path(raw_path)
    if not path.is_absolute():
        path = manifest_path.parent / path
    path = path.resolve()
    if not path.is_file():
        raise ValueError(f"evidence artifact does not exist: {path}")
    expected = _require_sha256(artifact.get("sha256"), f"artifact {path} sha256")
    actual = _sha256(path)
    if actual != expected:
        raise ValueError(
            f"evidence artifact SHA-256 mismatch for {path}: expected {expected}, got {actual}"
        )
    description = artifact.get("description")
    if not isinstance(description, str) or not description.strip():
        raise ValueError(f"evidence artifact description is missing: {path}")
    return {
        "path": str(path),
        "sha256": actual,
        "description": description.strip(),
        "scope": artifact.get("scope"),
        "device_identity_sha256": artifact.get("device_identity_sha256"),
        "cold_boot_id": artifact.get("cold_boot_id"),
        "storage_medium": artifact.get("storage_medium"),
    }


def audit(manifest_path: Path) -> dict[str, Any]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"schema_version must be {SCHEMA_VERSION}")

    target = manifest.get("target")
    if not isinstance(target, dict):
        raise ValueError("target object is missing")
    if target.get("hardware_revision") != EXPECTED_HARDWARE:
        raise ValueError("hardware revision does not match RT08_V3.1")
    if target.get("stock_firmware_revision") != EXPECTED_FIRMWARE:
        raise ValueError("stock firmware revision does not match the pinned build")
    stock_hash = _require_sha256(target.get("stock_sha256"), "target.stock_sha256")
    if stock_hash != EXPECTED_STOCK_SHA256:
        raise ValueError("stock SHA-256 does not match the pinned official image")

    candidate = manifest.get("candidate")
    if not isinstance(candidate, dict):
        raise ValueError("candidate object is missing")
    candidate_path_value = candidate.get("path")
    if not isinstance(candidate_path_value, str) or not candidate_path_value.strip():
        raise ValueError("candidate.path is missing")
    candidate_path = Path(candidate_path_value)
    if not candidate_path.is_absolute():
        candidate_path = manifest_path.parent / candidate_path
    candidate_path = candidate_path.resolve()
    if not candidate_path.is_file():
        raise ValueError(f"candidate does not exist: {candidate_path}")
    candidate_hash = _require_sha256(candidate.get("sha256"), "candidate.sha256")
    actual_candidate_hash = _sha256(candidate_path)
    if actual_candidate_hash != candidate_hash:
        raise ValueError("candidate SHA-256 does not match candidate file")

    gates = manifest.get("gates")
    if not isinstance(gates, dict):
        raise ValueError("gates object is missing")
    unknown = sorted(set(gates) - set(GATE_SPECS))
    if unknown:
        raise ValueError(f"unknown readiness gates: {', '.join(unknown)}")

    gate_reports: list[dict[str, Any]] = []
    all_passed = True
    for name, spec in GATE_SPECS.items():
        gate = gates.get(name)
        if not isinstance(gate, dict):
            gate_reports.append(
                {"name": name, "passed": False, "reasons": ["gate is missing"]}
            )
            all_passed = False
            continue
        status = gate.get("status")
        artifacts = gate.get("evidence", [])
        reasons: list[str] = []
        verified: list[dict[str, Any]] = []
        if status != "proven":
            reasons.append("status is not 'proven'")
        if not isinstance(artifacts, list):
            reasons.append("evidence must be a list")
            artifacts = []
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                raise ValueError(f"gate {name} has a non-object evidence item")
            verified.append(_resolve_artifact(manifest_path, artifact))
        if len(verified) < spec.minimum_artifacts:
            reasons.append(
                f"needs at least {spec.minimum_artifacts} verified artifacts"
            )
        paths = [item["path"] for item in verified]
        if len(paths) != len(set(paths)):
            reasons.append("evidence paths must be distinct within a gate")
        if spec.destructive:
            for item in verified:
                if item["scope"] != NON_UNIQUE_SCOPE:
                    reasons.append(
                        "destructive evidence must come from equivalent non-unique hardware"
                    )
                    break
                identity = item["device_identity_sha256"]
                try:
                    _require_sha256(identity, "device_identity_sha256")
                except ValueError as error:
                    reasons.append(str(error))
                    break
        if spec.distinct_cold_boots:
            cold_boots = [item["cold_boot_id"] for item in verified]
            if any(not isinstance(value, str) or not value for value in cold_boots):
                reasons.append("every readback artifact needs a cold_boot_id")
            elif len(set(cold_boots)) < 2:
                reasons.append("readback evidence must cover two distinct cold boots")
        if spec.distinct_storage_media:
            media = [item["storage_medium"] for item in verified]
            if any(not isinstance(value, str) or not value for value in media):
                reasons.append("every backup artifact needs a storage_medium")
            elif len(set(media)) < 2:
                reasons.append("backups must be verified on two physical storage media")
        passed = not reasons
        all_passed &= passed
        gate_reports.append(
            {
                "name": name,
                "passed": passed,
                "verified_artifact_count": len(verified),
                "reasons": reasons,
            }
        )

    return {
        "classification": "R08_FLASH_READINESS_AUDIT",
        "target": {
            "hardware_revision": EXPECTED_HARDWARE,
            "stock_firmware_revision": EXPECTED_FIRMWARE,
            "stock_sha256": stock_hash,
        },
        "candidate": {
            "path": str(candidate_path),
            "sha256": actual_candidate_hash,
        },
        "technical_gates_satisfied": all_passed,
        "gate_count": len(GATE_SPECS),
        "passed_gate_count": sum(item["passed"] for item in gate_reports),
        "gates": gate_reports,
        "flash_authorized": False,
        "authorization_note": (
            "This audit can establish technical readiness only. It never authorizes "
            "flashing; the user must separately approve one exact device and this "
            "exact candidate SHA-256 after reviewing the final recovery procedure."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Fail-closed audit of R08 flash-readiness evidence"
    )
    parser.add_argument("manifest", type=Path)
    args = parser.parse_args()
    try:
        report = audit(args.manifest.resolve())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["technical_gates_satisfied"] else 3


if __name__ == "__main__":
    raise SystemExit(main())
