#!/usr/bin/env python3
"""Execute the stock RT08 image-info resolver with deterministic ROM stubs.

This is an offline control-flow and dataflow check.  The ROM entry points stay
unnamed until exact RTL8762E SDK symbols are available.  Nothing in this module
connects to a ring, writes flash, builds an OTA packet, or authorizes flashing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from analyze_rt08_thumb import APPLICATION_BASE, HEADER_SIZE, load_image


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
IMAGE_INFO_ENTRY = 0x00826C22
IMAGE_INFO_END = 0x00826C9A
OTA_HEADER_IMAGE_ID = 0x2790
APP_IMAGE_ID = 0x2793
SPECIAL_IMAGE_ID = 0xFFFE
STACK_TOP = 0x0020DF00
DESCRIPTOR_ADDRESS = 0x0020E000
OUTPUT_ADDRESS = 0x0020F000
RETURN_SENTINEL = 0x008FF000

ROM_SPECIAL_RESOLVER = 0x000080B8
ROM_DESCRIPTOR_RESOLVER = 0x00008AE2
APP_LOG_WRAPPER = 0x00005AA8

OTA_HEADER_DESCRIPTOR_FIELD_OFFSET = 0x194
APPLICATION_DESCRIPTOR_FIELD_OFFSET = 0x60


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
class ImageInfoState:
    descriptor_result: int = 0
    special_result: int = 0
    calls: list[dict[str, int]] = field(default_factory=list)


class StockImageInfoEmulator:
    def __init__(self, stock: bytes, state: ImageInfoState) -> None:
        digest = hashlib.sha256(stock).hexdigest()
        if digest != EXPECTED_STOCK_SHA256:
            raise ValueError(f"stock SHA-256 mismatch: {digest}")
        unicorn, arm_const = _load_unicorn()
        self._arm = arm_const
        self.state = state
        self.uc = unicorn.Uc(
            unicorn.UC_ARCH_ARM,
            unicorn.UC_MODE_THUMB | unicorn.UC_MODE_MCLASS,
        )
        self.uc.mem_map(0x00000000, 0x00010000)
        self.uc.mem_map(0x00200000, 0x00010000)
        self.uc.mem_map(0x00800000, 0x00100000)
        self.uc.mem_write(APPLICATION_BASE, stock[HEADER_SIZE:])
        self.uc.mem_write(RETURN_SENTINEL, b"\x00\xbf")
        self._stubs: dict[int, Callable[[], None]] = {
            ROM_SPECIAL_RESOLVER: self._stub_special_resolver,
            ROM_DESCRIPTOR_RESOLVER: self._stub_descriptor_resolver,
            APP_LOG_WRAPPER: self._stub_log,
        }
        self.uc.hook_add(unicorn.UC_HOOK_CODE, self._on_code)

    def _reg(self, name: str) -> int:
        return self.uc.reg_read(getattr(self._arm, f"UC_ARM_REG_{name.upper()}"))

    def _set_reg(self, name: str, value: int) -> None:
        self.uc.reg_write(
            getattr(self._arm, f"UC_ARM_REG_{name.upper()}"), value & 0xFFFFFFFF
        )

    def _return(self, value: int | None = None) -> None:
        if value is not None:
            self._set_reg("r0", value)
        self._set_reg("pc", self._reg("lr"))

    def _record(self, name: str, argument: int) -> None:
        self.state.calls.append({"name": name, "r0": argument & 0xFFFFFFFF})

    def _stub_special_resolver(self) -> None:
        self._record("special_resolver", self._reg("r0"))
        self._return(self.state.special_result)

    def _stub_descriptor_resolver(self) -> None:
        self._record("descriptor_resolver", self._reg("r0"))
        self._return(self.state.descriptor_result)

    def _stub_log(self) -> None:
        self._record("log_wrapper", self._reg("r0"))
        self._return(0)

    def _on_code(self, _uc: Any, address: int, _size: int, _user_data: Any) -> None:
        normalized = address & ~1
        if normalized == RETURN_SENTINEL:
            self.uc.emu_stop()
            return
        stub = self._stubs.get(normalized)
        if stub is not None:
            stub()

    def run(
        self,
        image_id: int,
        *,
        first_output: int = OUTPUT_ADDRESS,
        second_output: int = OUTPUT_ADDRESS + 4,
        ota_header_field: int = 0x11111111,
        application_field: int = 0x22222222,
    ) -> dict[str, int | None]:
        for index in range(13):
            self._set_reg(f"r{index}", 0)
        self._set_reg("sp", STACK_TOP)
        self._set_reg("lr", RETURN_SENTINEL | 1)
        self._set_reg("r0", image_id)
        self._set_reg("r1", first_output)
        self._set_reg("r2", second_output)
        self.uc.mem_write(
            DESCRIPTOR_ADDRESS + OTA_HEADER_DESCRIPTOR_FIELD_OFFSET,
            ota_header_field.to_bytes(4, "little"),
        )
        self.uc.mem_write(
            DESCRIPTOR_ADDRESS + APPLICATION_DESCRIPTOR_FIELD_OFFSET,
            application_field.to_bytes(4, "little"),
        )
        if first_output:
            self.uc.mem_write(first_output, b"\xCC" * 4)
        if second_output:
            self.uc.mem_write(second_output, b"\xCC" * 4)
        self.uc.emu_start(IMAGE_INFO_ENTRY | 1, 0, count=1000)
        return {
            "return_value": self._reg("r0"),
            "first_output": (
                int.from_bytes(self.uc.mem_read(first_output, 4), "little")
                if first_output
                else None
            ),
            "second_output": (
                int.from_bytes(self.uc.mem_read(second_output, 4), "little")
                if second_output
                else None
            ),
        }


def _scenario(
    stock: bytes,
    name: str,
    *,
    image_id: int,
    descriptor_result: int = DESCRIPTOR_ADDRESS,
    special_result: int = DESCRIPTOR_ADDRESS,
    first_output: int = OUTPUT_ADDRESS,
    second_output: int = OUTPUT_ADDRESS + 4,
) -> dict[str, Any]:
    state = ImageInfoState(
        descriptor_result=descriptor_result,
        special_result=special_result,
    )
    outputs = StockImageInfoEmulator(stock, state).run(
        image_id,
        first_output=first_output,
        second_output=second_output,
    )
    return {
        "name": name,
        "image_id": f"0x{image_id:04X}",
        **outputs,
        "calls": state.calls,
    }


def validate_image_info(stock: bytes) -> dict[str, Any]:
    scenarios = [
        _scenario(stock, "ota_header", image_id=OTA_HEADER_IMAGE_ID),
        _scenario(stock, "application", image_id=APP_IMAGE_ID),
        _scenario(stock, "special", image_id=SPECIAL_IMAGE_ID),
        _scenario(stock, "resolver_failure", image_id=APP_IMAGE_ID, descriptor_result=0),
        _scenario(stock, "invalid_image_id", image_id=0x279B),
        _scenario(stock, "null_first_output", image_id=APP_IMAGE_ID, first_output=0),
        _scenario(stock, "null_second_output", image_id=APP_IMAGE_ID, second_output=0),
    ]
    by_name = {scenario["name"]: scenario for scenario in scenarios}

    assert by_name["ota_header"]["return_value"] == 0
    assert by_name["ota_header"]["first_output"] == 0x11111111
    assert by_name["ota_header"]["second_output"] == 0
    assert by_name["application"]["return_value"] == 0
    assert by_name["application"]["first_output"] == 0x22222222
    assert by_name["application"]["second_output"] == 0
    assert by_name["special"]["return_value"] == 0
    assert by_name["special"]["first_output"] == 0x22222222
    assert by_name["resolver_failure"]["return_value"] != 0
    assert by_name["invalid_image_id"]["return_value"] != 0
    assert by_name["null_first_output"]["return_value"] != 0
    assert by_name["null_second_output"]["return_value"] != 0

    start = HEADER_SIZE + IMAGE_INFO_ENTRY - APPLICATION_BASE
    end = HEADER_SIZE + IMAGE_INFO_END - APPLICATION_BASE
    return {
        "classification": "INSTRUCTION_LEVEL_STOCK_IMAGE_INFO_DATAFLOW",
        "stock_sha256": hashlib.sha256(stock).hexdigest(),
        "function_entry": f"0x{IMAGE_INFO_ENTRY:08X}",
        "function_bytes_sha256": hashlib.sha256(stock[start:end]).hexdigest(),
        "scenario_count": len(scenarios),
        "scenarios": scenarios,
        "observed_descriptor_field_offsets": {
            "ota_header_0x2790": f"0x{OTA_HEADER_DESCRIPTOR_FIELD_OFFSET:X}",
            "image_ids_0x2791_through_0x279A_and_0xFFFE": (
                f"0x{APPLICATION_DESCRIPTOR_FIELD_OFFSET:X}"
            ),
        },
        "ota_header_and_application_use_distinct_descriptor_fields": True,
        "ota_header_field_semantics_proven": False,
        "application_field_semantics_proven": False,
        "rom_api_names_proven": False,
        "installed_ota_header_readback_proven": False,
        "bank_selection_proven": False,
        "runtime_rollback_proven": False,
        "flash_authorized": False,
        "safety_note": (
            "The exact stock instructions prove only image-ID branching and field "
            "offset dataflow. The ROM resolver, descriptor fields, installed bank "
            "state, activation side effects, and rollback behavior remain unresolved."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Instruction-level offline checks for stock RT08 image-info logic"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    if not args.image.is_file():
        parser.error(f"file not found: {args.image}")
    try:
        report = validate_image_info(load_image(args.image))
    except (AssertionError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
