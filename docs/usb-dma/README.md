# EP1 OUT DMA on the black FM-1

At `27210227909df5ad0433787895dc1776fe7c2c83`, USB0 programs EP1 OUT with
`RX_MAX=0xff` and the address of a 68-byte buffer. A black FM-1 changed
offsets **68 and 69** when a Mac submitted one 68-byte packet. At offset 68
the board read back `0x5d` with two different sentinel fills. A 69-byte
packet put the host's **payload** byte `0x14` at offset 68. The 1023-byte
test reached offset **1023**. The production allocation at that revision
cannot accommodate these writes. Its 64-byte `COUNT` clamp executes after DMA.
[Original controller at the tested revision](https://github.com/fm1-rust/fm1-rust/blob/27210227909df5ad0433787895dc1776fe7c2c83/fm1/src/usb_midi/controller.rs)

The measurements used a separate, aligned 2048-byte diagnostic arena. EP1's
physical receiver kept its bulk configuration, including a zero high RXCSR.
Selecting the diagnostic isochronous alternate gave the Mac permission to
schedule an oversized DATA packet to that receiver. The normal MIDI
descriptor advertises 64-byte bulk packets. Ordinary CoreMIDI traffic was
tested separately. The board identification supplied by the operator was
`black FM-1`; the exact silicon stepping is unknown.

## Measurements

All offsets below are decimal and zero-based. `highest` is the last byte
whose final value differed from the fill anywhere in the arena. It cannot
reveal a write that left a byte equal to its sentinel. Both `0xa5` and
`0x5a` runs gave the same reported boundaries. Every no-OUT control stayed
intact. All three images enumerated and recorded zero retries or aborted
trials. The host and device agreed on the highest changed offset in all
60 trials of the final run; 32 crossed the original boundary.
[MIDI journal](black-fm1/midi.log) · [Bulk journal](black-fm1/bulk.log) ·
[ISO journal](black-fm1/iso.log) · [ISO host output](black-fm1/iso-host.log)

| Traffic | RX_MAX write/readback | Ready count(s) | Highest changed offset | Evidence |
| --- | --- | --- | --- | --- |
| One legal 64-byte MIDI or bulk packet | ff/ff | 64 | 65 | The two bytes after each distinct payload matched its USB CRC. |
| Two legal 64-byte bulk packets | ff/ff | 64, 64 | 65 | The second payload replaced the first at offset zero. With delayed first service, both runs saw ready events at 4 and 54 ms. With prompt service both were sampled at 4 ms. |
| Single bulk packets of 32, 33, 64 bytes | 20/20 | 32, 33, 64 respectively | 33, 34, 65 | The register read back as 32 and allowed a 64-byte payload. |
| Single ISO packets of 64, 65, 68, 69, 96, 128, 255, 256 bytes | ff/ff and 40/40 | Submitted length | Payload length + 1 | 68 was the first **tested** length to cross. The suite skipped 66 and 67. |
| One 512-byte ISO packet | ff/ff | 512 | 513 | First changed guard byte was the payload byte at offset 68. |
| One 1023-byte ISO packet | ff/ff | 1023 | 1023 | Both runs reached the 1024th byte. The second CRC byte would be at 1024; the log shows no change there. |
| One 512-byte ISO packet | 40/40 | 512 | 511 | The ready event reported the full length. The changed region stopped at the last payload offset. |
| One 1023-byte ISO submission | 40/40 | No ready event | 511 | Trials 49 and 60 recorded no RX IRQ, no RXPKTRDY, zero COUNT/CSR at the deadline, and `0x14` at offset 68. |

There are two independent ways to identify the bytes past the old allocation.

- A 68-byte payload ends at offset 67. Its computed USB data CRC in wire
  byte order is `5d 94`. Trials 20 and 31 (`0xff`) and 42 and 53 (`0x40`)
  recorded `first_guard=68 guard_byte=0x5d highest=69`. The journal records
  the first guard byte. The value `94` at offset 69 is a **prediction** in
  this record. Inspect the two raw bytes with the command below.
- The raw ISO payload is `0x10 | (offset % 32)`. From a 69-byte payload
  onward, the expected value *inside the payload* at offset 68 is `0x14`,
  which the board recorded for both register settings and both fills. The
  out-of-bounds finding is independent of identifying the CRC.

For the legal controls the operator also reported these actual four bytes
from the saved binary snapshots. The first two are the USB data CRC computed
over the 64 wire payload bytes, including the USB-MIDI headers for MIDI.
Offsets 66 and 67 retained their original fills. [USB 2.0, §8.3.5.2](https://www.usb.org/document-library/usb-20-specification)

| Snapshot | Offset 64 | 65 | 66 | 67 |
| --- | --- | --- | --- | --- |
| MIDI trial 002 | 48 | 5f | a5 | a5 |
| MIDI trial 004 | 48 | 5f | 5a | 5a |
| Bulk trial 002 | 8a | cd | a5 | a5 |

These logs and three byte excerpts are the physical evidence supplied for
this record. Re-running produces the full JSON and 2048-byte snapshots.
The largest observed offset gives a **lower bound** on what this hardware
can overwrite. The 512-byte result with `RX_MAX=0x40` is consistent with
64 units of eight bytes for this case. The test does not establish the
register encoding, a maximum for malformed traffic, or results on other
steppings. A received interrupt is unnecessary for memory corruption.

## Fix and remaining hardware contract

This patch reserves **2048 bytes each** for normal EP1 OUT and the shared
EP0 receive/transmit destination. The normal MIDI limit remains 64 bytes
and the original receiver register values are preserved. An enlarged EP1
destination contains every write observed here and has room for a 1023-byte
full-speed payload, its two CRC bytes and word padding. Growing EP0 is a
precaution for a second bus-writable destination. This investigation measured
EP1. RXD3 keeps comparing against the **historical** boundary of 68 so future
builds can still reproduce the original result.

Taking the USB port is safe and performs no DMA. Activating it with
`UsbPort::into_midi` or `into_dma_probe` requires `unsafe`. When migrating
a caller, establish the receive DMA contract for every host it can see,
including reconnects, then explain that condition at the call site. On the
bench, constrain the host, board and input range to the qualified setup.
For deployment on an unrestricted USB bus, an independently enforced
hardware bound or DMA isolation must cover malformed and rejected packets,
all receive destinations and outstanding writes. A post-receive check or
an interrupt handler cannot undo a store that already reached SRAM.

The WL82 SDK has a concrete lead for hardware confinement. Its
`dev_write_range_limit` programs low/high address registers, a permitted
peripheral ID and an out-of-range error enable. Its header identifies
`DBG_FUSB` as `0x81` and `DBG_FUSB1` as `0x82`. The code shows how to
configure and report a violation. Establish which master and SRAM aliases
are covered and whether **the offending store is suppressed before it
commits**. A useful qualification test places a guard *inside* a larger
reserved diagnostic allocation, programs the candidate window to end before
that guard, sends a known crossing and checks the SRAM after the fault.
Include crossings within a final bus beat, EP0 traffic, reset, detach and
reconnect. No range monitor is enabled by this patch. Its blocking semantics
would need independent validation before using it to justify safe activation.
[WL82 range setup](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/debug.c#L836-L890) ·
[WL82 bus-master IDs](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/debug.h#L6-L11)

## Reproduce on a board

1. On the Mac, build the host CLI from this checkout with standard Rust. The
   physical investigation used Rust 1.100.0. The log reader uses EP0 and the
   diagnostic uses `fm1-usb` for device discovery. The MIDI stage uses the
   system MIDI driver; raw transfers use vendored libusb.

   ```sh
   cargo build --locked --release -p fm1-cli
   host="$PWD/target/release/fm1"
   ```

2. In the firmware toolchain environment, build the flashable packages.
   The measured images used pi32v2 Rust `1.100.0-1`, pi32v2 LLVM
   `23.1.6-2` and the packager at `701a76faf103d1889d9bdf67987b872899c27257`.
   [BSP build and flash instructions](../../fm1/README.md#build-and-flash)

   ```sh
   for stage in midi bulk iso; do
       ./fm1/scripts/build-example.sh "rticx-dma-$stage" || exit 1
   done
   ```

3. Flash `target/firmware/rticx-dma-midi.fwsc` with M-Flasher. Use one device
   with development VID/PID `1209:0001`. Keep other MIDI senders on that
   port idle. Run the first pair of commands below. Flash the bulk image and
   run the second pair. Flash the ISO image and run the last pair. Each suite
   requires a fresh output directory. An observed crossing gives a nonzero
   exit; run the log command separately afterwards, before the next flash.
   Holding both octave buttons for two seconds re-enters download mode.

   ```sh
   "$host" dma-probe --board-revision "black FM-1" --out midi-results
   "$host" log > midi-results/usb-log.txt

   "$host" dma-probe --board-revision "black FM-1" --out bulk-results
   "$host" log > bulk-results/usb-log.txt

   "$host" dma-probe --board-revision "black FM-1" --out iso-results
   "$host" log > iso-results/usb-log.txt
   ```

4. Inspect offsets independently on the Mac. The Python 3.8+ script runs
   offline with the standard library. It recomputes the first guard change
   and highest change from each binary, checks them against the JSON, then
   prints the observed tail and the expected wire CRC side by side. Paired
   trials use the second packet. An incomplete capture or a mismatch between
   snapshot and report produces a nonzero exit. The 68-byte trials in the
   recorded ISO run were 020 and 031 at `0xff`.

   ```sh
   python3 tools/usb-dma-probe/inspect.py midi-results bulk-results iso-results
   xxd -g 1 -s 68 -l 4 iso-results/trial-020.bin
   xxd -g 1 -s 68 -l 4 iso-results/trial-031.bin
   ```

The timing application's USB0 interrupt and 10 ms fallback drive the probe.
After a 500 ms observation window it gates EP1 and redirects it to a separate
normal buffer. It waits at least 3 ms, executes the receive barrier and
copies the entire live arena into CPU-owned memory. The offset scan and
32 EP0 page reads therefore work even when no RX event occurred. The log
retains 64 trial summaries for the powered session. The LCD retains the
last trial, first crossing, enumeration stage and controller retries. On
the configured MIDI image, PLAY arms a local capture for the first ready
packet or three seconds of silence. [LCD and USB log fields](../../fm1-cli/README.md#reading-the-lcd-and-usb-log)

The original bench packages had the following SHA-256 digests. The revised
BSP and its memory layout can produce different binaries from these historical
ones.

| Measured image | SHA-256 |
| --- | --- |
| `rticx-dma-midi.fwsc` | `972b817b2c5e80ad7dd44b5610d8e5d2de719a4ef4ee35fd364e8b10f6fdaaac` |
| `rticx-dma-bulk.fwsc` | `bb6a3674bfe1c064e55109137fe651d2072170f48252ae6f1d79ae3ab66498f3` |
| `rticx-dma-iso.fwsc` | `59d045751ae515b90d1856bf137d93553f601f783c2d8c2fffaf5af584d6bfef` |

For a board-free check of enumeration, the LCD, MIDI PLAY capture, log
retention and an injected write with suppressed endpoint events, use the
[strict-mode emulator harness](../../tools/usb-dma-probe/emulator/README.md).
Its DMA behavior is a test input. Only the hardware logs establish the
physical write footprint.
