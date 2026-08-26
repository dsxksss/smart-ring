#!/usr/bin/env python3
"""Read-only address, string-anchor, and Thumb disassembly helper for RT08 OTA images."""

from __future__ import annotations

import argparse
import importlib
import json
import re
import struct
import sys
from pathlib import Path
from typing import Any


CONTAINER_MAGIC = bytes.fromhex("e5 c3 bd 81")
HEADER_SIZE = 0x50
REALTEK_IMAGE_HEADER_SIZE = 0x400
RTL8762E_IC_TYPE = 12
APPLICATION_BASE = 0x00826000
PRINTABLE_PATTERN = re.compile(rb"[\x20-\x7e]{4,}")
NUMERIC_OPERAND_PATTERN = re.compile(r"(?:#)?(0x[0-9a-fA-F]+|\d+)$")
LDR_LITERAL_PATTERN = re.compile(
    r"^[^,]+,\s*\[pc(?:,\s*#(0x[0-9a-fA-F]+|\d+))?\]$"
)


def parse_int(value: str) -> int:
    return int(value, 0)


def load_image(path: Path) -> bytes:
    data = path.read_bytes()
    if len(data) < HEADER_SIZE or data[:4] != CONTAINER_MAGIC:
        raise ValueError("not a recognized QRing 0x50 OTA container")
    length_a, length_b, stored_sum = struct.unpack_from("<III", data, 4)
    payload = data[HEADER_SIZE:]
    if length_a != length_b or length_a != len(payload):
        raise ValueError("container payload lengths do not match")
    if stored_sum != (sum(payload) & 0xFFFFFFFF):
        raise ValueError("container sum32 does not match")
    if b"RT08_V3.1" not in data:
        raise ValueError("image does not contain the exact RT08_V3.1 marker")
    if len(payload) < REALTEK_IMAGE_HEADER_SIZE:
        raise ValueError("payload is shorter than the RTL8762E 1024-byte image header")
    ic_type = payload[0]
    inner_payload_length = struct.unpack_from("<I", payload, 8)[0]
    exe_base = struct.unpack_from("<I", payload, 0x1C)[0]
    image_base_candidate = struct.unpack_from("<I", payload, 0x28)[0]
    if ic_type != RTL8762E_IC_TYPE:
        raise ValueError(f"unexpected Realtek IC type {ic_type}; expected 12 for RTL8762E")
    if inner_payload_length != len(payload) - REALTEK_IMAGE_HEADER_SIZE:
        raise ValueError("RTL8762E inner payload length does not match")
    if image_base_candidate != APPLICATION_BASE:
        raise ValueError(
            f"unexpected RTL8762E image base 0x{image_base_candidate:08x}"
        )
    if exe_base != APPLICATION_BASE + REALTEK_IMAGE_HEADER_SIZE:
        raise ValueError(f"unexpected RTL8762E executable base 0x{exe_base:08x}")
    return data


def address_to_file_offset(address: int, data_length: int) -> int:
    normalized = address & ~1
    offset = HEADER_SIZE + normalized - APPLICATION_BASE
    if offset < HEADER_SIZE or offset >= data_length:
        raise ValueError(f"address 0x{address:08x} is outside the OTA application")
    return offset


def file_offset_to_address(offset: int, data_length: int) -> int:
    if offset < HEADER_SIZE or offset >= data_length:
        raise ValueError(f"file offset 0x{offset:x} is outside the OTA payload")
    return APPLICATION_BASE + offset - HEADER_SIZE


def string_records(data: bytes, terms: list[str]) -> list[dict[str, Any]]:
    lowered_terms = [term.lower() for term in terms]
    application_end = APPLICATION_BASE + len(data) - HEADER_SIZE
    records: list[dict[str, Any]] = []
    for match in PRINTABLE_PATTERN.finditer(data):
        text = match.group().decode("ascii")
        if lowered_terms and not any(term in text.lower() for term in lowered_terms):
            continue
        offset = match.start()
        record: dict[str, Any] = {
            "file_offset": offset,
            "mapped_address": (
                file_offset_to_address(offset, len(data))
                if offset >= HEADER_SIZE
                else None
            ),
            "text": text,
        }
        if offset >= 4:
            preceding = struct.unpack_from("<I", data, offset - 4)[0]
            if (
                preceding & 1
                and APPLICATION_BASE <= (preceding & ~1) < application_end
            ):
                record["preceding_thumb_anchor"] = preceding
                record["preceding_thumb_file_offset"] = address_to_file_offset(
                    preceding, len(data)
                )
        records.append(record)
    return records


def load_disassembler(engine_path: Path | None):
    if engine_path is not None:
        sys.path.insert(0, str(engine_path.resolve()))
    failures: list[str] = []
    for package_name in ("capstone", "csengine"):
        try:
            module = importlib.import_module(package_name)
            return module.Cs(
                module.CS_ARCH_ARM,
                module.CS_MODE_THUMB | module.CS_MODE_LITTLE_ENDIAN,
            )
        except (ImportError, AttributeError, OSError) as error:
            failures.append(f"{package_name}: {error}")
    raise RuntimeError("Thumb disassembler unavailable; " + "; ".join(failures))


