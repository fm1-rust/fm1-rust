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

These logs and three byte excerpts are the evidence retained from RXD3.
The follow-up RXS1 archive includes all 65 raw snapshots from the same
black FM-1. Its 73-byte packets reached offset 74 at both `RX_MAX=0x08`
and `0x09`, ruling out a hard SRAM limit of eight times the register. Its
1023-byte packets at all three lower settings produced identifiable payload
and CRC bytes even with zero ready events and no RX IRQ. Across the full
run, offsets 1024 through 8191 retained the original fill. The highest
observed offset is a lower bound on the hardware's possible write extent.
[Black FM-1 stress results and offline replay](stress.md#results-from-the-black-fm-1)

### Interpreting an RXD3 verdict

Compare the host completion with the EP1 register samples and the frozen
SRAM bytes. A successful submission can
leave every byte at its fill. Even a sampled ready packet cannot establish
a write footprint in that case. Conversely, ISO trials 49 and 60 changed
offset 511 with no ready event or RX IRQ.

| Frozen trial | RXD3 verdict |
| --- | --- |
| No OUT submitted, fill intact, no ready packet or EP1 RX IRQ | `Observed` for the quiet control. Any changed byte or endpoint event contaminates that control. |
| Nonempty submission, all bytes still equal to the fill | `Inconclusive`, including when the host completed or the board sampled a ready packet. |
| Nonempty submission, a byte changed at offset 68 or higher | `Overrun` of the historical allocation, even when the host reports a short or failed transfer. |

Use both fills and the expected packet bytes to interpret a changed region.
A final snapshot can miss a transient store or a byte rewritten with its
original value. The 64-byte positive control and the two-packet test also
check the expected payload before reporting an intact boundary.
[RXD3 runner](../../fm1-cli/src/dma_probe.rs) · [Archived ISO journal](black-fm1/iso.log)

## Fix and remaining hardware contract

The BSP at `2462b9817` already reserves **2048 bytes each** for normal EP1 OUT and the shared
EP0 receive/transmit destination. The normal MIDI limit remains 64 bytes
and the original receiver register values are preserved. An enlarged EP1
destination contains every write observed here and has room for a 1023-byte
full-speed payload, its two CRC bytes and word padding. Growing EP0 is a
precaution for a second bus-writable destination. This investigation measured
EP1. RXD3 keeps comparing against the **historical** boundary of 68 so future
builds can still reproduce the original result.

Taking the USB port performs no DMA. Applications can call the safe
`UsbPort::try_into_midi(identity)` with any host connected. On AC791N it detaches USB0
and returns `UsbError::DmaProtectionUnavailable` without programming a DMA
destination. `UsbPort::into_midi(identity)` panics on the same error.
Returning a working device requires the BSP to cover malformed and rejected
packets, all receive destinations and outstanding writes across reconnects.

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
reconnect. The current AC791N driver leaves the range monitor disabled.
[WL82 range setup](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/debug.c#L836-L890) ·
[WL82 bus-master IDs](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/debug.h#L6-L11)

## Qualifying normal MIDI on the FM-1

On an explicitly qualified bench, board setup can call
`unsafe { port.assume_midi_dma() }`. This consumes
the raw port without touching USB0. Application code that receives the
result needs no unsafe block to choose its identity and start normal MIDI.

```rust
use fm1::usb_midi::{Identity, MidiDevice, QualifiedMidiPort};

fn start_instrument(port: QualifiedMidiPort, id: Identity) -> MidiDevice {
    port.into_midi(id)
}
```

The qualified port has a private field, cannot be duplicated and gives access
only to the MIDI personality. The diagnostic constructor has its own unsafe
contract for the additional capture arena. If board setup already knows the
identity, it can pass a ready `MidiDevice` directly. The obligation covers
arbitrary delays before starting, enumeration on EP0, ordinary EP1 traffic,
reset, suspend, host replacement and any DMA outstanding at detach. Safe
code can forget an active handle. Its destructor cannot be the only thing
keeping the bus within the qualified operating conditions.
[Worked board initialization](../../fm1-rticx/examples/usb-log.rs) ·
[Rust Reference on soundness](https://doc.rust-lang.org/reference/behavior-considered-undefined.html#undefinedsoundness)

**A controlled host is a useful restriction.** On the ordinary MIDI profile
`fm1-usb` sends MIDI messages through `midir`. For example, the log request
is the three-byte SysEx message `f0 7d f7`. The OS driver and host controller
divide traffic into USB packets. The descriptors advertise EP1 bulk at 64
bytes and EP0 has 64-byte maximum data packets and an eight-byte SETUP.
A large application message may use many such packets. Standard enumeration
precedes the application opening its MIDI port. The board integration can
trust a specific OS, controller and controlled physical cable, together with
the FM-1's write extent and CPU/DMA handoffs for that traffic. Installing
the host crate or exchanging a handshake cannot establish that premise on a
general USB port. Other raw clients or a replacement host can change the traffic.
[Host MIDI send](../../fm1-usb/src/midi.rs) ·
[Log request](../../fm1-usb/src/log.rs) ·
[USB 2.0 §§5.5.3, 5.8.3, 8.5.3](https://bitsavers.trailing-edge.com/components/usb/USB_2.0_2000.pdf)

**The raw diagnostics have a different traffic envelope.** The CLI checks the
vendor alternate setting. Its isochronous helper accepts 1–4 packets of
1–1023 bytes each. RXS1's largest bulk trial is a 4096-byte transfer split
into 64 legal packets. `DmaProbe::bulk` takes a byte slice of arbitrary
length, and an ARM request declares an intended length. The bounds of the
supplied test schedule apply to that runner's submissions. They cannot
constrain independent senders, the controller's CRC stores or its write
pointer between packets. [Host transport](../../fm1-usb/src/dma_probe.rs) ·
[ISO validation](../../fm1-usb/src/dma_probe/iso.rs) ·
[libusb packetization](https://libusb.sourceforge.io/api-1.0/libusb_packetoverflow.html)

### When the firmware stops

The normal EP0 and EP1 receive allocations each have a 16-byte trailing
guard immediately after the 2048-byte data area. The first forbidden word
is reserved storage in the same enclosing object. Four different words are
seeded while detached at the first activation after startup. A later owner
checks the existing guards. USB bus resets, endpoint configuration and
controller retries retain them. In RXS1 these two staging areas
are larger and their guards start at 8192. The live diagnostic's independent
8192-byte arena still records every byte, including crossings of 2048.

The USB task checks both guards on every service pass, before consuming
pending MIDI and before publishing MIDI output. The async IRQ wait also
checks them at 100 ms intervals if the executor runs and its delay advances.
This catches a persistent change even without RXPKTRDY, COUNT or an RX IRQ.
A changed guard or an observed normal EP1 ready count above 64 makes the
driver detach USB0, synchronize and panic. The EP0 handler also rejects
a reported COUNT0 above 64 this way. The normal 65-byte case panics while
the data still fit in staging. Armed diagnostic packets go through a separate
path so they can be measured. The example panic handlers enter download
mode. Only the application controls its panic handler.

A changed guard is evidence of an earlier store. It can trigger an orderly
stop if the affected bytes are confined to the reserved guard and the CPU
can still run the check. An overwrite that skips it, preserves its value or
hits a live object first may escape detection. Even a perfect final snapshot
can miss a transient write. Silent writes wholly inside 2048 bytes, such as
the observed 512-byte footprint, fit the staging area and need not trip the
production guard. The CPU does not scan an active DMA payload looking for
sentinels. Under the activation contract, the trailing guards remain
CPU-owned. Detection of a breach while DMA is active is best effort.
Volatile reads and SSYNC cannot enforce a bus-master write bound.
[Rust volatile access semantics](https://doc.rust-lang.org/core/ptr/fn.read_volatile.html) ·
[Black FM-1 captures and FIFO evidence](stress.md)

### Restoring safe activation on a raw port

The remaining FM-1 question is whether every USB0 write can be confined
before it can damage unrelated state, with exclusive ownership when the CPU
uses the bytes. A verified pre-store hardware window or a documented bound
for this silicon could establish spatial confinement. A common window that
also exposes EP0 to EP1 still needs a handoff design preventing writes to
bytes the CPU is using. Set protection before enabling D+ and retain it
through reset, detach, in-flight writes and forgotten handles.

The first comparator experiment can use one ordinary 64-byte EP1 OUT packet.
Reserve the entire test area and place the proposed hardware boundary inside
the known payload. With the comparator off, verify the expected payload on
both sides. With it on, confirm the transfer reached the allowed prefix and
inspect the first forbidden byte and the rest of the arena after the fault.
Use an independent readout that preserves SRAM if USB stops responding.
Repeat with a boundary inside a bus beat, then exercise EP0 and reset during
RX. A fault that commits the forbidden byte rules this mechanism out.
The separately named `axi_ram_write_range_limit`
accepts addresses in `0x01bfe000..0x01c00000`. The BSP links ordinary data
from `0x01c40000`. That routine cannot directly protect these buffers.
[WL82 AXI limits](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/debug.c#L1030-L1050) ·
[BSP linker map](../../fm1/runtime/fm1.ld)

Enable the successful path in `try_into_midi` once the BSP establishes both
the per-endpoint write bounds and the handoffs. Install protection before
enabling receive DMA. Keep it effective through deferred attachment, every
retry and any outstanding writes after detach. A protection fault can then
stop the device with the first forbidden byte still intact.

## Lessons from the macOS enumeration experiments

An early diagnostic replaced the firmware's EP0 control engine as well as
changing EP1. The application ran on the black FM-1, but several images
never appeared in macOS USB enumeration. One warm enumeration was followed
by a cold-start failure. The first failing control transaction is unknown.

| Board experiment | Result |
| --- | --- |
| Restore the replacement engine's EP0 allocation to 68 bytes, keeping EP1 enlarged | Enumeration still failed. |
| Service the replacement engine from the production USB0 IRQ and 10 ms polling fallback | Enumeration still failed. |
| Enlarge EP1 to 2 KiB in the working timing firmware, then try a 1 ms poll | Both timing images enumerated and returned data. |
| Add vendor bulk and ISO alternates to the production control engine | Both images enumerated and supplied captures. |

Keep production EP0 reset, address and configuration handling when changing
EP1. Record the image digest and test cold power-on, bootloader-to-app and
reconnect separately. The timing image's LCD records the furthest
enumeration stage and retry count when the host cannot fetch a log.

On macOS, filtering by product or CoreMIDI name missed the board. Discover
the candidate by VID/PID (`1209:0001`) and verify its protocol and profile
over EP0. An early runner requested 32 bytes and received ten beginning
with `RXD1`. The returned length and four-byte identifier distinguish that
legacy reply from a truncated `RXD3` reply. Current `RXD3` and `RXS1`
handshakes are both 12 bytes.
[Host discovery](../../fm1-usb/src/lib.rs) · [Handshake validation](../../fm1-usb/src/dma_probe.rs)

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

While EP1 owns the arena, the driver samples RXCSR and COUNT and can ACK a
ready packet without reading its payload. The driver's async wait services
the deferred ACK, 500 ms capture window and drain deadlines even on a quiet
bus. The timing application also polls every 10 ms. To freeze a capture,
the driver gates EP1, redirects DMA to a separate buffer, waits at least
3 ms under the board's drain assumption, executes the receive barrier and
copies the arena into CPU-owned memory. The host reads those frozen bytes
in 32 EP0 pages using the same trial token. The log
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

The follow-up [2 KiB stress investigation](stress.md) includes a separate
8 KiB capture, Mac packet trains, a programmable raw USB host and recovery
tests for the enlarged production staging.
