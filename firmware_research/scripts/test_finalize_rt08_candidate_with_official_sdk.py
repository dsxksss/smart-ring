from __future__ import annotations

import struct
import unittest
from pathlib import Path

from finalize_rt08_candidate_with_official_sdk import (
    EXPECTED_NEW_IMAGE_DIGEST,
    EXPECTED_OLD_IMAGE_DIGEST,
    HEADER_SIZE,
    INNER_SHA256_OFFSET,
    PROFILES,
    finalize_candidate,
)


ROOT = Path(__file__).resolve().parents[2]
V7_PREFINAL = (
    ROOT
    / "firmware_research"
    / "patches"
    / "r08_imu_stream"
    / "build"
    / "RT08_3.10.51_260827_imu_stream_v7_prefinal.NON_FLASHABLE.bin"
)
V8_PREFINAL = (
    ROOT
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.52_260827_imu_touch_v8_prefinal.NON_FLASHABLE.bin"
)
V9_PREFINAL = (
    ROOT
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.53_260827_imu_touch_v9_prefinal.NON_FLASHABLE.bin"
)
V10_PREFINAL = (
    ROOT
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.54_260828_imu_touch_v10_prefinal.NON_FLASHABLE.bin"
)
V11_PREFINAL = (
    ROOT
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.55_260828_imu_touch_v11_prefinal.NON_FLASHABLE.bin"
)


def synthetic_candidate() -> bytes:
    payload = bytearray(0x400)
    payload[INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32] = (
        EXPECTED_OLD_IMAGE_DIGEST
    )
    container = bytearray(HEADER_SIZE)
    container[:4] = bytes.fromhex("e5 c3 bd 81")
    struct.pack_into("<III", container, 4, len(payload), len(payload), sum(payload))
    return bytes(container + payload)


