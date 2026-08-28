from __future__ import annotations

import hashlib
import struct
import unittest

from analyze_rt08_thumb import HEADER_SIZE, address_to_file_offset, decode_thumb_bl
from build_rt08_imu_stream_candidate import (
    CAVE_ADDRESS,
    CAVE_END,
    BUMPED_GIT_VERSION,
    BUMPED_OUTER_FIRMWARE,
    DEFAULT_STATUS_ADDRESS,
    DEFAULT_STATUS_MARKER,
    EXPECTED_STOCK_INNER_SHA256,
    EXPECTED_STOCK_GIT_VERSION,
    HOOK_ADDRESS,
    HOOK_ORIGINAL,
    HID_MOUSE_REPORT_FUNCTIONS,
    INNER_SHA256_OFFSET,
    INNER_GIT_VERSION_OFFSET,
    TOUCH_INDICATOR_REPEAT_ADDRESS,
    TOUCH_INDICATOR_REPEAT_ORIGINAL,
    TOUCH_V8_GIT_VERSION,
    TOUCH_V8_OUTER_FIRMWARE,
    TOUCH_V9_GIT_VERSION,
    TOUCH_V9_OUTER_FIRMWARE,
    TOUCH_V9_STATUS_MARKER,
    TOUCH_V10_GIT_VERSION,
    TOUCH_V10_OUTER_FIRMWARE,
    TOUCH_V10_STATUS_MARKER,
    TOUCH_V11_GIT_VERSION,
    TOUCH_V11_OUTER_FIRMWARE,
    TOUCH_V11_STATUS_MARKER,
    TOUCH_WHEEL_EXTENDED_ENTRY_ADDRESS,
    TOUCH_WHEEL_EXTENDED_ENTRY_ORIGINAL,
    TOUCH_WHEEL_EXTENDED_ENTRY_REPLACEMENT,
    TOUCH_WHEEL_MOTION_HOOK_ADDRESS,
    TOUCH_WHEEL_MOTION_HOOK_ORIGINAL,
    TOUCH_WHEEL_PATCH_ADDRESS,
    TOUCH_WHEEL_PATCH_ORIGINAL,
    TOUCH_WHEEL_PATCH_SHA256,
    TOUCH_WHEEL_V11_PATCH_SHA256,
    build_candidate,
    validate_patch_binary,
    validate_touch_wheel_patch_binary,
)
from test_analyze_rt08_thumb import synthetic_image


def candidate_fixture() -> bytes:
    minimum_length = address_to_file_offset(CAVE_END - 1, 0x40000) + 1
    data = bytearray(synthetic_image())
    if len(data) < minimum_length:
        payload_growth = minimum_length - len(data)
        data.extend(b"\0" * payload_growth)
        payload_length = len(data) - HEADER_SIZE
        struct.pack_into("<I", data, HEADER_SIZE + 8, payload_length - 0x400)
        struct.pack_into("<II", data, 4, payload_length, payload_length)
    hook_offset = address_to_file_offset(HOOK_ADDRESS, len(data))
    data[hook_offset : hook_offset + 4] = HOOK_ORIGINAL
    data[INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32] = (
        EXPECTED_STOCK_INNER_SHA256
    )
    touch_offset = address_to_file_offset(TOUCH_INDICATOR_REPEAT_ADDRESS, len(data))
    data[
        touch_offset : touch_offset + len(TOUCH_INDICATOR_REPEAT_ORIGINAL)
    ] = TOUCH_INDICATOR_REPEAT_ORIGINAL
    for address, original, _replacement in HID_MOUSE_REPORT_FUNCTIONS:
        offset = address_to_file_offset(address, len(data))
        data[offset : offset + len(original)] = original
    motion_hook_offset = address_to_file_offset(
        TOUCH_WHEEL_MOTION_HOOK_ADDRESS, len(data)
    )
    data[
        motion_hook_offset : motion_hook_offset + len(TOUCH_WHEEL_MOTION_HOOK_ORIGINAL)
    ] = TOUCH_WHEEL_MOTION_HOOK_ORIGINAL
    extended_entry_offset = address_to_file_offset(
        TOUCH_WHEEL_EXTENDED_ENTRY_ADDRESS, len(data)
    )
    data[
        extended_entry_offset : extended_entry_offset
        + len(TOUCH_WHEEL_EXTENDED_ENTRY_ORIGINAL)
    ] = TOUCH_WHEEL_EXTENDED_ENTRY_ORIGINAL
    wheel_patch_offset = address_to_file_offset(TOUCH_WHEEL_PATCH_ADDRESS, len(data))
    data[
        wheel_patch_offset : wheel_patch_offset + len(TOUCH_WHEEL_PATCH_ORIGINAL)
    ] = TOUCH_WHEEL_PATCH_ORIGINAL
    struct.pack_into("<I", data, 12, sum(data[HEADER_SIZE:]) & 0xFFFFFFFF)
    return bytes(data)


