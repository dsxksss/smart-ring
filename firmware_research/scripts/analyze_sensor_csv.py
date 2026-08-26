#!/usr/bin/env python3
"""Summarize one or more CSV files produced by `r08 sensor-record`."""

from __future__ import annotations

import argparse
import csv
import json
import math
import statistics
from pathlib import Path
from typing import Iterable


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def axis_summary(values: Iterable[int]) -> dict[str, float | int]:
    samples = list(values)
    if not samples:
        return {"min": 0, "max": 0, "range": 0, "mean": 0.0, "stddev": 0.0}
    return {
        "min": min(samples),
        "max": max(samples),
        "range": max(samples) - min(samples),
        "mean": statistics.fmean(samples),
        "stddev": statistics.pstdev(samples),
    }


def analyze_file(path: Path) -> dict[str, object]:
    with path.open("r", encoding="utf-8", newline="") as handle:
        rows = list(csv.DictReader(handle))
    required = {"elapsed_ms", "x", "y", "z"}
    if not rows:
        raise ValueError(f"{path}: CSV 没有样本")
    if not required.issubset(rows[0]):
        missing = ", ".join(sorted(required.difference(rows[0])))
        raise ValueError(f"{path}: 缺少列 {missing}")

    elapsed = [float(row["elapsed_ms"]) for row in rows]
    axes = {
        name: [int(row[name]) for row in rows]
        for name in ("x", "y", "z")
    }
    intervals = [later - earlier for earlier, later in zip(elapsed, elapsed[1:])]
    span_ms = max(0.0, elapsed[-1] - elapsed[0])
    effective_hz = len(intervals) * 1000.0 / span_ms if span_ms > 0 else 0.0
    if effective_hz < 5.0:
        assessment = "采样率过低，不适合连续转动跟随"
    elif effective_hz < 15.0:
        assessment = "采样率偏低，只适合粗粒度转动判断"
    else:
        assessment = "采样率具备候选价值，仍需比较静止与转动数据"

    return {
        "file": str(path),
        "sample_count": len(rows),
        "span_seconds": span_ms / 1000.0,
        "effective_hz": effective_hz,
        "interval_ms": {
            "mean": statistics.fmean(intervals) if intervals else 0.0,
            "min": min(intervals) if intervals else 0.0,
            "p95": percentile(intervals, 0.95),
            "max": max(intervals) if intervals else 0.0,
        },
        "axes": {name: axis_summary(values) for name, values in axes.items()},
        "sampling_rate_assessment": assessment,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="分析 r08 sensor-record 生成的 CSV")
    parser.add_argument("csv", nargs="+", type=Path, help="一个或多个采集 CSV")
    args = parser.parse_args()
    try:
        results = [analyze_file(path) for path in args.csv]
    except (OSError, ValueError, csv.Error) as error:
        parser.error(str(error))
    print(json.dumps(results, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