def decode_thumb_bl(address: int, first_halfword: int, second_halfword: int) -> int | None:
    """Decode a Thumb-2 BL immediate without treating data as executable code."""
    if first_halfword & 0xF800 != 0xF000 or second_halfword & 0xD000 != 0xD000:
        return None
    sign = (first_halfword >> 10) & 1
    imm10 = first_halfword & 0x03FF
    j1 = (second_halfword >> 13) & 1
    j2 = (second_halfword >> 11) & 1
    imm11 = second_halfword & 0x07FF
    i1 = 1 ^ (j1 ^ sign)
    i2 = 1 ^ (j2 ^ sign)
    immediate = (
        (sign << 24)
        | (i1 << 23)
        | (i2 << 22)
        | (imm10 << 12)
        | (imm11 << 1)
    )
    if sign:
        immediate -= 1 << 25
    return (address + 4 + immediate) & 0xFFFFFFFF


def encode_thumb_bl(address: int, target: int) -> bytes:
    """Encode an ARMv6-M compatible Thumb BL immediate."""
    address &= ~1
    target &= ~1
    displacement = target - (address + 4)
    if displacement & 1 or not -(1 << 24) <= displacement < (1 << 24):
        raise ValueError("Thumb BL target is unaligned or out of range")
    immediate = displacement & ((1 << 25) - 1)
    sign = (immediate >> 24) & 1
    i1 = (immediate >> 23) & 1
    i2 = (immediate >> 22) & 1
    j1 = 1 ^ (i1 ^ sign)
    j2 = 1 ^ (i2 ^ sign)
    imm10 = (immediate >> 12) & 0x3FF
    imm11 = (immediate >> 1) & 0x7FF
    first = 0xF000 | (sign << 10) | imm10
    second = 0xD000 | (j1 << 13) | (j2 << 11) | imm11
    return struct.pack("<HH", first, second)


def find_bl_callers(data: bytes, target: int) -> list[dict[str, int | str]]:
    """Find exact Thumb BL immediates targeting an application address."""
    normalized_target = target & ~1
    callers: list[dict[str, int | str]] = []
    for file_offset in range(HEADER_SIZE, len(data) - 3, 2):
        first_halfword, second_halfword = struct.unpack_from("<HH", data, file_offset)
        address = file_offset_to_address(file_offset, len(data))
        decoded_target = decode_thumb_bl(address, first_halfword, second_halfword)
        if decoded_target == normalized_target:
            callers.append(
                {
                    "address": address,
                    "file_offset": file_offset,
                    "bytes": data[file_offset : file_offset + 4].hex(" "),
                    "target": decoded_target,
                }
            )
    return callers


def find_thumb_imm8_sites(data: bytes, immediate: int) -> list[dict[str, int | str]]:
    """Find common 16-bit Thumb MOVS/CMP/ADDS/SUBS immediate instructions."""
    if immediate < 0 or immediate > 0xFF:
        raise ValueError("Thumb imm8 search value must be between 0 and 255")
    operation_names = ("movs", "cmp", "adds", "subs")
    sites: list[dict[str, int | str]] = []
    for file_offset in range(HEADER_SIZE, len(data) - 1, 2):
        low_byte, high_byte = data[file_offset : file_offset + 2]
        if low_byte != immediate or not 0x20 <= high_byte <= 0x3F:
            continue
        operation_index = (high_byte - 0x20) // 8
        register = high_byte & 7
        sites.append(
            {
                "address": file_offset_to_address(file_offset, len(data)),
                "file_offset": file_offset,
                "bytes": data[file_offset : file_offset + 2].hex(" "),
                "mnemonic": operation_names[operation_index],
                "register": f"r{register}",
                "immediate": immediate,
            }
        )
    return sites


def find_thumb_ldr_literal_value_sites(
    data: bytes, literal_value: int
) -> list[dict[str, int | str]]:
    """Find 16-bit Thumb LDR literal instructions loading an exact uint32 value.

    This intentionally reports candidates rather than claiming every matching
    halfword is executable code. Callers should confirm each result against the
    surrounding control flow before assigning semantics.
    """
    if literal_value < 0 or literal_value > 0xFFFFFFFF:
        raise ValueError("literal search value must fit in uint32")
    sites: list[dict[str, int | str]] = []
    for file_offset in range(HEADER_SIZE, len(data) - 1, 2):
        halfword = struct.unpack_from("<H", data, file_offset)[0]
        if halfword & 0xF800 != 0x4800:
            continue
        address = file_offset_to_address(file_offset, len(data))
        register = (halfword >> 8) & 7
        immediate = (halfword & 0xFF) * 4
        literal_address = ((address + 4) & ~3) + immediate
        try:
            literal_offset = address_to_file_offset(literal_address, len(data))
        except ValueError:
            continue
        if literal_offset + 4 > len(data):
            continue
        loaded_value = struct.unpack_from("<I", data, literal_offset)[0]
        if loaded_value != literal_value:
            continue
        sites.append(
            {
                "address": address,
                "file_offset": file_offset,
                "bytes": data[file_offset : file_offset + 2].hex(" "),
                "mnemonic": "ldr",
                "register": f"r{register}",
                "immediate": immediate,
                "literal_address": literal_address,
                "literal_file_offset": literal_offset,
                "literal_value": loaded_value,
            }
        )
    return sites


