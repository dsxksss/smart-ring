import unittest

from smart_ring_detector import (
    R08_TOUCH_DISABLE_PACKET,
    R08_TOUCH_ENABLE_PACKET,
    R08_TOUCH_VIDEO_PACKET,
    R08_TOUCH_READ_PACKET,
    R08_TAP_FLUSH_MS,
    WHEEL_DELTA,
    build_smooth_scroll_deltas,
    build_colmi_packet,
    describe_colmi_packet,
    format_packet,
    parse_hex_payload,
)


class HexPayloadTests(unittest.TestCase):
    def test_accepts_common_separators(self) -> None:
        self.assertEqual(parse_hex_payload("02 04"), b"\x02\x04")
        self.assertEqual(parse_hex_payload("0xA1:0x04,04"), b"\xA1\x04\x04")

    def test_rejects_empty_odd_and_non_hex(self) -> None:
        for value in ("", "A", "02 GG"):
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    parse_hex_payload(value)

    def test_formats_uppercase_bytes(self) -> None:
        self.assertEqual(format_packet(b"\x00\xa1\xff"), "00 A1 FF")

    def test_builds_padded_colmi_packet_with_checksum(self) -> None:
        packet = build_colmi_packet(bytes.fromhex("02 04"))
        self.assertEqual(len(packet), 16)
        self.assertEqual(packet.hex(), "02040000000000000000000000000006")

    def test_builds_official_r08_touch_packets(self) -> None:
        self.assertEqual(R08_TOUCH_ENABLE_PACKET.hex(), "3b02000101000000000000000000003f")
        self.assertEqual(R08_TOUCH_VIDEO_PACKET.hex(), "3b020002010000000000000000000040")
        self.assertEqual(R08_TOUCH_DISABLE_PACKET.hex(), "3b02000001000000000000000000003e")
        self.assertEqual(R08_TOUCH_READ_PACKET.hex(), "3b01000000000000000000000000003c")

    def test_describes_observed_r08_notification(self) -> None:
        packet = bytes.fromhex("732a010000000000000000000000009e")
        self.assertIn("R08 未知状态通知 0x2A=1", describe_colmi_packet(packet))

    def test_describes_unsupported_command_response(self) -> None:
        packet = bytes.fromhex("aaee0000000000000000000000000098")
        self.assertIn("未识别或不受支持", describe_colmi_packet(packet))

    def test_describes_observed_remote_event(self) -> None:
        packet = bytes.fromhex("02020000000000000000000000000004")
        self.assertIn("R08 相机/长按事件", describe_colmi_packet(packet))

    def test_describes_music_touch_action(self) -> None:
        packet = bytes.fromhex("1d02000000000000000000000000001f")
        description = describe_colmi_packet(packet)
        self.assertIn("下滑", description)
        self.assertIn("上一项", description)

    def test_tap_window_accepts_observed_slow_triple_click(self) -> None:
        self.assertEqual(R08_TAP_FLUSH_MS, 850)

    def test_smooth_scroll_preserves_total_wheel_distance(self) -> None:
        up = build_smooth_scroll_deltas(1, 2)
        down = build_smooth_scroll_deltas(-1, 3)
        self.assertEqual(len(up), 6)
        self.assertEqual(sum(up), 2 * WHEEL_DELTA)
        self.assertEqual(len(down), 9)
        self.assertEqual(sum(down), -3 * WHEEL_DELTA)

    def test_smooth_scroll_rejects_invalid_values(self) -> None:
        for direction, notches in ((0, 1), (1, 0)):
            with self.subTest(direction=direction, notches=notches):
                with self.assertRaises(ValueError):
                    build_smooth_scroll_deltas(direction, notches)

    def test_describes_r08_touch_status(self) -> None:
        packet = bytes.fromhex("3b01000101000000000000000000003e")
        description = describe_colmi_packet(packet)
        self.assertIn("已开启", description)
        self.assertIn("应用类型=1", description)
        self.assertIn("休眠=1 分钟", description)

    def test_decodes_accelerometer_packet(self) -> None:
        packet = bytes.fromhex("a1031fe4fe88010c000000000000003a")
        description = describe_colmi_packet(packet)
        self.assertIn("X=28", description)
        self.assertIn("Y=500", description)
        self.assertIn("Z=-24", description)


if __name__ == "__main__":
    unittest.main()