class OfficialSdkFinalizerTests(unittest.TestCase):
    def processed_inner(self, candidate: bytes) -> bytes:
        inner = bytearray(candidate[HEADER_SIZE:])
        self.assertEqual(
            inner[INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32],
            EXPECTED_OLD_IMAGE_DIGEST,
        )
        inner[INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32] = (
            EXPECTED_NEW_IMAGE_DIGEST
        )
        return bytes(inner)

    def test_only_accepts_the_reviewed_digest_update(self) -> None:
        candidate = synthetic_candidate()
        finalized, report = finalize_candidate(
            candidate,
            self.processed_inner(candidate),
            enforce_input_hash=False,
        )
        self.assertEqual(
            finalized[
                HEADER_SIZE
                + INNER_SHA256_OFFSET : HEADER_SIZE
                + INNER_SHA256_OFFSET
                + 32
            ],
            EXPECTED_NEW_IMAGE_DIGEST,
        )
        self.assertEqual(
            struct.unpack_from("<I", finalized, 12)[0],
            sum(finalized[HEADER_SIZE:]) & 0xFFFFFFFF,
        )
        self.assertEqual(report["official_tool_changed_inner_bytes"], 32)
        self.assertFalse(report["flash_allowed"])

    def test_rejects_an_unrelated_tool_change(self) -> None:
        candidate = synthetic_candidate()
        processed = bytearray(self.processed_inner(candidate))
        processed[0x200] ^= 1
        with self.assertRaisesRegex(ValueError, "unexpected inner offset"):
            finalize_candidate(
                candidate,
                bytes(processed),
                enforce_input_hash=False,
            )

    def test_accepts_digest_when_one_new_byte_matches_the_old_digest(self) -> None:
        candidate = synthetic_candidate()
        profile = {
            **PROFILES["imu-v7"],
            "new_digest": bytes(
                [EXPECTED_OLD_IMAGE_DIGEST[0]]
                + list(PROFILES["imu-v7"]["new_digest"][1:])
            ),
        }
        inner = bytearray(candidate[HEADER_SIZE:])
        inner[
            INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32
        ] = profile["new_digest"]

        _finalized, report = finalize_candidate(
            candidate,
            bytes(inner),
            enforce_input_hash=False,
            profile=profile,
        )

        self.assertEqual(report["official_tool_changed_inner_bytes"], 31)

    @unittest.skipUnless(V7_PREFINAL.exists(), "exact v7 pre-final image not present")
    def test_exact_v7_profile_produces_locked_candidate(self) -> None:
        profile = PROFILES["imu-v7"]
        candidate = V7_PREFINAL.read_bytes()
        inner = bytearray(candidate[HEADER_SIZE:])
        digest_slice = slice(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32)
        self.assertEqual(inner[digest_slice], profile["old_digest"])
        inner[digest_slice] = profile["new_digest"]

        finalized, report = finalize_candidate(candidate, bytes(inner), profile=profile)

        self.assertEqual(
            report["candidate_sha256"],
            "575d500b385f61b6cc1cf8eb9d1a55b68da4ff49a0be32800f8f91f2d8a1ff2a",
        )
        self.assertEqual(report["classification"], "SDK_FINALIZED_IMU_STREAM_CANDIDATE")
        self.assertTrue(report["imu_patch_present"])
        self.assertFalse(report["flash_allowed"])

    @unittest.skipUnless(V8_PREFINAL.exists(), "exact v8 pre-final image not present")
    def test_exact_v8_profile_accepts_only_the_locked_digest_update(self) -> None:
        profile = PROFILES["imu-touch-v8"]
        candidate = V8_PREFINAL.read_bytes()
        inner = bytearray(candidate[HEADER_SIZE:])
        digest_slice = slice(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32)
        self.assertEqual(inner[digest_slice], profile["old_digest"])
        inner[digest_slice] = profile["new_digest"]

        _finalized, report = finalize_candidate(
            candidate, bytes(inner), profile=profile
        )

        self.assertEqual(report["classification"], "SDK_FINALIZED_IMU_TOUCH_V8_CANDIDATE")
        self.assertEqual(
            report["candidate_sha256"],
            "4b44c8a82f227e6697e7c5dc2633db5ed478f69ca28684b19d7fb17920d08441",
        )
        self.assertEqual(report["touch_indicator_repeat"], 3)
        self.assertTrue(report["imu_patch_present"])
        self.assertFalse(report["flash_allowed"])

    @unittest.skipUnless(V9_PREFINAL.exists(), "exact v9 pre-final image not present")
    def test_exact_v9_profile_accepts_only_the_locked_digest_update(self) -> None:
        profile = PROFILES["imu-touch-v9"]
        candidate = V9_PREFINAL.read_bytes()
        inner = bytearray(candidate[HEADER_SIZE:])
        digest_slice = slice(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32)
        self.assertEqual(inner[digest_slice], profile["old_digest"])
        inner[digest_slice] = profile["new_digest"]

        _finalized, report = finalize_candidate(
            candidate, bytes(inner), profile=profile
        )

        self.assertEqual(report["classification"], "SDK_FINALIZED_IMU_TOUCH_V9_CANDIDATE")
        self.assertEqual(
            report["candidate_sha256"],
            "681dbb3e7a9112fc85b1d8e546717eb5052ae7a7138b117b6dfff75de7eba1f5",
        )
        self.assertEqual(report["activation_marker_status"], "0xFC/0xFE")
        self.assertEqual(report["touch_indicator_repeat"], 3)
        self.assertTrue(report["hid_mouse_reports_blocked"])
        self.assertEqual(report["official_tool_changed_inner_bytes"], 31)
        self.assertFalse(report["flash_allowed"])

    @unittest.skipUnless(V10_PREFINAL.exists(), "exact v10 pre-final image not present")
    def test_exact_v10_profile_accepts_only_the_locked_digest_update(self) -> None:
        profile = PROFILES["imu-touch-v10"]
        candidate = V10_PREFINAL.read_bytes()
        inner = bytearray(candidate[HEADER_SIZE:])
        digest_slice = slice(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32)
        self.assertEqual(inner[digest_slice], profile["old_digest"])
        inner[digest_slice] = profile["new_digest"]

        _finalized, report = finalize_candidate(
            candidate, bytes(inner), profile=profile
        )

        self.assertEqual(report["classification"], "SDK_FINALIZED_IMU_TOUCH_V10_CANDIDATE")
        self.assertEqual(
            report["candidate_sha256"],
            "6cd256de135ce4290794feebec808cdf4cea2e6fd9dfdd30e675a16fcb7927bb",
        )
        self.assertEqual(report["activation_marker_status"], "0xFB/0xFE")
        self.assertEqual(report["touch_indicator_repeat"], 3)
        self.assertTrue(report["touch_wheel_rewritten"])
        self.assertFalse(report["hid_mouse_reports_blocked"])
        self.assertEqual(report["official_tool_changed_inner_bytes"], 32)
        self.assertFalse(report["flash_allowed"])

    @unittest.skipUnless(V11_PREFINAL.exists(), "exact v11 pre-final image not present")
    def test_exact_v11_profile_accepts_only_the_locked_digest_update(self) -> None:
        profile = PROFILES["imu-touch-v11"]
        candidate = V11_PREFINAL.read_bytes()
        inner = bytearray(candidate[HEADER_SIZE:])
        digest_slice = slice(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32)
        self.assertEqual(inner[digest_slice], profile["old_digest"])
        inner[digest_slice] = profile["new_digest"]

        _finalized, report = finalize_candidate(
            candidate, bytes(inner), profile=profile
        )

        self.assertEqual(report["classification"], "SDK_FINALIZED_IMU_TOUCH_V11_CANDIDATE")
        self.assertEqual(
            report["candidate_sha256"],
            "7b60058f5d4de8246834acf139b059009495e0dc9a811b5ff041ec33e3e00e0f",
        )
        self.assertEqual(report["activation_marker_status"], "0xFA/0xFE")
        self.assertEqual(report["touch_indicator_repeat"], 3)
        self.assertTrue(report["touch_wheel_rewritten"])
        self.assertTrue(report["contact_gated_wheel"])
        self.assertEqual(report["minimum_abs_y"], 16)
        self.assertFalse(report["hid_mouse_reports_blocked"])
        self.assertEqual(report["official_tool_changed_inner_bytes"], 32)
        self.assertFalse(report["flash_allowed"])


if __name__ == "__main__":
    unittest.main()
