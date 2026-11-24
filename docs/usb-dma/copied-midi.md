# USB-MIDI with a private CPU copy

The black FM-1 routed different bytes of the same tagged SETUP request to two
SRAM buffers when software changed EP0_ADR during reception. WaveGraph can
avoid direct CPU access to live USB payloads by keeping the endpoint addresses
fixed and using the WL82 memory-copy engine in both directions. Enable the
`usb-midi-copy` feature and call `UsbPort::into_midi_with_dma_copy` at the
board integration boundary. The returned `MidiDevice` supports normal poll,
interrupt-driven waits, send, receive and session handling.

## Physical measurements

The Mac EPH3 run completed ten shots. CPU0 never read either USB payload
directly while attached in the measurement window. Each independent copy
used a fixed 16-byte task and passed its per-shot calibration and trailer
checks. Both endpoint arenas were observed after detach and an assumed
10 ms drain. The original address was `0x01c41f80`, the spare `0x01c42780`.
Each cell below names byte offsets whose values matched the replacement
SETUP or its CRC. Every other observed byte retained its respective seed.
The ready and copy triggers refer to the replacement SETUP.

| Trigger | Token | Old at trigger | Old in post-switch copy | Old at terminal inspection | Spare at terminal inspection |
| --- | --- | --- | --- | --- | --- |
| Switch before host submission | `2d6c` | — | None | None | 0–9 |
| RXPKTRDY | `dfb2` | — | 0–9 | 0–9 | None |
| First changed copy | `e33a` | 0–3 | 0–7 | 0–7 | 8–9 |
| First changed copy | `1601` | 0–3 | 0–3 | 0–3 | 4–9 |
| First changed copy | `5eff` | 0–3 | 0–7 | 0–7 | 8–9 |

In the fixed-address delivery control (`769c`), all ten bytes changed after
the independent baseline copy. All four quiet controls retained both seeds.
Each of the six challenges had a matching EP1 receipt, ended at CSR0 `0x11`
and COUNT0 8, and recorded zero sampled bus transition flags. Host control
timeouts were expected. Each comparison used a token whose data and CRC
bytes all differed from the seeds. [Ten-shot capture](black-fm1/ep0-observer.json)

The split was visible inside the eight-byte SETUP itself for token `1601`.
Its full wire signature was `40 7d c4 5b 01 16 00 00 83 b2`. Its first
four bytes were present only at the old address, and the final six only at
the spare. The other two cases routed only the CRC to the spare. The journal
identifies the host request and the final placement. A bus analyzer would
be needed to count wire attempts. All three terminal old prefixes matched
the post-switch sample. That sample consists of bus reads. Two old prefixes
advanced between the trigger and the post-switch sample, which does not date
those writes relative to the switch. The run supplies no upper bound on
write retirement or on the address generator.

## WaveGraph on the black FM-1

On November 24, 2026 the operator sent a note from the Mac piano roll and
heard it play on the FM-1. The exact pitch was not recorded. With the same
test firmware, the Mac returned this output from `fm1 log`.

```text
Using 1209:0001 on bus 00, address 1
WaveGraph ready
```

WaveGraph handles the MIDI SysEx request `f0 7d f7` and returns its log
through EP1 IN. This exchange exercises EP1 OUT and IN on the physical
board. The audible note also exercises the route into the synth.

After physically disconnecting and reconnecting the FM-1, the operator
reported that Ableton Live recovered its MIDI connection and that a new
`fm1 log` request returned `WaveGraph ready`. The power state during the
unplug was not recorded, so this run cannot distinguish a warm reconnect
from a fresh boot.

The tested kit
was built from WaveGraph `6fc0cc1` with the copied MIDI BSP patch on
`9b9af57`. The hardware obligations for unsafe activation are described
below.

## Board identity from the copied-MIDI probe

On November 24, 2026 the operator ran the ID1 firmware on the black FM-1.
The Mac received the following reply to `fm1 log` after copied-MIDI startup.

```text
Using 1209:0001 on bus 00, address 1
CHIP_ID 00006F01
Loader tag: fm1
```

ID1 read the full 32-bit WL82 `JL_INTEST->CHIP_ID` register at
`0x00010200`. It also read the installed bootloader's `AC79BLID` signature
and eight-byte, NUL-padded board tag at `0x01c02040`. The firmware allowed
a PLAY tap to start copied MIDI on an exact `fm1` loader tag and reported
the silicon ID. This run exercised enumeration, the EP1 OUT SysEx request
and the EP1 IN response on the unit reporting `0x00006f01`. The maximum
USB0 write extent and the timing of the copy engine's last SRAM access
relative to DONE remain unmeasured by ID1.
[WL82 register layout](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/WL82.h#L374-L381) ·
[Bootloader identity layout](https://github.com/jvitkauskas/m-bootloader/blob/eb4f2c1f83f30f99508d065cb98089e086b2440e/docs/SPEC.md#81-board-selection-and-image-identity)

## Ownership in the experimental MIDI path

At startup the BSP locks these SRAM arenas into one access mode for the
boot. Direct and copied modes cannot be mixed, including by dropping and
reacquiring the port. The CPU never loads or stores live endpoint payloads
or guards on the copied path. Both receive arenas have 2048 usable bytes
and a 16-byte guard. EP1 IN has a separate 68-byte area. Reset and endpoint
reconfiguration select the same permanent addresses. USB0 never receives
a task address, a stack address or a pointer into an application buffer.

The second engine moves exactly 68 bytes for a payload and 16 for a guard.
The length and the two addresses are constructed from private static storage outside
of packet parsing. The driver clears stale DONE, orders the descriptor and
staging writes, starts a single task and requires fresh DONE before reading
or reusing private staging. Two distinct 68-byte calibration patterns must
survive an outbound and a return copy through the EP1 IN arena at startup. Guards are seeded once
and checked through copies. A copy timeout, error, stale DONE, failed
calibration or changed guard detaches USB and poisons access before panic.
A later handle cannot recover a staging buffer that DMA might still own.

Read snapshots are untrusted byte arrays. The BSP checks the USB wire CRC,
takes a second sample on mismatch, and stalls an invalid SETUP or drops an
invalid OUT packet. CRC can collide. A torn or stale message can still
affect USB behavior or a note. No result of the CRC check changes the
copy size or grants Rust a reference to device memory.

The unsafe constructor requires independently bounded USB0 accesses on
every input and bus transition, coherent arbitration even during hardware
collisions, and a bounded, exclusively owned copy engine whose completion
retires all accesses to the descriptor and staging. These obligations extend
through drop, forget and restart. Observing an intact guard does not bound
the next write. A violation outside reserved SRAM can corrupt Rust memory
before software can react. The ordinary safe constructor still detaches and
returns `DmaProtectionUnavailable`. The SDK's descriptor and completion
protocol are in the pinned WL82 vendor archive. [DMA interface](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/dma.h) ·
[Implementation in cpu.a](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/liba/cpu.a) ·
[USB0 confinement tests](window.md)
