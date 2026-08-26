#!/usr/bin/env python3
"""Execute the stock R08 activation wrapper with deterministic ROM stubs.

This is an offline control-flow check.  It does not name the ROM functions,
connect to a ring, generate a firmware image, or authorize flashing.
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
ACTIVATION_ENTRY = 0x00826F2A
ACTIVATION_END = 0x00826F88
SPECIAL_IMAGE_ID = 0xFFFE
STACK_TOP = 0x0020FF00
RETURN_SENTINEL = 0x008FF000

ROM_SPECIAL_RESOLVER = 0x000080B8
ROM_IMAGE_ID_RESOLVER = 0x00008B94
ROM_ADDRESS_CLASSIFIER = 0x00008B7A
ROM_ADDRESS_VALIDATOR = 0x00008A5C
APP_COMMIT_WRAPPER = 0x00826F16
APP_LOG_WRAPPER = 0x00005AA8


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
class ActivationState:
    resolver_result: int = 0
    classifier_result: int = 0
    validator_result: int = 0
    special_resolver_result: int = 0
    calls: list[dict[str, int]] = field(default_factory=list)
    validation_addresses: list[int] = field(default_factory=list)
    committed_addresses: list[int] = field(default_factory=list)


class StockActivationEmulator:
    def __init__(self, stock: bytes, state: ActivationState) -> None:
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
            ROM_IMAGE_ID_RESOLVER: self._stub_image_id_resolver,
            ROM_ADDRESS_CLASSIFIER: self._stub_address_classifier,
            ROM_ADDRESS_VALIDATOR: self._stub_address_validator,
            APP_COMMIT_WRAPPER: self._stub_commit,
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
        self._return(self.state.special_resolver_result)

    def _stub_image_id_resolver(self) -> None:
        self._record("image_id_resolver", self._reg("r0"))
        self._return(self.state.resolver_result)

    def _stub_address_classifier(self) -> None:
        self._record("address_classifier", self._reg("r0"))
        self._return(self.state.classifier_result)

    def _stub_address_validator(self) -> None:
        address = self._reg("r0")
        self._record("address_validator", address)
        self.state.validation_addresses.append(address)
        self._return(self.state.validator_result)

    def _stub_commit(self) -> None:
        address = self._reg("r0")
        self._record("commit_wrapper", address)
        self.state.committed_addresses.append(address)
        self._return(0)

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

    def run(self, image_id: int, second_argument: int) -> int:
        for index in range(13):
            self._set_reg(f"r{index}", 0)
        self._set_reg("sp", STACK_TOP)
        self._set_reg("lr", RETURN_SENTINEL | 1)
        self._set_reg("r0", image_id)
        self._set_reg("r1", second_argument)
        self.uc.emu_start(ACTIVATION_ENTRY | 1, 0, count=1000)
        return self._reg("r0")


def _scenario(
    stock: bytes,
    name: str,
    *,
    image_id: int = 0x2793,
    second_argument: int = 0,
    resolver_result: int = 0,
    classifier_result: int = 0,
    validator_result: int = 0,
    special_resolver_result: int = 0,
) -> dict[str, Any]:
    state = ActivationState(
        resolver_result=resolver_result,
        classifier_result=classifier_result,
        validator_result=validator_result,
        special_resolver_result=special_resolver_result,
    )
    returned = StockActivationEmulator(stock, state).run(image_id, second_argument)
    return {
        "name": name,
        "image_id": f"0x{image_id:04X}",
        "second_argument": second_argument,
        "return_value": returned,
        "calls": state.calls,
        "validation_addresses": [f"0x{value:08X}" for value in state.validation_addresses],
        "committed_addresses": [f"0x{value:08X}" for value in state.committed_addresses],
    }


def validate_activation(stock: bytes) -> dict[str, Any]:
    scenarios = [
        _scenario(stock, "resolver_failure", resolver_result=0),
        _scenario(
            stock,
            "validation_failure_after_offset",
            second_argument=0x400,
            resolver_result=0x0084E000,
            classifier_result=0,
            validator_result=0,
        ),
        _scenario(
            stock,
            "validation_success_after_offset",
            second_argument=0x400,
            resolver_result=0x0084E000,
            classifier_result=0,
            validator_result=1,
        ),
        _scenario(
            stock,
            "classifier_bypasses_offset",
            second_argument=0x400,
            resolver_result=0x0084E000,
            classifier_result=1,
            validator_result=1,
        ),
        _scenario(
            stock,
            "special_image_id_path",
            image_id=SPECIAL_IMAGE_ID,
            special_resolver_result=0x00001234,
            classifier_result=1,
            validator_result=1,
        ),
    ]

    by_name = {scenario["name"]: scenario for scenario in scenarios}
    assert by_name["resolver_failure"]["committed_addresses"] == []
    assert by_name["resolver_failure"]["return_value"] == 0
    assert by_name["validation_failure_after_offset"]["validation_addresses"] == [
        "0x0084E400"
    ]
    assert by_name["validation_failure_after_offset"]["committed_addresses"] == []
    assert by_name["validation_success_after_offset"]["committed_addresses"] == [
        "0x0084E400"
    ]
    assert by_name["validation_success_after_offset"]["return_value"] == 1
    assert by_name["classifier_bypasses_offset"]["validation_addresses"] == [
        "0x0084E000"
    ]
    assert by_name["classifier_bypasses_offset"]["committed_addresses"] == [
        "0x0084E000"
    ]
    assert by_name["special_image_id_path"]["committed_addresses"] == [
        "0x01001234"
    ]

    start = HEADER_SIZE + ACTIVATION_ENTRY - APPLICATION_BASE
    end = HEADER_SIZE + ACTIVATION_END - APPLICATION_BASE
    return {
        "classification": "INSTRUCTION_LEVEL_STOCK_ACTIVATION_CONTROL_FLOW",
        "stock_sha256": hashlib.sha256(stock).hexdigest(),
        "activation_entry": f"0x{ACTIVATION_ENTRY:08X}",
        "activation_bytes_sha256": hashlib.sha256(stock[start:end]).hexdigest(),
        "scenario_count": len(scenarios),
        "scenarios": scenarios,
        "resolver_failure_blocks_validation_and_commit": True,
        "validation_failure_blocks_commit": True,
        "validation_success_commits_checked_address": True,
        "second_argument_is_conditional_address_offset": True,
        "rom_api_names_proven": False,
        "application_flag_transition_proven": False,
        "ota_bank_header_update_proven": False,
        "runtime_rollback_proven": False,
        "flash_authorized": False,
        "safety_note": (
            "The exact stock instructions were executed with deterministic ROM stubs. "
            "This proves application-level gating and address dataflow only; it does not "
            "prove the side effects of the unresolved ROM calls or authorize flashing."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Instruction-level offline checks for the stock R08 activation wrapper"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    if not args.image.is_file():
        parser.error(f"file not found: {args.image}")
    try:
        report = validate_activation(load_image(args.image))
    except (AssertionError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
