# Test the 2 KiB receive boundary on the FM-1

The black FM-1 already supplied a counterexample to the original 68-byte
allocation. A single 69-byte DATA packet put the host's byte at offset 68.
The largest changed offset in the recorded runs was 1023. The question here
is whether traffic can change offset **2048** or later under the enlarged
allocation and whether the receiver can still deliver ordinary MIDI-sized
packets afterwards. The board stepping was not recorded.
[Original measurements and journals](README.md)

A normal full-speed host controller can schedule a single isochronous packet
of up to 1023 data bytes. A larger bulk write on a Mac turns into multiple
USB packets. The USB DATA packet contains a PID, payload and CRC, with an
end-of-packet signal marking the end on the bus. A programmable transmitter
can keep signaling data beyond the usual limit. The device chooses the DMA
base address. The host controls the incoming bits and their timing. An
overrun only needs a receiver that increments its write address farther than
the allocation. A second possible mechanism is advancement across packets or
service/reset boundaries. The earlier silent 512-byte SRAM change shows why
looking only at COUNT, IRQ or rejected transfers misses relevant behavior.
[USB 2.0 §§5.6.3, 8.3.6, 8.4.4](https://bitsavers.trailing-edge.com/components/usb/USB_2.0_2000.pdf)

## Results from the black FM-1

The Mac completed the 64 scheduled trials and one more after a physical
disconnect/reconnect. Across all 65 frozen 8192-byte captures, every byte
from offset 1024 through 8191 retained its trial's fill. The eight quiet
controls retained the fill everywhere. The original flashable RXS1 image
and both sentinel polarities produced the observations below. Offsets are
decimal and inclusive.

| Stimulus | RX_MAX | Observation on the board |
| --- | --- | --- |
| 69 payload bytes | `ff` | The 69 payload bytes and both USB CRC bytes survived through offset 70. This reproduces the failure of the original 68-byte allocation. |
| One 1023-byte packet | `ff` | One ready event with count 1023. The full payload survived at offsets 0–1022 and its low CRC byte, `6c`, at 1023. |
| Four distinct 1023-byte packets | `ff` | The host reported four full completions. The FM-1 sampled one ready event. With prompt service the fourth payload and its low CRC byte survived. With delayed service the first payload survived and byte 1023 held `42`, the fourth packet's high CRC byte. A final snapshot cannot reconstruct the order of the stores. |
| One or four 1023-byte packets, including delayed service | `40`, `08`, `09` | All 18 trials had zero ready events, no RX IRQ and zero gate count. Bytes 1–511 matched the corresponding offsets of the last submitted packet. Byte 0 matched that packet's high CRC byte, `76` for the single or `42` for the fourth. Bytes 512–8191 retained the fill. |
| 1023 then 64 bytes | `ff`, `08` | The short packet's 64 payload bytes and two CRC bytes occupied offsets 0–65. The surviving long-packet data reached 1023 at `ff` and 511 at `08`. |
| 64, 65, 72, 73 bytes | `08`, `09` | Each packet produced its full count, payload and two CRC bytes. In particular a 73-byte payload reached offset 74 at both settings. These settings do not impose a hard SRAM byte limit of `8 × RX_MAX`. |
| 4096-byte bulk train and recovery | `ff` | Each train produced 64 ready events and the last packet and CRC occupied offsets 0–65. All eight per-setting recovery trials and the post-reconnect 64-byte transfer also passed. |

The original CLI matched the first 16 bytes of each test packet. In the 18
silent cases, byte 0 had changed from payload to CRC, so it reported
`Inconclusive`. For example, in trial 010 the input begins with `5d` and its
wire CRC is `6c 76`. The frozen array begins with `76` followed by the
original bytes 1–511. Comparing those 511 distinct positions also distinguishes
the result from a simple 512-byte circular overwrite of the whole payload.
The updated CLI recognizes this precise pattern as `ObservedSilentPartial`.
For a four-packet train this identifies the surviving packet. A host ISO
completion alone cannot count packets serviced by the receiver.

Replay the original JSON and SRAM snapshots offline from a patched checkout.
The archive also contains the device journal retrieved before disconnect.
Trial 065 is the separate reconnect row in the JSON. Replay leaves the
original verdicts and files intact.

```sh
tar -xzf docs/usb-dma/black-fm1/stress-results.tar.gz
cargo run --locked -q -p fm1-cli -- dma-stress --replay stress-results
```

The final line reports `65 captures verified with intact guards, including 18 silent partial writes.`
[Archived black FM-1 acquisition](black-fm1/stress-results.tar.gz)

The archived captures predate the production guard checks. RXS1 deliberately
records oversized input in a separate 8192-byte EP1 arena and reports the
first changed byte at or beyond 2048 on the LCD and in its USB journal. Normal
MIDI checks its own EP0 and EP1 guards and stops on a detected violation.
The emulator exercises that stop path independently, including a silent write
one byte past the production boundary. The physical captures above establish
the observed extent on this board. The injected faults exercise the new
firmware response. [Production behavior](README.md#when-the-firmware-stops) ·
[Fault-injection commands](../../tools/usb-dma-probe/emulator/README.md)

## Run on the Mac

1. Build the stress image with `./fm1/scripts/build-example.sh rticx-dma-stress`
   in the firmware toolchain environment. Flash **rticx-dma-stress.fwsc** through M-Flasher's
   procedure. The display shows the stress profile and the enumeration stage.
   Connect the FM-1 directly to the test host.
2. On the Mac, build the host CLI and run it against a fresh results
   directory. The optional final step prompts for a cable disconnect and
   reconnect.

   ```sh
   cargo build --locked --release -p fm1-cli
   host="$PWD/target/release/fm1"
   "$host" dma-stress --board-revision "black FM-1" --out stress-results --reconnect
   ```

3. Keep `report.json`, all `trial-*.bin` files and `usb-log.txt`. The host
   checkpoints each 8 KiB snapshot and report row. The journal is fetched
   before the optional cable cycle. To read the current journal separately,
   run `"$host" log`. A board power cycle clears RAM. For a CoreMIDI end-to-end check after
   stress, build and flash `rticx-dma-midi` and run
   `"$host" dma-probe --board-revision "black FM-1" --out midi-after-stress`. On loss of USB, photograph
   the LCD before power-cycling. It retains the first guard hit, the last
   trial, maximum and readback, ready counts, IRQ, gate CSR/count, highest
   changed offset and first guard byte. An interrupted acquisition may only
   have a previous completed record and the aborted token.

The acquisition gates EP1 and retargets it, waits three milliseconds, executes
the DMA synchronization instruction, and copies the entire 8192-byte arena
into separate CPU-owned storage. EP0 downloads 128 numbered pages. Snapshot
is timer-driven even with no receive indication. The normal EP1 destination
and EP0 staging also get 8 KiB in this opt-in build. All four DMA buffers
(including the normal transmit buffer) use eight-byte alignment when passed
to the peripheral. The retained journal has 64 records. The LCD keeps the
first crossing even when the rolling journal wraps.

| Mac stimulus, repeated with both fills | What the observation can establish |
| --- | --- |
| Quiet at each register setting | Calibrates fill, freeze and retrieval without sending EP1 OUT data. |
| 69-byte single packet at `ff` | The historical positive control must reproduce the exact 69-byte prefix through offset 68. |
| 1023 bytes, four distinct 1023-byte packets in one submission with prompt/delayed service, and selected 1023/64 pairs | Exercises a full-speed maximum packet, pointer behavior across frames, and an occupied endpoint. Length and status of every submitted packet are recorded. One final snapshot can retain only some of the writes. |
| 64, 65, 72 and 73 at `08` and `09` | Distinguishes a hard 8-byte-unit write limit from reception beyond the indicated packet size. |
| 4096-byte bulk transfer at `ff` | 64 ordinary 64-byte packets. The check requires all 64 ready events, the first two counts and the final payload. A loss is recorded independently of the guard. |
| A 64-byte bulk packet after reselecting the alternate at each setting | Verifies both the data and count for eight complete MIDI note-on/off pairs on EP1. This vendor profile measures USB transport. An audible synth or the OS MIDI class driver is outside that check. |
| Optional cable cycle | Waits for device disappearance, a fresh RXS1 handshake, and a verified 64-byte bulk packet on the reconnected device. |

The 64 trials use fills `a5` and `5a`. The candidate registers `08`, `09`,
and the previously measured `40` are compared with production's `ff`.
The normal firmware keeps `ff`. `GuardChanged` takes precedence over a host
transfer error. `QuietContaminated`, an incorrect register echo, missing
payload witness, a failed positive control, `BulkLoss` and `RecoveryFailed`
each prevent an all-clear result. If USB fails outright, `complete` remains
false for the interrupted suite. For a suspicious row, inspect bytes on both
sides of the boundary with `xxd -g 1 -s 2032 -l 48 stress-results/trial-NNN.bin`
and repeat with the other fill. Two sentinels expose many otherwise invisible
writes. Bytes equal to the original fill, overwritten intermediate values and
writes to an unrelated address can escape this method.

## Optional raw USB transmitter for the FM-1

Use an **Adafruit Feather RP2040 USB Host** with its onboard USB-A port. Flash
`fm1_raw_host.uf2` **on the Feather** by copying it to RPI-RP2 in BOOTSEL mode.
Leave the RXS1 image on the FM-1. Connect Mac → Feather native USB-C,
Feather USB-A → FM-1. The fixture enables USB-A power on GPIO18 and uses
GPIO16/17 for D+/D−. Wait for the FM-1 to finish enumeration. Pick the
Feather's native CDC device from `ls /dev/cu.usbmodem*`.
[Feather pinout](https://learn.adafruit.com/adafruit-feather-rp2040-with-usb-type-a-host/pinouts)

```sh
python3 feather/capture.py --port /dev/cu.usbmodemXXXX \
  --board-revision "black FM-1" --out raw-results
```

In a source checkout the script is `tools/usb-dma-probe/feather/capture.py`.
It asserts DTR, starts a run, retains a serial transcript, reconstructs every
snapshot, and independently checks the 90-trial schedule, high watermark,
guard and exact payload prefix. Interrupted output remains in the results
directory. Replay with `--replay raw-results/serial.log` in place of `--port`
and a fresh output directory. The fixture also reports results directly over
CDC. All nonstandard traffic is confined to EP1 after a matching RXS1/8192/2048
handshake with device 1209:0001. Use a direct connection to this bench image.

The fixture sends one OUT token and one contiguous DATA0 packet per arm.
The valid-CRC suite covers 64, 69, 1023, 2047, 2048, 2049, 3072 and 4096
bytes. It repeats 2049 and 4096 with inverted CRC. A 2047-byte payload is
already large enough for its CRC to reach offset 2048 if accepted in full.
Generated PIO symbols include synchronization, bit stuffing, CRC and a
single EOP. Each vector is decoded and compared at build time. SOFs are
paused while the deliberately overlong transaction is on the wire. Reading
the results and submitting control requests uses the ordinary TinyUSB host
stack. Normal bulk recovery uses a single raw DATA0 packet after a fresh
alternate selection. Every setting gets quiet and 64-byte controls. Both
fills at `ff` must reproduce the old 69-byte positive before the longer
stimuli are interpreted.

The CPU verifies the generated symbol stream. For an independent measurement
of an actual long transaction, capture D+/D− with an analyzer that preserves
malformed packets and check the number of bit times before EOP. A correctly
captured prefix establishes that EP1 saw the stimulus. It does not by itself
measure where the transmitter ended the physical packet. A failed calibration
or an all-sentinel result without a delivery witness remains inconclusive.

To rebuild the Feather image, set `PICO_SDK_PATH` to Raspberry Pi Pico SDK
2.2.0 (`a1438dff1d38bd9c65dbd693f0e5db4b9ae91779`, including its TinyUSB
submodule) and `PIO_USB_PATH` to Pico-PIO-USB
`b186e36bea0f3bcc2ee94ca1fa81e7f71281443a`. Use the Arm GNU embedded
toolchain and Pico SDK's normal picotool/UF2 output.

```sh
cmake -S tools/usb-dma-probe/feather -B build/feather -G Ninja \
  -DPICO_BOARD=adafruit_feather_rp2040_usb_host
cmake --build build/feather
```

## Prior investigations and the missing guarantee

JieLi's catalogue points to eight AC791N-family datasheets. I inspected the
copies in the WL82 SDK below. These describe the USB interface and electrical
or package features. I found no specification for EP1's maximum SRAM address,
the unit or saturation behavior of RXMAXP, the last CRC/padded store after a
rejected packet, or the quiescence interval after gating an endpoint.
The file list below identifies the exact editions reviewed.
The AC79 online USB guide covers device and host class configuration. The
separate AC791x HUSB signal-quality note describes PHY drive, pre-emphasis
and termination settings for the high-speed interface. Neither supplies an
SRAM address bound for USB0's full-speed receiver.

| Chip | Datasheet revision | Pages | SHA-256 prefix |
| --- | --- | ---: | --- |
| AC7911B | 1.4 | 18 | 7ee65c82131a |
| AC7912A | 1.3 | 25 | a292bf3173de |
| AC7913A0 | 1.3 | 16 | a10498635d70 |
| AC7913A6 | 1.3 | 17 | a60f06087dcf |
| AC7913B8 | 1.0 | 17 | 7ac3c444c994 |
| AC7915A | 1.2 | 21 | a73f15f3d47e |
| AC7915B6A | 1.2 | 18 | 636bccdf44b6 |
| AC7916A | 2.3 | 26 | 9edccd886eee |

[JieLi datasheet catalogue](https://doc.zh-jieli.com/SDKWiki/master/Jieli-Tech/fw-AC79_AIoT_SDK/9-developer-resources/9.1-docs-datasheets.html) ·
[Pinned WL82 SDK PDFs](https://github.com/tuya/TuyaOpen-JieLi/tree/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/doc/datasheet/AC791N规格书/datasheet)

There is prior art specific to the FM-1. AL-255 maps indexed SIE register 19
to RXMAXP and describes the DMA address registers. A SIDOU developer reported
an EP1 **IN** stall at address 4 mod 8, while OUT continued working. The
Felucca maintainer then reported an IN endpoint working at the same alignment
in a different build. Eight-byte alignment removes one layout variable.
These reports do not characterize the EP1 OUT write limit.
[AL-255 register map](https://github.com/AL-255/FM-1-RE/blob/main/docs/io/06-usb.md) ·
[Felucca #293 and maintainer response](https://github.com/hugelton/Felucca/issues/293)

AL-255 identifies the MUSB-style serial interface engine and the JieLi DMA
front-end. Kagaimiq's JieLi full-speed notes identify `EPx_RADR` as the
SRAM destination and report that flushing the RX FIFO appears to reset the
write pointer. Felucca's `ep1_config` sets `RXMAXP=0xff` with a `64 + 4`
EP1 receive array. These sources describe the register interface and working
firmware conventions. I found no independent measurement there of the silent
512-byte footprint, the CRC at offset zero or the largest writable address.
[JieLi full-speed register notes](https://kagaimiq.github.io/jielie/periph/usb-fs.html#epx_radr) ·
[Felucca buffer](https://github.com/hugelton/felucca-bsp/blob/2bd9dbe1fcbb8c67ba6721b2435f3fb0597a202a/chip/usb.c#L227) ·
[Felucca setup](https://github.com/hugelton/felucca-bsp/blob/2bd9dbe1fcbb8c67ba6721b2435f3fb0597a202a/chip/usb.c#L1477)

In the vendor SDK, `usb_epbuf_alloc` budgets four extra bytes per USB0 RX
slot for CRC and eight for two slots. In the WL82 `cpu.a`'s `usb.c.o`, the
OUT path in `usb_g_ep_config` passes 1023 to `usb_write_rxmaxp`. USB0's
indexed writer only transmits the low eight data bits. `usb_set_dma_rsize`
stores the caller's size in a RAM structure. It does not program an end
address there. These are concrete facts about the vendor firmware, with no
specified enforcement for a malformed waveform. To reproduce, use
`llvm-ar x cpu.a usb.c.o` and `llvm-dis usb.c.o -o usb.ll`. Inspect these
four symbols, including the USB0 branch of `musb_write_usb`. The SDK
archive's Git blob ID is `dd8053fcac26e94c88c24f53419fa7dcdd78e27d`.
[SDK allocator](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/apps/common/usb/usb_epbuf_manager.c#L48-L53) ·
[WL82 archive](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/liba/cpu.a)

JieLi's public user-boot project explicitly lists AC791N as WL82. Its USB
configuration declares `ep1_dma_out[0x40 + 4]` and returns that allocation
for nonzero OUT endpoints. This is vendor precedent for the old size.
The source gives no adversarial bound, and the FM-1 tests above used our
diagnostic firmware. They do not establish exploitability of the bootloader.
[JieLi bootloader platform list](https://github.com/Jieli-Tech/fw-Bootloader/blob/ff3b9299d1b94d834b995fd858b3f75e2faa9840/README.md) ·
[Bootloader USB buffers](https://github.com/Jieli-Tech/fw-Bootloader/blob/ff3b9299d1b94d834b995fd858b3f75e2faa9840/user_boot/app/src/usb_config.c#L19-L21)

### Why the 512-byte and 1024-byte footprints are plausible

Mentor Graphics' MUSBMHDRC guide §8.4.2 describes a fixed-size receive FIFO
which switches to double-packet buffering when the configured maximum is at
most half the FIFO size, provided double buffering is enabled. It makes a
second prediction independent of the byte footprint. After the first packet,
single buffering sets both `RxPktRdy` and `FIFOFull` (CSR `03`), while double
buffering sets only `RxPktRdy` (CSR `01`) until a second packet is pending.
On the black FM-1 the **same 64-byte recovery payload** gave `03` at `ff`
and `01` at `40`, `08` and `09` in both fills. The earlier bulk run also
gave `01` at `20`. In the NXP LH79524/LH79525 guide §17.2.3.6 a full-speed
OUTMAXP uses 8-byte units, reserves `0x80` for 1023 bytes and documents the
same half-FIFO rule. The NXP part separately programs its DMA transfer count
(§17.1.5.3). The equivalent memory-side enforcement has yet to be identified
on the FM-1.
[Mentor MUSBMHDRC guide, §§8.4.2.1–2](https://linux-sunxi.org/images/7/73/Musbmhdrc.pdf) ·
[NXP LH79524/LH79525 guide, §§17.2.3.6 and 17.1.5.3](https://www.nxp.com/docs/en/user-guide/LH79524-LH79525_UG_V1_3.pdf)

A plausible FM-1 model is one 1024-byte FIFO that can act as two 512-byte
slots. It fits both the SRAM plateaus and the CSR bit. It leaves the exact
address generator, CRC placement and behavior after an electrically
overlength packet unspecified. A discriminating FM-1 experiment would hold
the 64-byte control and the single 1023-byte stimulus fixed and compare
`RX_MAX=0x40` with `0x41`. With a half-FIFO threshold in 8-byte units,
crossing 512 to 520 should change both the control's FIFOFull bit and the
long packet's footprint. Neither this threshold nor the two-bank model has
been established on the board.

## What would change the release decision?

* If a calibrated trial changes offset 2048 or later, the 2 KiB assumption
  fails for that setting. Save both fills, the exact prefix, lengths, count and
  register echo. Repeat the same stimulus at `08` and `09`. A smaller setting
  earns consideration for production only if it clips the crossing and passes
  ordinary 64-byte traffic, packet trains and reconnects on supported boards.
* For this black FM-1, the 2 KiB staging area contains every observed EP1
  write under the Mac suite, including oversized packets, silent writes and
  packet trains. Keeping the production `ff` setting and the staging area is
  supported by these results. Keep the unsafe activation contract for an
  unrestricted bus. The suite did not send a single payload over 1023 bytes,
  reset the board during an in-flight store, or measure EP0's write extent.
* To justify a safe constructor for arbitrary hosts, obtain an authoritative
  upper address and completion guarantee or enforce a DMA memory window
  independently of the USB packet parser. A concrete next experiment is to
  place a hardware write limit *inside* the sacrificial arena at an address
  already known to be crossed. Record the SRAM with that limit off and on.
  Check whether the first forbidden store is actually suppressed, including a
  straddling final bus beat and reset with a transfer pending. A fault flag by
  itself cannot establish that. The existing investigation identifies the WL82
  debug registers and the FUSB master IDs. If no tested setting contains a
  demonstrated overrun, leave USB RX disabled in builds requiring that guarantee
  until a verified containment mechanism is available.
  [Hardware range-monitor lead](README.md#fix-and-remaining-hardware-contract)

The kit's emulator log records the actual RXS1 firmware detecting an injected
silent 3072-byte write at offset 2048, accepting exactly 2048 bytes with the
guard intact, and receiving a new 64-byte packet after an interrupted acquisition
and re-enumeration. The emulator's writer copies the length supplied by the
test. The archived Mac run measures real hardware up to 1023 payload bytes
per packet. The injected 2048- and 3072-byte writes are emulator results.
[Reproduce the firmware-level tests](../../tools/usb-dma-probe/emulator/README.md)