def numeric_operand(operands: str) -> int | None:
    match = NUMERIC_OPERAND_PATTERN.search(operands.strip())
    return int(match.group(1), 0) if match else None


def instruction_annotations(
    data: bytes, address: int, mnemonic: str, operands: str
) -> dict[str, Any]:
    annotations: dict[str, Any] = {}
    if mnemonic in {"b", "b.w", "bl", "blx"}:
        target = numeric_operand(operands)
        if target is not None:
            annotations["branch_target"] = target

    if mnemonic == "ldr":
        match = LDR_LITERAL_PATTERN.match(operands)
        if match:
            immediate = int(match.group(1), 0) if match.group(1) else 0
            literal_address = ((address + 4) & ~3) + immediate
            annotations["literal_address"] = literal_address
            try:
                literal_offset = address_to_file_offset(literal_address, len(data))
            except ValueError:
                annotations["literal_mapped"] = False
            else:
                if literal_offset + 4 <= len(data):
                    literal_value = struct.unpack_from("<I", data, literal_offset)[0]
                    annotations.update(
                        {
                            "literal_mapped": True,
                            "literal_file_offset": literal_offset,
                            "literal_value": literal_value,
                        }
                    )
                    try:
                        pointed_offset = address_to_file_offset(literal_value, len(data))
                    except ValueError:
                        pass
                    else:
                        annotations["literal_points_to_file_offset"] = pointed_offset
    return annotations


def disassemble(
    data: bytes,
    address: int,
    byte_count: int,
    engine_path: Path | None,
) -> list[dict[str, Any]]:
    disassembler = load_disassembler(engine_path)
    normalized = address & ~1
    file_offset = address_to_file_offset(normalized, len(data))
    instructions = []
    for instruction in disassembler.disasm(
        data[file_offset : file_offset + byte_count], normalized
    ):
        record = {
            "address": instruction.address,
            "file_offset": file_offset + instruction.address - normalized,
            "bytes": instruction.bytes.hex(" "),
            "mnemonic": instruction.mnemonic,
            "operands": instruction.op_str,
        }
        record.update(
            instruction_annotations(
                data, instruction.address, instruction.mnemonic, instruction.op_str
            )
        )
        instructions.append(record)
    return instructions


def image_summary(data: bytes) -> dict[str, Any]:
    payload_length = len(data) - HEADER_SIZE
    return {
        "container": "qring-0x50-sum32",
        "file_size": len(data),
        "payload_length": payload_length,
        "application_base": APPLICATION_BASE,
        "application_end": APPLICATION_BASE + payload_length,
        "hardware": "RT08_V3.1",
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only RT08 RTL8762E address and Thumb analysis"
    )
    parser.add_argument("image", type=Path)
    parser.add_argument("--find", action="append", default=[])
    parser.add_argument("--disassemble-address", type=parse_int)
    parser.add_argument("--disassemble-anchor", help="disassemble the pointer before an exact string")
    parser.add_argument("--find-callers", action="append", type=parse_int, default=[])
    parser.add_argument("--find-thumb-imm8", action="append", type=parse_int, default=[])
    parser.add_argument(
        "--find-literal-value", action="append", type=parse_int, default=[]
    )
    parser.add_argument("--bytes", type=parse_int, default=0x100)
    parser.add_argument("--engine-path", type=Path)
    args = parser.parse_args()

    data = load_image(args.image)
    report: dict[str, Any] = {"summary": image_summary(data)}
    if args.find:
        report["strings"] = string_records(data, args.find)
    if args.find_callers:
        report["callers"] = {
            f"0x{target & ~1:08x}": find_bl_callers(data, target)
            for target in args.find_callers
        }
    if args.find_thumb_imm8:
        report["thumb_imm8_sites"] = {
            f"0x{immediate:02x}": find_thumb_imm8_sites(data, immediate)
            for immediate in args.find_thumb_imm8
        }
    if args.find_literal_value:
        report["literal_value_sites"] = {
            f"0x{literal_value:08x}": find_thumb_ldr_literal_value_sites(
                data, literal_value
            )
            for literal_value in args.find_literal_value
        }

    address = args.disassemble_address
    if args.disassemble_anchor:
        anchors = [
            record
            for record in string_records(data, [args.disassemble_anchor])
            if record["text"] == args.disassemble_anchor
            and "preceding_thumb_anchor" in record
        ]
        if not anchors:
            raise ValueError(
                f"no mapped Thumb anchor immediately before {args.disassemble_anchor!r}"
            )
        address = anchors[0]["preceding_thumb_anchor"]
        report["selected_anchor"] = anchors[0]

    if address is not None:
        report["instructions"] = disassemble(
            data, address, args.bytes, args.engine_path
        )

    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