REVIEWED_TOUCH_WHEEL_PATCH = bytes.fromhex(
    "00 2a 05 d0 02 dc 01 23 5b 42 02 e0 01 23 00 e0 "
    "00 23 00 20 00 21 00 22 6c 46 20 72 70 47"
)
REVIEWED_TOUCH_WHEEL_V11_PATCH = bytes.fromhex(
    "01 28 0b d1 10 2a 07 da 10 24 64 42 a2 42 00 dd "
    "04 e0 01 23 5b 42 02 e0 01 23 00 e0 00 23 00 20 "
    "00 21 00 22 6c 46 20 72 70 47"
)


class ImuStreamCandidateBuilderTests(unittest.TestCase):
    def test_patches_hook_and_cave_and_updates_outer_sum(self) -> None:
        stock = candidate_fixture()
        patch = bytes(range(32))
        candidate, report = build_candidate(
            stock, patch, enforce_stock_hash=False, validate_patch=False
        )
        hook_offset = address_to_file_offset(HOOK_ADDRESS, len(candidate))
        first, second = struct.unpack_from("<HH", candidate, hook_offset)
        self.assertEqual(decode_thumb_bl(HOOK_ADDRESS, first, second), CAVE_ADDRESS)
        cave_offset = address_to_file_offset(CAVE_ADDRESS, len(candidate))
        self.assertEqual(candidate[cave_offset : cave_offset + len(patch)], patch)
        self.assertTrue(report["outer_sum32_valid"])
        self.assertEqual(report["unplanned_differences"], 0)
        self.assertGreater(report["changed_byte_count"], 0)
        self.assertFalse(report["flash_allowed"])

    def test_optional_revision_bump_is_exact_and_reported(self) -> None:
        stock = bytearray(candidate_fixture())
        struct.pack_into("<I", stock, INNER_GIT_VERSION_OFFSET, EXPECTED_STOCK_GIT_VERSION)
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)
        candidate, report = build_candidate(
            bytes(stock),
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            bump_internal_revision=True,
        )
        self.assertEqual(
            struct.unpack_from("<I", candidate, INNER_GIT_VERSION_OFFSET)[0],
            BUMPED_GIT_VERSION,
        )
        self.assertEqual(report["internal_version"]["candidate_semantic"], "1.4.6")
        self.assertTrue(report["internal_version"]["bumped_for_bank_selection"])
        self.assertEqual(report["unplanned_differences"], 0)

    def test_optional_activation_markers_are_exact_and_reported(self) -> None:
        stock = bytearray(candidate_fixture())
        stock[0x10 : 0x10 + len(BUMPED_OUTER_FIRMWARE)] = b"RT08_3.10.48_260309"
        marker_offset = address_to_file_offset(DEFAULT_STATUS_ADDRESS, len(stock))
        stock[marker_offset : marker_offset + 2] = bytes.fromhex("ff 21")
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)
        candidate, report = build_candidate(
            bytes(stock),
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            bump_outer_revision=True,
            add_activation_marker=True,
        )
        self.assertEqual(
            candidate[0x10 : 0x10 + len(BUMPED_OUTER_FIRMWARE)],
            BUMPED_OUTER_FIRMWARE,
        )
        self.assertEqual(
            candidate[marker_offset : marker_offset + 2], DEFAULT_STATUS_MARKER
        )
        self.assertTrue(report["outer_firmware_revision"]["bumped"])
        self.assertTrue(report["activation_marker"]["enabled"])
        self.assertEqual(report["unplanned_differences"], 0)

    def test_optional_touch_indicator_repeat_is_exact_and_reported(self) -> None:
        stock = candidate_fixture()
        candidate, report = build_candidate(
            stock,
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            touch_indicator_repeat=3,
        )
        touch_offset = address_to_file_offset(
            TOUCH_INDICATOR_REPEAT_ADDRESS, len(candidate)
        )
        self.assertEqual(
            candidate[touch_offset : touch_offset + 2], bytes.fromhex("03 23")
        )
        self.assertTrue(report["touch_indicator"]["patched"])
        self.assertEqual(report["touch_indicator"]["candidate_repeat"], 3)
        self.assertTrue(report["touch_indicator"]["optical_sensor_led_untouched"])
        self.assertEqual(report["unplanned_differences"], 0)

    def test_touch_v8_revision_profile_is_newer_than_installed_v7(self) -> None:
        stock = bytearray(candidate_fixture())
        struct.pack_into("<I", stock, INNER_GIT_VERSION_OFFSET, EXPECTED_STOCK_GIT_VERSION)
        stock[0x10 : 0x10 + len(TOUCH_V8_OUTER_FIRMWARE)] = b"RT08_3.10.48_260309"
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)
        candidate, report = build_candidate(
            bytes(stock),
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            bump_internal_revision=True,
            bump_outer_revision=True,
            touch_indicator_repeat=3,
            revision_profile="imu-touch-v8",
        )
        self.assertEqual(
            struct.unpack_from("<I", candidate, INNER_GIT_VERSION_OFFSET)[0],
            TOUCH_V8_GIT_VERSION,
        )
        self.assertEqual(
            candidate[0x10 : 0x10 + len(TOUCH_V8_OUTER_FIRMWARE)],
            TOUCH_V8_OUTER_FIRMWARE,
        )
        self.assertEqual(report["internal_version"]["candidate_semantic"], "1.4.7")
        self.assertEqual(report["internal_version"]["profile"], "imu-touch-v8")
        self.assertEqual(report["unplanned_differences"], 0)

    def test_touch_v9_blocks_only_reviewed_hid_mouse_report_helpers(self) -> None:
        stock = bytearray(candidate_fixture())
        struct.pack_into("<I", stock, INNER_GIT_VERSION_OFFSET, EXPECTED_STOCK_GIT_VERSION)
        stock[0x10 : 0x10 + len(TOUCH_V9_OUTER_FIRMWARE)] = b"RT08_3.10.48_260309"
        marker_offset = address_to_file_offset(DEFAULT_STATUS_ADDRESS, len(stock))
        stock[marker_offset : marker_offset + 2] = bytes.fromhex("ff 21")
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)

        candidate, report = build_candidate(
            bytes(stock),
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            bump_internal_revision=True,
            bump_outer_revision=True,
            add_activation_marker=True,
            touch_indicator_repeat=3,
            block_hid_mouse_reports=True,
            revision_profile="imu-touch-v9",
        )

        self.assertEqual(
            struct.unpack_from("<I", candidate, INNER_GIT_VERSION_OFFSET)[0],
            TOUCH_V9_GIT_VERSION,
        )
        self.assertEqual(
            candidate[0x10 : 0x10 + len(TOUCH_V9_OUTER_FIRMWARE)],
            TOUCH_V9_OUTER_FIRMWARE,
        )
        self.assertEqual(
            candidate[marker_offset : marker_offset + 2], TOUCH_V9_STATUS_MARKER
        )
        for address, _original, replacement in HID_MOUSE_REPORT_FUNCTIONS:
            offset = address_to_file_offset(address, len(candidate))
            self.assertEqual(
                candidate[offset : offset + len(replacement)], replacement
            )
        self.assertEqual(report["internal_version"]["candidate_semantic"], "1.4.8")
        self.assertEqual(report["activation_marker"]["unknown_a1_status"], "0xFC")
        self.assertTrue(report["hid_mouse_reports"]["blocked"])
        self.assertTrue(
            report["hid_mouse_reports"]["keyboard_attribute_index_0x18_untouched"]
        )
        self.assertEqual(report["unplanned_differences"], 0)

    def test_hid_mouse_block_requires_v9_profile(self) -> None:
        with self.assertRaisesRegex(ValueError, "requires the imu-touch-v9"):
            build_candidate(
                candidate_fixture(),
                bytes(range(32)),
                enforce_stock_hash=False,
                validate_patch=False,
                block_hid_mouse_reports=True,
                revision_profile="imu-touch-v8",
            )

    def test_touch_v10_rewrites_only_y_motion_to_native_wheel(self) -> None:
        self.assertEqual(
            hashlib.sha256(REVIEWED_TOUCH_WHEEL_PATCH).hexdigest(),
            TOUCH_WHEEL_PATCH_SHA256,
        )
        stock = bytearray(candidate_fixture())
        struct.pack_into("<I", stock, INNER_GIT_VERSION_OFFSET, EXPECTED_STOCK_GIT_VERSION)
        stock[0x10 : 0x10 + len(TOUCH_V10_OUTER_FIRMWARE)] = (
            b"RT08_3.10.48_260309"
        )
        marker_offset = address_to_file_offset(DEFAULT_STATUS_ADDRESS, len(stock))
        stock[marker_offset : marker_offset + 2] = bytes.fromhex("ff 21")
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)

        candidate, report = build_candidate(
            bytes(stock),
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            bump_internal_revision=True,
            bump_outer_revision=True,
            add_activation_marker=True,
            touch_indicator_repeat=3,
            touch_wheel_patch=REVIEWED_TOUCH_WHEEL_PATCH,
            revision_profile="imu-touch-v10",
        )

        self.assertEqual(
            struct.unpack_from("<I", candidate, INNER_GIT_VERSION_OFFSET)[0],
            TOUCH_V10_GIT_VERSION,
        )
        self.assertEqual(
            candidate[0x10 : 0x10 + len(TOUCH_V10_OUTER_FIRMWARE)],
            TOUCH_V10_OUTER_FIRMWARE,
        )
        self.assertEqual(
            candidate[marker_offset : marker_offset + 2], TOUCH_V10_STATUS_MARKER
        )
        motion_hook_offset = address_to_file_offset(
            TOUCH_WHEEL_MOTION_HOOK_ADDRESS, len(candidate)
        )
        first, second = struct.unpack_from("<HH", candidate, motion_hook_offset)
        self.assertEqual(
            decode_thumb_bl(TOUCH_WHEEL_MOTION_HOOK_ADDRESS, first, second),
            TOUCH_WHEEL_PATCH_ADDRESS,
        )
        extended_entry_offset = address_to_file_offset(
            TOUCH_WHEEL_EXTENDED_ENTRY_ADDRESS, len(candidate)
        )
        self.assertEqual(
            candidate[
                extended_entry_offset : extended_entry_offset
                + len(TOUCH_WHEEL_EXTENDED_ENTRY_REPLACEMENT)
            ],
            TOUCH_WHEEL_EXTENDED_ENTRY_REPLACEMENT,
        )
        wheel_patch_offset = address_to_file_offset(
            TOUCH_WHEEL_PATCH_ADDRESS, len(candidate)
        )
        self.assertEqual(
            candidate[
                wheel_patch_offset : wheel_patch_offset + len(REVIEWED_TOUCH_WHEEL_PATCH)
            ],
            REVIEWED_TOUCH_WHEEL_PATCH,
        )
        self.assertEqual(report["internal_version"]["candidate_semantic"], "1.4.9")
        self.assertEqual(report["activation_marker"]["unknown_a1_status"], "0xFB")
        self.assertTrue(report["touch_wheel_rewrite"]["enabled"])
        self.assertTrue(report["touch_wheel_rewrite"]["pointer_axes_forced_zero"])
        self.assertTrue(report["touch_wheel_rewrite"]["mouse_buttons_forced_zero"])
        self.assertFalse(report["hid_mouse_reports"]["blocked"])
        self.assertEqual(report["unplanned_differences"], 0)

    def test_touch_wheel_rewrite_requires_v10_profile(self) -> None:
        with self.assertRaisesRegex(ValueError, "requires the imu-touch-v10"):
            build_candidate(
                candidate_fixture(),
                bytes(range(32)),
                enforce_stock_hash=False,
                validate_patch=False,
                touch_wheel_patch=REVIEWED_TOUCH_WHEEL_PATCH,
                revision_profile="imu-touch-v9",
            )

    def test_touch_v11_requires_contact_and_large_y_motion(self) -> None:
        self.assertEqual(
            hashlib.sha256(REVIEWED_TOUCH_WHEEL_V11_PATCH).hexdigest(),
            TOUCH_WHEEL_V11_PATCH_SHA256,
        )
        stock = bytearray(candidate_fixture())
        struct.pack_into("<I", stock, INNER_GIT_VERSION_OFFSET, EXPECTED_STOCK_GIT_VERSION)
        stock[0x10 : 0x10 + len(TOUCH_V11_OUTER_FIRMWARE)] = (
            b"RT08_3.10.48_260309"
        )
        marker_offset = address_to_file_offset(DEFAULT_STATUS_ADDRESS, len(stock))
        stock[marker_offset : marker_offset + 2] = bytes.fromhex("ff 21")
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)

        candidate, report = build_candidate(
            bytes(stock),
            bytes(range(32)),
            enforce_stock_hash=False,
            validate_patch=False,
            bump_internal_revision=True,
            bump_outer_revision=True,
            add_activation_marker=True,
            touch_indicator_repeat=3,
            touch_wheel_patch=REVIEWED_TOUCH_WHEEL_V11_PATCH,
            revision_profile="imu-touch-v11",
        )

        self.assertEqual(
            struct.unpack_from("<I", candidate, INNER_GIT_VERSION_OFFSET)[0],
            TOUCH_V11_GIT_VERSION,
        )
        self.assertEqual(
            candidate[0x10 : 0x10 + len(TOUCH_V11_OUTER_FIRMWARE)],
            TOUCH_V11_OUTER_FIRMWARE,
        )
        self.assertEqual(
            candidate[marker_offset : marker_offset + 2], TOUCH_V11_STATUS_MARKER
        )
        validation = report["touch_wheel_rewrite"]["validation"]
        self.assertEqual(validation["behavior"], "contact-gated-abs-y-16")
        self.assertTrue(validation["contact_flag_required"])
        self.assertEqual(validation["minimum_abs_y"], 16)
        self.assertEqual(report["internal_version"]["candidate_semantic"], "1.5.0")
        self.assertEqual(report["activation_marker"]["unknown_a1_status"], "0xFA")
        self.assertEqual(report["unplanned_differences"], 0)

    def test_v10_requires_exact_reviewed_touch_wheel_patch(self) -> None:
        with self.assertRaisesRegex(ValueError, "requires the reviewed"):
            build_candidate(
                candidate_fixture(),
                bytes(range(32)),
                enforce_stock_hash=False,
                validate_patch=False,
                revision_profile="imu-touch-v10",
            )
        damaged = bytearray(REVIEWED_TOUCH_WHEEL_PATCH)
        damaged[0] ^= 1
        with self.assertRaisesRegex(ValueError, "SHA-256 differs"):
            validate_touch_wheel_patch_binary(bytes(damaged))

    def test_rejects_touch_indicator_repeat_outside_reviewed_range(self) -> None:
        with self.assertRaisesRegex(ValueError, "between 1 and 10"):
            build_candidate(
                candidate_fixture(),
                bytes(range(32)),
                enforce_stock_hash=False,
                validate_patch=False,
                touch_indicator_repeat=0,
            )

    def test_rejects_unexpected_touch_indicator_instruction(self) -> None:
        stock = bytearray(candidate_fixture())
        touch_offset = address_to_file_offset(
            TOUCH_INDICATOR_REPEAT_ADDRESS, len(stock)
        )
        stock[touch_offset] ^= 1
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(ValueError, "repeat instruction"):
            build_candidate(
                bytes(stock),
                bytes(range(32)),
                enforce_stock_hash=False,
                validate_patch=False,
                touch_indicator_repeat=3,
            )

    def test_rejects_oversized_patch(self) -> None:
        with self.assertRaisesRegex(ValueError, "does not fit"):
            build_candidate(
                candidate_fixture(),
                b"x" * (CAVE_END - CAVE_ADDRESS + 1),
                enforce_stock_hash=False,
                validate_patch=False,
            )

    def test_rejects_patch_with_stale_literal_table(self) -> None:
        with self.assertRaisesRegex(ValueError, "exactly match"):
            validate_patch_binary(b"\0" * (CAVE_END - CAVE_ADDRESS))

    def test_rejects_stock_with_unexpected_sdk_digest(self) -> None:
        stock = bytearray(candidate_fixture())
        stock[INNER_SHA256_OFFSET] = 1
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(ValueError, "inner SHA-256 field"):
            build_candidate(
                bytes(stock),
                b"x" * (CAVE_END - CAVE_ADDRESS),
                enforce_stock_hash=False,
                validate_patch=False,
            )


if __name__ == "__main__":
    unittest.main()
