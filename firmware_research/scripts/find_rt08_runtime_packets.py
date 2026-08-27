#!/usr/bin/env python3
"""Rank RT08 packet-builder candidates near checksum + NUS notify calls.

This is a read-only triage helper. It does not patch an image or talk to a ring.
The result is intentionally labelled as candidates because linear disassembly
cannot by itself prove a function boundary or a runtime data flow.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from analyze_rt08_thumb import (
    disassemble,
    find_bl_callers,
    load_image,
)


CHECKSUM = 0x0082AC00
NUS_NOTIFY = 0x0082E974


def parse_int(value: str) -> int:
    return int(value, 0)


def immediate_value(operands: str) -> int | None:
    marker = "#"
    if marker not in operands:
        return None
    value = operands.rsplit(marker, 1)[1].strip()
    try:
        return int(value, 0)
    except ValueError:
        return None


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Rank checksum/notify packet builders containing an immediate"
    )
    parser.add_argument("image", type=Path)
    parser.add_argument("--head", type=parse_int, required=True)
    parser.add_argument("--look-behind", type=parse_int, default=0x80)
    parser.add_argument("--look-ahead", type=parse_int, default=0x40)
    parser.add_argument("--engine-path", type=Path)
    args = parser.parse_args()

    data = load_image(args.image)
    checksum_calls = find_bl_callers(data, CHECKSUM)
    notify_addresses = {
        int(record["address"]) for record in find_bl_callers(data, NUS_NOTIFY)
    }
    candidates: list[dict[str, object]] = []

    for call in checksum_calls:
        checksum_address = int(call["address"])
        start = max(0x00826000, checksum_address - args.look_behind)
        start &= ~1
        instructions = disassemble(
            data,
            start,
            args.look_behind + args.look_ahead,
            args.engine_path,
        )
        head_sites = [
            item
            for item in instructions
            if item["address"] < checksum_address
            and item["mnemonic"] in {"movs", "cmp", "adds", "subs"}
            and immediate_value(str(item["operands"])) == args.head
        ]
        later_notify = [
            item
            for item in instructions
            if checksum_address < item["address"] <= checksum_address + args.look_ahead
            and item["address"] in notify_addresses
        ]
        if not head_sites or not later_notify:
            continue
        first = max(0, instructions.index(head_sites[-1]) - 8)
        last = min(len(instructions), instructions.index(later_notify[0]) + 5)
        candidates.append(
            {
                "checksum_call": checksum_address,
                "notify_call": later_notify[0]["address"],
                "head_sites": [item["address"] for item in head_sites],
                "window": instructions[first:last],
            }
        )

    print(
        json.dumps(
            {
                "image": str(args.image),
                "head": args.head,
                "candidate_count": len(candidates),
                "candidates": candidates,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
