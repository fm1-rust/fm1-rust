"""Capture a raw Feather run on a POSIX serial port; no third-party modules."""

import argparse
import array
import fcntl
import json
import os
import re
import select
import sys
import termios
import time
import tty
from pathlib import Path

from vectors import LENGTHS, payload


def capture(lines, out, board):
    """Validate all serial pages, retain partial transcripts after failure."""
    out.mkdir()
    rows = []
    current = None
    snapshot = bytearray()
    end = None
    reference = payload(4096)
    report = {"board_revision": board, "protocol": "FM1-RXS1-FEATHER 1", "trials": rows}

    def write_report():
        tmp = out / "report.tmp"
        tmp.write_text(json.dumps(report, indent=2), encoding="ascii")
        tmp.replace(out / "report.json")

    def finish_trial():
        nonlocal current
        if current is None:
            return
        if len(snapshot) != 8192:
            raise ValueError(
                f"token {current['token']} has {len(snapshot)} snapshot bytes"
            )
        changes = [i for i, b in enumerate(snapshot) if b != current["fill"]]
        high = changes[-1] if changes else -1
        first = next((i for i in changes if i >= 2048), -1)
        prefix = 0
        while prefix < current["length"] and snapshot[prefix] == reference[prefix]:
            prefix += 1
        if (high, first, prefix) != (
            current["high"],
            current["first_guard"],
            current["prefix"],
        ):
            raise ValueError("PC and Feather disagree about the snapshot")
        if not current["length"] and (high >= 0 or current["ready"] or current["irq"]):
            verdict = "inconclusive"
        elif first >= 0:
            verdict = "guard-changed"
        elif current["recover"] and (
            prefix != 64 or current["ready"] != 1 or current["count"] != 64
        ):
            verdict = "recovery-failed"
        elif current["length"] and (
            prefix < 16
            or (current["max"] == 255 and current["length"] == 69 and prefix != 69)
        ):
            verdict = "no-payload-witness"
        else:
            verdict = "observed"
        if verdict != current["outcome"]:
            raise ValueError("PC and Feather disagree about the trial outcome")
        name = f"trial-{len(rows) + 1:03}.bin"
        (out / name).write_bytes(snapshot)
        current["snapshot"] = name
        rows.append(current)
        write_report()
        print(
            f"{len(rows):03} max={current['max']:#04x} n={current['length']} high={high} "
            f"guard={first} {current['outcome']}",
            flush=True,
        )
        current = None

    try:
        with (out / "serial.log").open("w", buffering=1) as log:
            for line in lines:
                log.write(line + "\n")
                if line.startswith("BEGIN "):
                    if line != "BEGIN FM1-RXS1-FEATHER 1" or report.get("started"):
                        raise ValueError("unexpected generator identity or restart")
                    report["started"] = True
                elif line.startswith("TRIAL "):
                    if not report.get("started"):
                        raise ValueError("missing fixture identity")
                    finish_trial()
                    _, token, *fields, outcome = line.split()
                    current = {"token": int(token), "outcome": outcome}
                    for field in fields:
                        k, v = field.split("=")
                        current[k] = int(v, 16 if k in ("max", "fill") else 10)
                    if not 0 <= current["length"] <= 4096:
                        raise ValueError("invalid requested length")
                    snapshot.clear()
                elif line.startswith("PAGE "):
                    _, token, off, data = line.split()
                    if (
                        current is None
                        or int(token) != current["token"]
                        or int(off) != len(snapshot)
                        or not re.fullmatch("[0-9a-f]{128}", data)
                        or len(snapshot) >= 8192
                    ):
                        raise ValueError("out-of-order, short or stale snapshot page")
                    snapshot.extend(bytes.fromhex(data))
                elif line.startswith("END "):
                    finish_trial()
                    end = line
                    break
        if not end:
            raise ValueError("missing END")
        # Verify the full schedule independently of the board's summary.
        expected = []
        for fill in (0xA5, 0x5A):
            for register in (0xFF, 0x40, 0x08, 0x09):
                cases = [(0, 0, 0), (64, 0, 1)]
                if register == 0xFF:
                    cases.append((69, 0, 0))
                for n in LENGTHS[2:]:
                    cases.append((n, 0, 0))
                    if n in (2049, 4096):
                        cases.append((n, 1, 0))
                cases.append((64, 0, 1))
                expected.extend((fill, register, *c) for c in cases)
        schedule = [
            (r["fill"], r["max"], r["length"], r["bad_crc"], r["recover"]) for r in rows
        ]
        report["fixture_summary"] = end
        report["schedule_complete"] = schedule == expected
        report["all_observed"] = bool(
            schedule == expected
            and end == "END observed"
            and all(r["outcome"] == "observed" for r in rows)
        )
        return report["all_observed"]
    except (OSError, EOFError, ValueError, KeyError) as exc:
        report["error"] = str(exc)
        raise
    finally:
        write_report()


def serial_lines(port):
    fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    original = termios.tcgetattr(fd)
    try:
        tty.setraw(fd)
        attrs = termios.tcgetattr(fd)
        attrs[4:6] = [termios.B115200, termios.B115200]
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
        if hasattr(termios, "TIOCMBIS") and hasattr(termios, "TIOCM_DTR"):
            fcntl.ioctl(fd, termios.TIOCMBIS, array.array("i", [termios.TIOCM_DTR]))
        time.sleep(0.2)
        termios.tcflush(fd, termios.TCIFLUSH)
        os.write(fd, b"r")
        pending = bytearray()
        while True:
            if not select.select([fd], [], [], 25)[0]:
                raise TimeoutError("no fixture output for 25 seconds")
            data = os.read(fd, 4096)
            if not data:
                raise EOFError("fixture disconnected")
            pending.extend(data)
            while b"\n" in pending:
                line, _, rest = pending.partition(b"\n")
                pending = bytearray(rest)
                yield line.decode("ascii").rstrip("\r")
    finally:
        try:
            termios.tcsetattr(fd, termios.TCSANOW, original)
        finally:
            os.close(fd)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument(
        "--port", help="Feather's native CDC device (macOS /dev/cu.usbmodem...)"
    )
    source.add_argument(
        "--replay", type=Path, help="Validate a previously recorded serial.log"
    )
    parser.add_argument("--out", type=Path, required=True, help="New results directory")
    parser.add_argument("--board-revision", required=True)
    args = parser.parse_args()
    if args.replay:
        with args.replay.open() as raw:
            passed = capture(
                (line.strip() for line in raw), args.out, args.board_revision
            )
    else:
        passed = capture(serial_lines(args.port), args.out, args.board_revision)
    sys.exit(0 if passed else 1)
