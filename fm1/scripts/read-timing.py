#!/usr/bin/env python3
# /// script
# dependencies = ["python-rtmidi>=1.5"]
# ///
"""Read all 25 pages of the FM-1 hardware timing diagnostic over USB-MIDI."""
import argparse
import sys
import time

import rtmidi

# CC112 selects a snapshot page; the existing UsbLog SysEx requests that page.
PAGES = 25
REQUEST = [0xF0, 0x7D, 0xF7]


def find_ports(inputs, outputs, labels):
    for label in labels:
        needle = label.casefold()
        rx = [i for i, name in enumerate(inputs) if needle in name.casefold()]
        tx = [i for i, name in enumerate(outputs) if needle in name.casefold()]
        if len(rx) == len(tx) == 1:
            return rx[0], tx[0]
        if rx and tx:
            raise SystemExit(f"Multiple ports match {label!r}. Inputs: {inputs}. Outputs: {outputs}. "
                             "Use --port to select a unique pair, or disconnect duplicate devices.")
    raise SystemExit(f"No FM-1 timing MIDI port. Inputs: {inputs}; outputs: {outputs}")


def read_page(rx, tx, number, timeout):
    # Drain stale messages, then request an explicitly selected page.
    while rx.get_message():
        pass
    tx.send_message([0xB0, 112, number])
    time.sleep(0.03)  # Let USB0 process the CC before the separate SysEx request.
    tx.send_message(REQUEST)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        event = rx.get_message()
        if event:
            message = bytes(event[0])
            if message.startswith(b"\xf0\x7d\x01") and message.endswith(b"\xf7"):
                return message[3:-1].decode("ascii").rstrip("\n")
        time.sleep(0.005)
    raise SystemExit(f"Timed out waiting for report page {number}. Check USB cable and firmware.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", help="match a unique substring in MIDI input/output names")
    parser.add_argument("--timeout", type=float, default=2.0, help="per-page USB timeout in seconds")
    args = parser.parse_args()
    # The examples share a USB VID/PID, so the host may retain an earlier name.
    labels = [args.port] if args.port else ["FM-1 Timing", "FM-1"]
    rx, tx = rtmidi.MidiIn(), rtmidi.MidiOut()
    rx.ignore_types(sysex=False)
    rx_port, tx_port = find_ports(rx.get_ports(), tx.get_ports(), labels)
    with rx.open_port(rx_port), tx.open_port(tx_port):
        for number in range(PAGES):
            line = read_page(rx, tx, number, args.timeout)
            if number == 0 and line != "24MHZ AUDIO 128F PANEL 10KHZ":
                raise SystemExit("Unexpected firmware response: not the FM-1 timing diagnostic.")
            if number == 1 and line.startswith("RUNNING"):
                raise SystemExit("Diagnostic still running; read again when the LCD says DONE.")
            print(line)


if __name__ == "__main__":
    try:
        main()
    except (rtmidi.RtMidiError, UnicodeDecodeError) as exc:
        raise SystemExit(str(exc)) from exc
