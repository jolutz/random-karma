#!/usr/bin/env python3
"""Extract vehicle and lap-time columns from an exported CSV file."""

from __future__ import annotations

import argparse
import csv
import os
import sys
import tempfile
from pathlib import Path

VEHICLE_HEADER = "Vehicle"
LAP_TIME_HEADER = "Lap Time (m:ss.000)"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="source CSV exported from the game")
    parser.add_argument("output", type=Path, help="destination CSV containing vehicle,lap_time")
    parser.add_argument(
        "--in-place",
        action="store_true",
        help="allow output to replace the input after a successful transformation",
    )
    return parser.parse_args()


def transform(input_path: Path, output_path: Path, allow_in_place: bool) -> int:
    if not input_path.is_file():
        raise ValueError(f"input file does not exist: {input_path}")
    if input_path.resolve() == output_path.resolve() and not allow_in_place:
        raise ValueError("refusing to overwrite input; pass --in-place to allow it")

    with input_path.open("r", encoding="utf-8-sig", newline="") as source:
        rows = list(csv.reader(source))

    header_index = next(
        (
            index
            for index, row in enumerate(rows)
            if VEHICLE_HEADER in row and LAP_TIME_HEADER in row
        ),
        None,
    )
    if header_index is None:
        raise ValueError(
            f"could not find a header containing {VEHICLE_HEADER!r} and {LAP_TIME_HEADER!r}"
        )

    header = rows[header_index]
    vehicle_index = header.index(VEHICLE_HEADER)
    lap_time_index = header.index(LAP_TIME_HEADER)
    extracted = [("vehicle", "lap_time")]

    for row in rows[header_index + 1 :]:
        if len(row) <= max(vehicle_index, lap_time_index):
            continue
        vehicle = row[vehicle_index].strip()
        lap_time = row[lap_time_index].strip()
        if vehicle and lap_time:
            extracted.append((vehicle, lap_time))

    output_path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_path = tempfile.mkstemp(
        dir=output_path.parent,
        prefix=f".{output_path.name}.",
        suffix=".tmp",
        text=True,
    )
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="") as destination:
            csv.writer(destination).writerows(extracted)
        Path(temporary_path).replace(output_path)
    except BaseException:
        Path(temporary_path).unlink(missing_ok=True)
        raise

    return len(extracted) - 1


def main() -> int:
    args = parse_args()
    try:
        count = transform(args.input, args.output, args.in_place)
    except (OSError, ValueError, csv.Error) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    print(f"wrote {count} vehicle rows to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
