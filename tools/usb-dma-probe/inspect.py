#!/usr/bin/env python3
"""Recompute RXD3 footprints and compare the two bytes after a payload to its USB CRC."""

import argparse
import json
import sys
from pathlib import Path


def usb_crc(data):
    """USB data CRC in wire byte order (USB 2.0, section 8.3.5.2)."""
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0xA001 if crc & 1 else 0)
    return (crc ^ 0xFFFF).to_bytes(2, "little")


def inspect(directory):
    report = json.loads((directory / "report.json").read_text())
    if (report["protocol"], report["arena_bytes"], report["first_out_of_bounds_offset"]) != (
        "RXD3", 2048, 68
    ):
        raise ValueError("unexpected report format or guard boundary")
    print(f'{directory} ({report["board_revision"]}, {report["profile"]})')
    complete = report["completed"]
    for row in report["trials"]:
        trial = row["trial"]
        if "snapshot" not in row:
            print(f'{trial:03} missing snapshot: {row.get("error", row.get("reason"))}')
            complete = False
            continue
        name = f"trial-{trial:03}.bin"
        if row["snapshot"] != name:
            raise ValueError(f"unexpected snapshot name for trial {trial}")
        arena = (directory / name).read_bytes()
        if len(arena) != 2048:
            raise ValueError(f"{name}: expected a 2048-byte snapshot")
        changed = [i for i, b in enumerate(arena) if b != row["sentinel"]]
        high = changed[-1] if changed else None
        guard = next((i for i in changed if i >= 68), None)
        if (high, guard) != (row["highest_changed"], row["first_guard_change"]):
            raise ValueError(f"{name}: snapshot and report disagree on the footprint")

        payload = bytes(row["expected_wire_bytes"])
        if len(payload) != row["bytes"] or len(payload) > 1023 or (row["pair"] and len(payload) != 128):
            raise ValueError(f"{name}: inconsistent stimulus length")
        if row["pair"]:
            payload = payload[64:]  # The two 64-byte packets have different contents.
        first = "-" if guard is None else f"{guard}={arena[guard]:02x}"
        result = f"{trial:03} {row['pipe']} bytes={row['bytes']} high={high} guard={first}"
        if payload:
            matched = next((i for i, b in enumerate(payload) if arena[i] != b), len(payload))
            tail = arena[len(payload):len(payload) + 2]
            result += (f" payload_prefix={matched}/{len(payload)}"
                       f" tail={tail.hex(' ')} expected_crc={usb_crc(payload).hex(' ')}")
        print(result)
    return complete


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directories", type=Path, nargs="+")
    complete = True
    for directory in parser.parse_args().directories:
        try:
            if not inspect(directory):
                complete = False
        except (OSError, KeyError, TypeError, ValueError) as error:
            print(f"{directory}: {error}", file=sys.stderr)
            complete = False
    return 0 if complete else 1


if __name__ == "__main__":
    sys.exit(main())
