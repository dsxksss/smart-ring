#!/usr/bin/env python3
"""Instruction-level safety checks for the non-flashable R08 IMU patch.

The patch executes as Cortex-M0 Thumb code. Calls into the stock firmware are
replaced with deterministic contract stubs so control flow, RAM mutations,
packet layout, watchdog behavior, and teardown can be checked without a ring.
This is deliberately not a device or flashing tool.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable


PATCH_BASE = 0x00849B08
CONTROL_ENTRY = PATCH_BASE
STOP_ENTRY = 0x00849B54
TICK_ENTRY = 0x00849B74
PATCH_SHA256 = "0aeb8f7fd8ed84e642b38dadfa578d0185fd3aee96a55554ce2798c9a0faec0a"

A1_STATE = 0x00209CB8
A1_TIMER = 0x00209CC8
RING_STATE = 0x0020BFA0
COMMAND = 0x0020E000
STACK_TOP = 0x0020FF00
RETURN_SENTINEL = 0x008FF000

STOCK_A1_EPILOGUE = 0x00828114
FN_TIMER_START = 0x00829F18
FN_TIMER_STOP = 0x00829F44
FN_SENSOR_25HZ = 0x00832D06
FN_FIFO_STOP = 0x00832CBC
FN_SENSOR_STANDBY = 0x008335FC
FN_RING_LATEST = 0x0083394E
FN_CHECKSUM = 0x0082AC00
FN_CONNECTED = 0x0082DCFE
FN_NOTIFY = 0x0082E974


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _load_unicorn() -> tuple[Any, Any]:
    try:
        import unicorn
        from unicorn import arm_const
    except ImportError as error:  # pragma: no cover - exercised by CLI users
        raise RuntimeError(
            "Unicorn is required; install firmware_research/requirements-analysis.txt"
        ) from error
    return unicorn, arm_const


@dataclass
class StubState:
    timer_start_result: int = 1
    connected: bool = True
    ring_fresh: bool = True
    xyz: tuple[int, int, int] = (-321, 654, -987)
    calls: list[str] = field(default_factory=list)
    timer_start_args: tuple[int, int, int, int] | None = None
    notifications: list[bytes] = field(default_factory=list)
    epilogue_status: int | None = None


class PatchEmulator:
    """Small deterministic machine around one reviewed patch binary."""

    def __init__(self, patch: bytes, state: StubState | None = None) -> None:
        if sha256_hex(patch) != PATCH_SHA256:
            raise ValueError("patch hash differs from the instruction-reviewed object")
        unicorn, arm_const = _load_unicorn()
        self._unicorn = unicorn
        self._arm = arm_const
        self.state = state or StubState()
        self.uc = unicorn.Uc(
            unicorn.UC_ARCH_ARM,
            unicorn.UC_MODE_THUMB | unicorn.UC_MODE_MCLASS,
        )
        self.uc.mem_map(0x00800000, 0x00100000)
        self.uc.mem_map(0x00200000, 0x00010000)
        self.uc.mem_write(PATCH_BASE, patch)
        self.uc.mem_write(RETURN_SENTINEL, b"\x00\xbf")
        self._stubs: dict[int, Callable[[], None]] = {
            STOCK_A1_EPILOGUE: self._stub_epilogue,
            FN_TIMER_START: self._stub_timer_start,
            FN_TIMER_STOP: lambda: self._record_and_return("timer_stop"),
            FN_SENSOR_25HZ: lambda: self._record_and_return("sensor_25hz"),
            FN_FIFO_STOP: lambda: self._record_and_return("fifo_stop"),
            FN_SENSOR_STANDBY: lambda: self._record_and_return("sensor_standby"),
            FN_RING_LATEST: self._stub_ring_latest,
            FN_CHECKSUM: self._stub_checksum,
            FN_CONNECTED: lambda: self._record_and_return(
                "connected", int(self.state.connected)
            ),
            FN_NOTIFY: self._stub_notify,
        }
        self.uc.hook_add(unicorn.UC_HOOK_CODE, self._on_code)

    def _reg(self, name: str) -> int:
        return self.uc.reg_read(getattr(self._arm, f"UC_ARM_REG_{name.upper()}"))

    def _set_reg(self, name: str, value: int) -> None:
        self.uc.reg_write(
            getattr(self._arm, f"UC_ARM_REG_{name.upper()}"), value & 0xFFFFFFFF
        )

    def _return_from_stub(self, r0: int | None = None) -> None:
        if r0 is not None:
            self._set_reg("r0", r0)
        self._set_reg("pc", self._reg("lr"))

    def _record_and_return(self, name: str, r0: int = 0) -> None:
        self.state.calls.append(name)
        if name == "timer_stop":
            handle_address = self._reg("r0")
            if 0x00200000 <= handle_address <= 0x0020FFFC:
                self.write_u32(handle_address, 0)
        self._return_from_stub(r0)

    def _stub_epilogue(self) -> None:
        self.state.calls.append("a1_epilogue")
        self.state.epilogue_status = self._reg("r1") & 0xFF
        self.uc.emu_stop()

    def _stub_timer_start(self) -> None:
        handle_address = self._reg("r0")
        callback = self._reg("r1")
        period_ms = self._reg("r2")
        repeat = self._reg("r3")
        self.state.calls.append("timer_start")
        self.state.timer_start_args = (
            handle_address,
            callback,
            period_ms,
            repeat,
        )
        if self.state.timer_start_result:
            self.write_u32(handle_address, 0x2000F000)
        self._return_from_stub(self.state.timer_start_result)

    def _stub_ring_latest(self) -> None:
        self.state.calls.append("ring_latest")
        for pointer, value in zip(
            (self._reg("r0"), self._reg("r1"), self._reg("r2")),
            self.state.xyz,
        ):
            self.uc.mem_write(pointer, struct.pack("<h", value))
        if self.state.ring_fresh:
            self.write_u16(RING_STATE + 8, (self.read_u16(RING_STATE + 8) + 6) % 0x1EC)
        self._return_from_stub(0)

    def _stub_checksum(self) -> None:
        self.state.calls.append("checksum")
        data = bytes(self.uc.mem_read(self._reg("r0"), self._reg("r1")))
        self._return_from_stub(sum(data) & 0xFF)

    def _stub_notify(self) -> None:
        self.state.calls.append("notify")
        self.state.notifications.append(bytes(self.uc.mem_read(self._reg("r0"), 16)))
        self._return_from_stub(0)

    def _on_code(self, _uc: Any, address: int, _size: int, _user_data: Any) -> None:
        normalized = address & ~1
        if normalized == RETURN_SENTINEL:
            self.uc.emu_stop()
            return
        stub = self._stubs.get(normalized)
        if stub is not None:
            stub()

    def reset_registers(self) -> None:
        for index in range(13):
            self._set_reg(f"r{index}", 0)
        self._set_reg("sp", STACK_TOP)
        self._set_reg("lr", RETURN_SENTINEL | 1)

    def run(self, entry: int, *, instruction_limit: int = 4000) -> None:
        self.reset_registers()
        self.uc.emu_start(entry | 1, 0, count=instruction_limit)

    def run_control(self, subcommand: int) -> None:
        self.reset_registers()
        command = bytearray(16)
        command[0:3] = bytes((0xA1, 0x09, subcommand & 0xFF))
        self.uc.mem_write(COMMAND, bytes(command))
        self._set_reg("r5", COMMAND)
        self._set_reg("r6", 0)
        self._set_reg("r7", A1_STATE)
        self.uc.emu_start(CONTROL_ENTRY | 1, 0, count=4000)

    def read_u8(self, address: int) -> int:
        return self.uc.mem_read(address, 1)[0]

    def read_u16(self, address: int) -> int:
        return struct.unpack("<H", self.uc.mem_read(address, 2))[0]

    def read_u32(self, address: int) -> int:
        return struct.unpack("<I", self.uc.mem_read(address, 4))[0]

    def write_u8(self, address: int, value: int) -> None:
        self.uc.mem_write(address, bytes((value & 0xFF,)))

    def write_u16(self, address: int, value: int) -> None:
        self.uc.mem_write(address, struct.pack("<H", value & 0xFFFF))

    def write_u32(self, address: int, value: int) -> None:
        self.uc.mem_write(address, struct.pack("<I", value & 0xFFFFFFFF))


def _assert_packet(packet: bytes, *, sequence: int, flags: int, xyz: tuple[int, int, int]) -> None:
    assert len(packet) == 16
    assert packet[:4] == bytes((0xA2, 0x10, sequence, flags))
    assert struct.unpack_from("<hhh", packet, 4) == xyz
    assert packet[10:15] == bytes(5)
    assert packet[15] == sum(packet[:15]) & 0xFF


def validate_patch(patch: bytes) -> dict[str, Any]:
    passed: list[str] = []

    passthrough = PatchEmulator(patch)
    passthrough.uc.mem_write(COMMAND, bytes((0xA1, 0x08)) + bytes(14))
    passthrough.write_u8(A1_STATE, 0xAA)
    passthrough.write_u8(A1_STATE + 1, 0xBB)
    passthrough.reset_registers()
    passthrough._set_reg("r5", COMMAND)
    passthrough._set_reg("r6", 0)
    passthrough._set_reg("r7", A1_STATE)
    passthrough.uc.emu_start(CONTROL_ENTRY | 1, 0, count=100)
    assert passthrough.read_u8(A1_STATE) == 0
    assert passthrough.read_u8(A1_STATE + 1) == 0
    assert passthrough.state.calls == []
    passed.append("non_custom_hook_replays_overwritten_stores")

    failed = PatchEmulator(patch, StubState(timer_start_result=0))
    failed.write_u32(A1_STATE, 0xFFFFFFFF)
    failed.write_u32(A1_STATE + 4, 0xFFFFFFFF)
    failed.run_control(1)
    assert failed.state.epilogue_status == 0xFF
    assert failed.read_u8(A1_STATE) == 0
    assert "timer_start" in failed.state.calls
    assert "sensor_25hz" not in failed.state.calls
    assert failed.read_u8(RING_STATE + 1) == 0
    passed.append("timer_start_failure_never_powers_sensor")

    started = PatchEmulator(patch)
    started.write_u16(RING_STATE + 8, 0x012C)
    started.run_control(1)
    assert started.state.timer_start_args == (A1_TIMER, TICK_ENTRY | 1, 100, 1)
    assert started.state.epilogue_status == 0xFF
    assert started.state.calls[:4] == [
        "timer_stop",
        "fifo_stop",
        "sensor_standby",
        "timer_start",
    ]
    assert "sensor_25hz" in started.state.calls
    assert started.read_u8(A1_STATE) == 9
    assert started.read_u16(A1_STATE + 4) == 0x012C
    assert started.read_u8(RING_STATE + 1) == 1
    passed.append("start_orders_teardown_timer_then_sensor")

    fresh = PatchEmulator(patch, StubState(ring_fresh=True))
    fresh.write_u8(A1_STATE, 9)
    fresh.write_u8(A1_STATE + 2, 7)
    fresh.write_u16(A1_STATE + 4, 0x0030)
    fresh.write_u16(RING_STATE + 8, 0x0030)
    fresh.run(TICK_ENTRY)
    assert fresh.state.calls == ["connected", "ring_latest", "checksum", "notify"]
    assert fresh.read_u16(RING_STATE + 8) == 0x0036
    assert fresh.read_u16(A1_STATE + 4) == 0x0036
    assert fresh.read_u8(A1_STATE + 2) == 8
    assert fresh.read_u8(A1_STATE + 3) == 1
    _assert_packet(
        fresh.state.notifications[0], sequence=7, flags=1, xyz=fresh.state.xyz
    )
    passed.append("fresh_sample_notifies_and_advances_sequence")

    stale = PatchEmulator(patch, StubState(ring_fresh=False))
    stale.write_u8(A1_STATE, 9)
    stale.write_u8(A1_STATE + 2, 4)
    stale.write_u16(A1_STATE + 4, 0x0042)
    stale.write_u16(RING_STATE + 8, 0x0042)
    stale.write_u8(RING_STATE + 1, 1)
    stale.run(TICK_ENTRY)
    _assert_packet(
        stale.state.notifications[0], sequence=4, flags=2, xyz=stale.state.xyz
    )
    assert stale.read_u32(A1_STATE) == 0
    assert stale.read_u32(A1_STATE + 4) == 0
    assert stale.read_u8(RING_STATE + 1) == 0
    assert stale.state.calls[-3:] == ["timer_stop", "fifo_stop", "sensor_standby"]
    passed.append("stale_sample_notifies_once_then_fully_stops")

    watchdog = PatchEmulator(patch, StubState(ring_fresh=True))
    watchdog.write_u8(A1_STATE, 9)
    watchdog.write_u8(A1_STATE + 2, 11)
    watchdog.write_u8(A1_STATE + 3, 119)
    watchdog.write_u16(A1_STATE + 4, 0x0050)
    watchdog.write_u16(RING_STATE + 8, 0x0050)
    watchdog.write_u8(RING_STATE + 1, 1)
    watchdog.run(TICK_ENTRY)
    _assert_packet(
        watchdog.state.notifications[0], sequence=11, flags=1, xyz=watchdog.state.xyz
    )
    assert watchdog.read_u32(A1_STATE) == 0
    assert watchdog.read_u8(RING_STATE + 1) == 0
    assert watchdog.state.calls[-3:] == ["timer_stop", "fifo_stop", "sensor_standby"]
    passed.append("tick_120_enforces_full_hard_stop")

    stopped = PatchEmulator(patch)
    stopped.write_u32(A1_STATE, 0xDEADBEEF)
    stopped.write_u32(A1_STATE + 4, 0xA5A5A5A5)
    stopped.write_u8(RING_STATE + 1, 1)
    stopped.run_control(0)
    assert stopped.read_u32(A1_STATE) == 0
    assert stopped.read_u32(A1_STATE + 4) == 0
    assert stopped.read_u8(RING_STATE + 1) == 0
    assert stopped.state.calls[:3] == ["timer_stop", "fifo_stop", "sensor_standby"]
    passed.append("explicit_stop_is_complete_and_idempotent")

    disconnected = PatchEmulator(patch, StubState(connected=False))
    disconnected.write_u8(A1_STATE, 9)
    disconnected.write_u8(RING_STATE + 1, 1)
    disconnected.run(TICK_ENTRY)
    assert disconnected.state.notifications == []
    assert disconnected.read_u32(A1_STATE) == 0
    assert disconnected.read_u8(RING_STATE + 1) == 0
    assert disconnected.state.calls == [
        "connected",
        "timer_stop",
        "fifo_stop",
        "sensor_standby",
    ]
    passed.append("disconnect_immediately_stops_before_fifo_or_notify")

    inactive = PatchEmulator(patch)
    inactive.run(TICK_ENTRY)
    assert inactive.state.calls == []
    assert inactive.state.notifications == []
    passed.append("inactive_tick_has_no_side_effects")

    return {
        "classification": "OFFLINE_EMULATION_ONLY",
        "patch_sha256": sha256_hex(patch),
        "architecture": "ARMv6-M Thumb / Cortex-M0 class",
        "passed_scenarios": passed,
        "scenario_count": len(passed),
        "flash_allowed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("patch", type=Path)
    args = parser.parse_args()
    report = validate_patch(args.patch.read_bytes())
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, RuntimeError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
