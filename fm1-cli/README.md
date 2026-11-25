# fm1-cli

[![crates.io][crate-badge]][crate]
[![Build status][ci-badge]][ci]

Read the M-VAVE FM-1's USB log or the hardware timing diagnostic from a host
terminal. Install the `fm1` command from crates.io with a standard Rust toolchain.

```sh
cargo install --locked fm1-cli
fm1 log
fm1 timing
```

To build from a checkout, run `cargo install --locked --path fm1-cli` from
the workspace root.

`log` prints the USB-MIDI log, or the on-board trial history on a DMA
diagnostic image. With `rticx-timing` loaded, wait for
**DONE - USB REPORT** on the LCD, then use `timing` to read all 25 pages.
With `rticx-cpu1-audio` loaded, `fm1 log` returns the latest block counts
and timing peaks. If the host cannot see the device, the FM-1 LCD shows
the audio result, timings and USB enumeration diagnostics. The [CPU1 probe report](../fm1-rticx/README.md#persistent-cpu1-audio-probe)
lists the fields and status codes. Report text goes to stdout and the
selected USB location goes to stderr.

Connect one device using the development USB ID `1209:0001`. The tool looks
for a MIDI pair that answers the log request, including ports with a name
cached by macOS. To restrict discovery or change the per-request timeout,
use `fm1 log --port 'FM-1 MIDI Piano' --timeout 2.5`.

On Debian or Ubuntu, install `pkg-config` and `libasound2-dev` before building.
macOS uses the system CoreMIDI framework.

## USB peripheral write-range test

Flash the `usb-window` image and close M-Flasher. R4 shows `WND3 R4` on the LCD. The board resets between shots. The runner works with R3 and R4 and can also replay earlier WND1 captures.

```sh
fm1 dma-window --board "black FM-1" --out usb-window-r4-results
```

The full schedule has 24 EP0 SETUP and EP1 bulk OUT trials across two sentinel fills. The host retrieves the frozen SRAM and registers after each reconnect. A failed control skips its dependent shots. `--endpoint ep1` or `--endpoint ep0` selects 12 trials. Use `fm1 dma-window --recover DIR` to retrieve a pending snapshot and `fm1 dma-window --replay DIR` to verify the saved evidence offline. Exit 2 can indicate a crossing, an inconclusive case or an incomplete run. [Probe protocol and board evidence](../docs/usb-dma/window.md)

Export one or more completed capture directories to a single JSON file with `--pack`. The JSON goes to standard output and verification messages go to standard error. Replay accepts the file directly, without a board.

```sh
fm1 dma-window --pack usb-window-r4-results > window-r4.json
fm1 dma-window --replay docs/usb-dma/black-fm1/window-runs.json
```

## EP1 OUT DMA diagnostic

The three diagnostic images use the USB task, USB0 interrupt, 10 ms fallback
and two-octave-button download gesture from `rticx-timing`. Build them with
the normal firmware toolchain or use the packaged images.

The [physical investigation](../docs/usb-dma/README.md) includes the black
FM-1 logs, CRC observations, reproduction commands, and the DMA safety contract.

```sh
for stage in midi bulk iso; do
    ./fm1/scripts/build-example.sh "rticx-dma-$stage"
done
```

Start with `target/firmware/rticx-dma-midi.fwsc` in M-Flasher. The LCD shows
the profile, furthest enumeration stage and SIE retry count even when the
host fails to enumerate. On this image, `fm1 log` starts with `RXD3 MIDI READY`.
Close other programs sending MIDI to the unit during the measurement. Run
the same diagnostic command after each flash.

```sh
cargo run --locked -q -p fm1-cli -- dma-probe --board-revision "black FM-1"
cargo run --locked -q -p fm1-cli -- log > dma-log.txt
```

| Image | Host traffic and controls |
| --- | --- |
| `rticx-dma-midi` | Stock timing descriptors. The host sends a 48-byte SysEx message through the MIDI class driver. The snapshot and raw count must confirm one 64-byte packet. |
| `rticx-dma-bulk` | Vendor interface. Test a quiet EP1, one 64-byte packet, and two distinguishable 64-byte packets with immediate service and with the first acknowledgement deferred for 50 ms. Repeat a quiet control and 32/33/64-byte writes with `RX_MAX = 0x20`. |
| `rticx-dma-iso` | Adds an isochronous OUT alternate to the bulk image. After the bulk trials, submit individual 64, 65, 68, 69, 96, 128, 255, 256, 512 and 1023-byte packets. Repeat at `RX_MAX = 0xff` and `0x40`. The peripheral retains its production bulk RXCSR settings. |

All trials run with both `0xa5` and `0x5a` fills. EP0 arms the trial and
retrieves the results. The firmware gates EP1, fills a separate aligned
2048-byte SRAM allocation, programs the destination and enables EP1. On
RXPKTRDY it records both COUNT bytes and RXCSR before service. A 500 ms
device deadline gates EP1 and redirects DMA to a separate idle allocation.
After at least three more milliseconds and the BSP receive barrier, it copies
the entire live arena into CPU-owned storage. A silent discard can therefore
leave evidence even if the controller raises no interrupt.

The command creates a new directory containing `report.json` and a 2048-byte
`.bin` per captured trial. Use `--out DIRECTORY` to name it. Each row records
the register value written and read back, host completion, device events,
the expected bytes, the first changed guard byte and the highest changed
offset. Isochronous requests use one libusb packet descriptor and record
both transfer and packet status. Failed transfers still get a snapshot
when EP0 remains available. For a nonempty stimulus, a snapshot that still
matches the fill everywhere is inconclusive, even after a ready event.
A quiet control requires an unchanged arena, zero ready packets and no EP1
RX IRQ. A failed quiet or 64-byte control stops dependent tests for that
setting and polarity. Both an observed overrun and an inconclusive run
produce a nonzero exit status.

RXD3 always tests the original 68-byte boundary from `2721022`, including
when built with larger normal buffers. At `RX_MAX = 0xff`, a repeatable change
at offset 68 or higher after a clean quiet control disproves the old bound.
Offsets 64–67 belonged to the spare word. Check the bytes against
the stimulus and repeat on the same port and board. `COUNT` describes the
endpoint event and can differ from the SRAM footprint. A host-completed
isochronous OUT packet can be discarded by a bulk-configured receiver.
A clean guard bounds what changed on this board in these trials.

### Reading the LCD and USB log

The LCD shows the current or last trial number, requested length, RX_MAX
written/read back, fill, ready and IRQ observations, the first two raw
counts and the final count and RXCSR. After capture it shows `HIGH` (last
changed offset) and `GUARD` (first changed offset from 68 and its byte).
The red `HIT` rows retain the first crossing across later trials and USB
disconnects while the board stays powered. `AB#` identifies a trial
interrupted before its snapshot. A dash marks an absent observation.

For a capture without the host CLI, load the MIDI image and wait for
`USB UP`. Tap PLAY and send a note from the Mac to the FM-1, or leave the
port quiet. The device captures the first ready packet or freezes after
three seconds of silence. An ordinary Note On generally occupies one
four-byte USB-MIDI event. The LCD gives the actual observed count.

Run `fm1 log` after each suite, before power-off or reflashing. On all
three images the detailed log uses read-only EP0 requests and can be read
again after a USB reconnect. It retains the last 64 completed trials.
It includes the raw CSR, 16-bit count and time for the first two ready
observations and the deadline, plus the written/read-back maximum, fill,
highest change, first guard change and changed guard byte. Host transfer
completion and full arena snapshots are in the `dma-probe` output directory.
On the MIDI image, a MIDI-only log reader can also read a compact latest
result after a capture.

## Stress the production 2 KiB boundary

Flash `rticx-dma-stress.fwsc` to capture 8192 bytes from EP1, including
changes past the normal 2048-byte data area. The LCD retains the first
crossing if USB drops out. The command saves the detailed USB journal,
packet counts, per-packet host completions and every captured SRAM image.

```sh
fm1 dma-stress --board-revision "black FM-1" --out stress-results --reconnect
fm1 dma-stress --replay stress-results
```

Replay also accepts the original black FM-1 capture bundled with the
source. Normal `rticx-timing` uses the production guards and the `fm1 timing`
command. [Board results and reproduction](../docs/usb-dma/stress.md) ·
[Production guard behavior](../docs/usb-dma/README.md#when-the-firmware-stops)

## License

Licensed under the [MIT License](LICENSE).

[crate-badge]: https://img.shields.io/crates/v/fm1-cli?logo=rust
[crate]: https://crates.io/crates/fm1-cli
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-cli.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-cli.yml
