#!/usr/bin/env python3
# /// script
# dependencies = ["python-rtmidi>=1.5"]
# ///
"""Print one snapshot from the FM-1 USB Log example."""
import argparse
import sys
import time

import rtmidi


def find_ports(inputs, outputs, labels):
    for label in labels:
        rx = [i for i, name in enumerate(inputs) if label in name]
        tx = [i for i, name in enumerate(outputs) if label in name]
        if len(rx) == len(tx) == 1:
            return rx[0], tx[0]
        if rx and tx:
            raise SystemExit(f"Multiple ports named {label}. Select a unique pair with --port.")
    raise SystemExit(f"No matching FM-1 port pair. Inputs: {inputs}. Outputs: {outputs}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", help="match this text in the MIDI input and output port names")
    port = parser.parse_args().port
    labels = [port] if port else ["FM-1 USB Log", "FM-1 MIDI Piano"]
    rx, tx = rtmidi.MidiIn(), rtmidi.MidiOut()
    rx.ignore_types(sysex=False)
    rx_port, tx_port = find_ports(rx.get_ports(), tx.get_ports(), labels)
    with rx.open_port(rx_port), tx.open_port(tx_port):
        tx.send_message([0xF0, 0x7D, 0xF7])
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            event = rx.get_message()
            if event:
                msg = bytes(event[0])
                if msg.startswith(b"\xf0\x7d\x01") and msg.endswith(b"\xf7"):
                    sys.stdout.buffer.write(msg[3:-1])
                    return
            time.sleep(0.01)
    raise SystemExit("FM-1 port opened, but no log reply arrived. Check that the firmware serves USB log requests.")


if __name__ == "__main__":
    try:
        main()
    except rtmidi.RtMidiError as error:
        raise SystemExit(str(error)) from error
