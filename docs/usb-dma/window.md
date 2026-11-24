# Does USB0 stop the first out-of-range write?

The USB window probe tests the WL82 peripheral write-range monitor on an AC791N FM-1. The window ends *inside* an 8 KiB diagnostic allocation. A legal packet is long enough to cross the window. A frozen SRAM image shows whether a known payload byte reached the other side. The test uses EP1 bulk OUT for MIDI reception and EP0 SETUP for the control pipe used during enumeration. The firmware presents a vendor test interface to the Mac. The Ableton test below is for WaveGraph.

## Slot 7 allowed USB0 writes past its upper limit

R4 completed all 24 shots on the black FM-1. The eight quiet controls preserved the entire fill. The eight delivery controls (slot off or a 128-byte window) reproduced the payload and its two CRC bytes. All eight narrow-window shots wrote those same bytes. In each case the full 8192-byte arena matched its unprotected delivery control byte for byte. This result held at both destinations, `0x01c437c0` (fill A5) and `0x01c43800` (fill 5A).

| Endpoint | First forbidden offset | Byte found there | Last payload or CRC offset | Endpoint count |
| --- | ---: | --- | ---: | ---: |
| EP1 bulk OUT | 32 | `30` | 65 | 64 |
| EP1 bulk OUT | 33 | `31` | 65 | 64 |
| EP0 SETUP | 4 | `34` | 9 | 8 |
| EP0 SETUP | 5 | `33` | 9 | 8 |

Offsets start at the programmed DMA destination. The endpoint count excludes the CRC. Every crossing read back the requested inclusive high address, with only slot 7 enabled and allowed for FUSB0 (`0x81`). Each took vector 1 and recorded a fresh peripheral write-range cause (`DBG_MSG` changed from 0 to 4). The write-limit master was `0x81`, the error-slot mask was `0x8c` and the MMU write ID was `0x3ff`. Those last two values fail the replay's single-slot and ID checks, hence `CROSSED_WITHOUT_ATTRIBUTED_FAULT` on all eight. The known bytes establish the crossing independently of fault attribution. The exception registers were captured on vector entry and the SRAM later, so the records do not order individual stores against vector entry.

This configuration cannot protect the first forbidden byte. The cuts were deliberately inside the diagnostic arena. The maximum possible write from either endpoint and the event that makes an old destination safe for CPU access remain open. The safe BSP constructor therefore keeps USB0 detached on this board.

## Results from all four firmware revisions

| Firmware | Captures | What the black FM-1 showed |
| --- | ---: | --- |
| R1 | 4 | Setup aborted before the arena fill. All four SRAM images contained zeros. |
| R2 | 4 | The arena was filled. Both write-only endpoint address registers read back as zero. The probe aborted before traffic. |
| R3 | 16 | Four quiet controls and four delivery controls passed. Eight guarded shots aborted at the interrupt-enable readback. |
| R4 | 24 | All 16 controls passed. All eight boundary challenges delivered the full payload and CRC across the limit. |

The [combined JSON](black-fm1/window-runs.json) contains the four reports and all 48 original wire records. Each run's `records` object uses the filenames in its report as keys. To reconstruct a record, decode the 128 bytes in `header_hex`, then append an 8192-byte arena initialized to `arena_fill` and overwrite bytes starting at `arena_offset` with the decoded `arena_hex`. The fill is an observed byte from the saved arena. An empty `arena_hex` means every arena byte had that value. Each trial retains its original SHA-256.

From the workspace root, replay all four runs with the native CLI. It reconstructs each record, checks its hash and firmware checksum, and recomputes the header and verdict against the run's controls. These physical results yield exit status 2. Invalid data yields exit status 1.

```sh
fm1 dma-window --replay docs/usb-dma/black-fm1/window-runs.json
```

## Run on a Mac

1. Flash `usb-window-r4.fwsc` with M-Flasher's **Firmware** action. From an earlier probe, hold PLAY for two seconds to enter the flasher. After restarting, check that the LCD reads **FM-1 USB WINDOW / WND3 R4**. Close M-Flasher and any other app using the board. [BSP flashing instructions](../../fm1/README.md#build-and-flash)
2. Connect one FM-1 to the Mac and run the command below. The native `fm1 dma-window` installed for R3 also drives R4. To install it from this checkout, run `cargo install --locked --path fm1-cli` at the workspace root.

```sh
fm1 dma-window --board "black FM-1" --out usb-window-r4-results
```

The CLI prints `Connected to WND3 window firmware.` The board reboots after each shot. The full schedule contains 24 shots. A failed control can shorten the run. An EP0 stimulus timeout is expected because the bench leaves that SETUP's status stage unserviced. Keep the local results directory for recovery. After a completed run, `fm1 dma-window --pack usb-window-r4-results > window-r4.json` exports the report and every record for sharing. The same command accepts several capture directories.

3. If the board stops reconnecting, note the last LCD token before cycling power. Recover the outstanding record after reconnect. Replay verifies the saved evidence offline.

```sh
fm1 dma-window --recover usb-window-r4-results
fm1 dma-window --replay usb-window-r4-results
```

Exit 2 can indicate a crossing, an inconclusive case or an incomplete run. For a 12-shot run, use `--endpoint ep1` or `--endpoint ep0` and a fresh directory. Hold PLAY for two seconds to return to the flasher.

## What reached SRAM on the black FM-1

R3 acquired 16 checksummed records. Four quiet, comparator-off controls preserved their entire 8192-byte fill. Four delivery controls put exactly the bytes below at the programmed destination. Both endpoints were tested at `0x01c437c0` with fill A5 and `0x01c43800` with fill 5A. The leading guard and everything after the CRC retained their fill in all four records. The CRC values below are computed from the known payload and listed in wire byte order.

| Endpoint | Host payload (hex) | Endpoint count | CRC in frozen SRAM | Total changed bytes |
| --- | --- | ---: | --- | ---: |
| EP0 SETUP | `40 79 32 31 34 33 00 00` | 8 | `d4 ea` at offsets 8–9 | 10 |
| EP1 bulk OUT | `10 11 … 4f` | 64 | `eb f3` at offsets 64–65 | 66 |

Every guarded attempt, including the quiet shots, returned setup status `0x12`. CPU0 requested `0x000000f0` at `0x01eef100` and read `0x00000000`. All eight records have `started_go = 0`, an intact arena and no captured exception. The vector-table and TIMER5 checks had passed. R3 aborted at the source-1 enable check before programming slot 7. The runner skipped all eight dependent boundary shots.

All 16 records had `DBG_MSG = 0`. Even in this idle state the slot mask read `0xaf`, the write-limit master read `0x15` and the MMU write ID read `0x3ff`. Those values alone have no fault attribution. The runner retains both ID registers and requires a newly asserted range cause before interpreting them.

R4 still installs and verifies the terminal exception vector, masks and checks TIMER5, requests source 1 at priority 7, and verifies the comparator configuration. It no longer requires the source-1 enable bit to read back. If a fault dispatches, R4 records its cause on entry. Otherwise it samples the same fault latches at the one-second deadline. The CLI requires a fresh, attributable fault and matching SRAM and control evidence for `OBSERVED_STOP` on either path. The SDK describes `ICFG00` as read/write. Felucca requests the same source-1 value. Neither source specifies what that nibble returns on this chip. [WL82 interrupt bank](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/csfr.h#L275-L284) · [Felucca interrupt setup](https://github.com/hugelton/felucca-bsp/blob/2bd9dbe1fcbb8c67ba6721b2435f3fb0597a202a/chip/fm1_irq.h#L93-L109)

## Why both endpoints returned zero on the black FM-1

R2 filled the entire 8192-byte arena on all four quiet trials. A read after writing each endpoint's DMA address returned zero. Each shot ended before GO status, with its fill intact, `cause = 0` and `trapped = 0`. No payload or guarded trial ran. These four captures have valid checksums and distinct nonces. The earlier R1 abort occurred before the fill and remains a separate observation.

| Endpoint | Tokens (A5 / 5A) | Status | Address register | Requested | Read |
| --- | --- | --- | --- | --- | --- |
| EP1 bulk OUT | `0xd779` / `0xaadd` | 0x14 | EP1_RADR, 0x00011820 | 0x01c437c0 | 0 |
| EP0 SETUP | `0x7b33` / `0xb1ab` | 0x13 | EP0_ADR, 0x00011818 | 0x01c437c0 | 0 |

The WL82 SDK defines `___WO` as write-only and applies it to both members of `JL_USB_TypeDef`. It separately declares the debug window's bounds, allowed ID and enable registers read/write. Felucca and x0x program the endpoint destinations with stores. The physical zero is consistent with the SDK. It gives no information about whether the controller latched an address. R2 incorrectly required a readable value. [WL82 access types](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/WL82.h#L18-L23) · [USB registers](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/WL82.h#L464-L485) · [Debug window registers](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/include_lib/driver/cpu/wl82/asm/csfr.h#L137-L186) · [Felucca HAL](https://github.com/hugelton/felucca-bsp/blob/2bd9dbe1fcbb8c67ba6721b2435f3fb0597a202a/chip/fm1_usb.h#L43-L55) · [x0x HAL](https://github.com/charlesvestal/fm1-x0x/blob/6515e77d978c23414c80162558532f0b4312dd4f/firmware/hal/fm1_usb.h#L25-L50)

R3 writes each destination once during GO. The host checks for the exact 64-byte bulk payload or eight-byte SETUP in the frozen arena. The A5 fill places the start at offset 64, the 5A fill at offset 128. A quiet shot can establish a clean background. Delivery requires the expected bytes at the selected offset, an intact leading guard and the expected count. The guarded in-range capture must reproduce the entire corresponding unprotected capture. Boundary shots depend on both quiet and both positive controls. R3 attempts the independent unprotected delivery control even if its quiet control fails.

A WND3 header reports `arena_base` (CPU address of the first frozen byte) and `dest` (address programmed). An R1/R2 WND1 record keeps its original `address_readback` and classification. Replaying the R2 report with the R3 CLI displays each failing register, requested value and returned value.

## What the first black FM-1 run established

The four R1 quiet attempts covered EP0 and EP1 with fills 0xa5 and 0x5a. The Mac recovered all 130 pages with the matching nonce and checksums after each warm reboot. Each firmware header recorded result 3, `started_go = 0` and 238 or 239 TIMER4 ticks (about 10 µs). All four 8192-byte images contain only zeros. R1 still had to fill the arena at that point. Its reported `last = 8127` comes from comparing the zeroed allocation with the requested fill. The runner correctly skipped the dependent transfers. This run establishes device enumeration, GO dispatch and retention across the reset. It gives no measurement of a USB write at the boundary.

The two R1 checks before the fill were a readback of the exception vector and full-word readbacks of all 32 interrupt-configuration registers. They shared status 3. R2 ran the unprotected controls with CPU0 interrupts masked and TIMER4 free-running. R2 and R3 installed and checked vector 1, disabled TIMER5 and required the source-1 enable bit to read back on guarded trials. The standalone program starts with only TIMER5 enabled. Felucca's interrupt driver also manipulates individual source enables. [BSP TIMER5 startup](https://github.com/fm1-rust/fm1-rust/blob/738e8c60aa8c84e654bff4b41382b69a49765d97/fm1/src/time.rs#L122-L141) · [Felucca interrupt setup](https://github.com/hugelton/felucca-bsp/blob/2bd9dbe1fcbb8c67ba6721b2435f3fb0597a202a/chip/fm1_irq.h#L93-L127)

R2 fills the arena before checking the exception setup. For a register mismatch the LCD shows the register address, requested word and observed word in hex. The existing CLI logs those three values in the `header.count`, `header.csr0` and `header.rxcsr` fields of `report.json`, respectively. Its short summary for these new status codes is `INCONCLUSIVE: state`. Endpoint counts and CSRs retain their usual meanings on completed shots. A DMA address readback of 0xffffffff denotes a check that the firmware never reached, unless the failed check was the DMA address itself.

| R2 status (hex) | Failed check |
| --- | --- |
| 10 | Vector 1 address in SRAM |
| 11 / 12 | TIMER5 enable failed to clear / exception enable failed to set |
| 13 / 14 | EP0 / EP1 DMA address |
| 15 / 16 | Slot 7 lower / upper bound |
| 17 | Slot 7 peripheral ID |
| 18 / 19 / 1a | Slot 7 allow / out-of-range error / range-limit enable |

R2 compared the relevant bit in the IRQ and slot-enable checks. The upper-bound setup check permits the low three bits to differ to capture potential rounding. The host still requires the exact upper bound for an attributed stop or crossing.

## What the board measures

Each shot starts from the vendor alternate on CPU0 with CPU1 inactive. A host nonce identifies the shot across its warm reboot. The receiver uses aligned, ordinary SRAM with a 64- or 128-byte leading guard. The measured cut is the **first forbidden byte**, relative to the programmed DMA address. Slot 7 uses the SDK settings for FUSB0 (0x81), inclusive upper bound requested. Comparator readbacks, the two candidate master IDs, faulting slot mask, bus status, endpoint CSRs, counts, first forbidden byte and the entire frozen allocation go in the record. The host compares the frozen bytes with the known packet. Running both sentinel polarities reduces ambiguity when a store leaves a byte equal to the fill.

| Trial per endpoint and fill | EP1 stimulus | EP0 stimulus |
| --- | --- | --- |
| Quiet, slot off | None after GO | None after GO |
| Delivery, slot off | One known 64-byte bulk packet | One known 8-byte SETUP |
| Delivery, allowing 128 bytes | Same packet | Same SETUP |
| Quiet, narrow window | None after GO | None after GO |
| Boundary aligned to 4 bytes | First forbidden offset 32 | First forbidden offset 4 |
| Boundary inside a 4-byte word | First forbidden offset 33 | First forbidden offset 5 |

The EP0 test uses the regular shared EP0 address for setup reception. GO completes its zero-length IN status before the host submits the new SETUP. Its distinctive bytes are `40 79 32 31 34 33 00 00`. The CPU never acknowledges that stimulus. For EP1, the packet is 64 different bytes starting at 0x10. The CPU leaves USB and the monitor running to the same one-second deadline on the ordinary and exception paths. It then gates and detaches USB0, waits 10 ms, and copies the full arena to reserved retained SRAM. A checksum and commit marker distinguish complete records. The new boot displays the first header page and serves the 130 retained pages. These timings and maximum writes are bench assumptions. The bench calls an unsafe API and does not run the WaveGraph audio worker.

- **CROSSED_WITH_FAULT** means an identifiable payload byte reached or passed the programmed boundary and a new slot-7 FUSB range fault was latched. This falsifies pre-commit suppression for that configuration.
- **CROSSED_WITHOUT_ATTRIBUTED_FAULT** means a crossing was visible and attribution failed. Inspect the readbacks, `cause`, `slots`, `master`, `wid` and `trapped`. Wrong bus-master selection, an inactive comparator or other causes remain possible.
- **OBSERVED_STOP** requires matching control captures, an exact upper-address readback, the known prefix up to the cut, an intact remainder and an attributed range fault. It describes the final sampled bytes on this shot. The hardware may reject a whole beat before the cut, and the classifier deliberately leaves that ambiguous result inconclusive. A silent or completely empty shot, a USB timeout, a rounded cutoff, or a fault alone cannot qualify protection.

### What the WaveGraph MIDI port needs

The BSP needs an independent bound on every address USB0 can write, including malformed, oversized or retried traffic and CRC trailers. A shared window would also need to prevent one endpoint from overwriting storage the CPU is using for another. EP1 OUT needs a stable handoff before the CPU reads a packet. EP0 needs one across a new SETUP arriving during an earlier transfer. EP1 IN needs stable TX storage until completion or a confirmed abort. Those guarantees must hold through reset, suspend, detach and reconnect, including outstanding transfers.

The existing safe `UsbPort::try_into_midi(identity)` is where the BSP can return a working device once it establishes those guarantees before attachment. On AC791N it currently detaches and returns `DmaProtectionUnavailable`. WaveGraph already handles that result and keeps playing. Its CPU0 task copies input into bounded events, retires old input sessions on disconnect and removes only the outbound packets the BSP accepted.

## Where the hardware evidence comes from

- **USB0 RX storage.** JieLi's WL82 `usb_epbuf_alloc` adds eight bytes for USB0. The source comment calls for four extra bytes per received CRC and two such allowances for ping-pong operation. The static configuration reserves four extra bytes on EP0, HID RX and speaker RX. This is a vendor sizing rule for normal packets. R3 identifies the two actual CRC16 bytes on both endpoints. The rule gives no upper bound for a mismatched or oversized packet. The allocator's separate alignment comment explicitly names the high-speed controller. [WL82 allocator, lines 48–53](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/apps/common/usb/usb_epbuf_manager.c#L48-L53) · [Static endpoint buffers](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/apps/common/usb/usb_config.c#L81-L90) · [HUSB alignment comment](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/apps/common/usb/usb_epbuf_manager.c#L16-L19)
- **Fault timing and handoff.** The AC79 debugging manual describes range protection as raising an exception on a prohibited software write. Its example uses a CPU writer. The WL82 peripheral debug driver programs the window and reports faults. Neither source specifies whether USB's offending SRAM beat commits, how a partly overlapping beat is handled, or when gate/flush/detach drains all outstanding writes. EP0 shares one RX/TX address, and USB 2.0 §5.5.5 requires an endpoint receiving a new SETUP to abort an unfinished control transfer. Firmware must account for a new SETUP while the previous response is outstanding. [AC79 exception FAQ](https://doc.zh-jieli.com/AC79/zh-cn/master/module_example/system/exception.html) · [WL82 fault report](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/debug.c#L639-L676) · [FUSB register research](https://kagaimiq.github.io/jielie/periph/usb-fs.html) · [USB 2.0 specification](https://www.usb.org/document-library/usb-20-specification)
- **Earlier black FM-1 captures.** Legal 64-byte MIDI traffic wrote two CRC bytes beyond the payload. A 69-byte packet put a host payload byte at offset 68, outside the historical allocation. Of 65 later captures in an 8 KiB arena, 18 showed identifiable partial writes without a ready flag or RX IRQ. Those captures motivate testing the store and its attribution directly. The BSP at the tested revision reserves 2 KiB for each of EP0 and EP1. [Original black-board investigation](https://github.com/fm1-rust/fm1-rust/blob/3cfa739ffb6aa3355cd032b5ffdc9d302e8e84af/docs/usb-dma/README.md) · [Raw archive and replay](https://github.com/fm1-rust/fm1-rust/blob/3cfa739ffb6aa3355cd032b5ffdc9d302e8e84af/docs/usb-dma/stress.md)
- **JieLi silicon and SDK.** The eight published AC791N family datasheets supplied with the WL82 SDK cover the USB block and pins. AC7911B v1.4 lists a FUSB 1.1 OTG controller. These documents give no USB0 out-of-range write bound or DMA buffer ownership event. The SDK programs the allowed master, low/high window and out-of-range error. Its fault handler scans `PRP_WR_LIMIT_ERR_NUM` as a bitmask and prints `PRP_MMU_ERR_WID`. It also exposes `PRP_WR_LIMIT_ID`. The probe retains both IDs and repeats the SDK's debug-latch rearm for every shot. [Datasheet catalogue](https://doc.zh-jieli.com/SDKWiki/master/Jieli-Tech/fw-AC79_AIoT_SDK/9-developer-resources/9.1-docs-datasheets.html) · [SDK setup](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/debug.c#L836-L890) · [Fault handler](https://github.com/tuya/TuyaOpen-JieLi/blob/8521d32608e3723b4016b1bf5a5e93c5de5674bc/chip/wl82/AC79_AIoT_SDK/cpu/wl82/debug.c#L640-L676)
- **Stock firmware and reverse engineering.** The stock trace places MIDI on EP4 IN/OUT with 64-byte descriptors, a separate software maximum and writes of 1023 during endpoint setup. Its EP4 DMA path uses a different register block from WaveGraph's USB0 EP1. The independent full-speed register notes describe FIFO flush as *apparently* resetting the SRAM write pointer. Neither source establishes when an outstanding bus write has drained. In a separate physical FM-1 CPU experiment, a misaligned store changed the aligned-down address before the exception arrived seven instruction markers later. That result concerns the CPU. It explains why the probe inspects SRAM even when the USB monitor faults. [Stock USB trace, §§2, 5](https://github.com/AL-255/FM-1-RE/blob/ec832f281ca165a7d5e0722985f3acb1465046d5/docs/io/06-usb.md) · [FUSB reverse engineering](https://kagaimiq.github.io/jielie/periph/usb-fs.html) · [Physical exception logs](https://github.com/jvitkauskas/pi32v2-isa/tree/058b89bb835d12b194a9f13f51ec153c1d6cc24b/hw/fm1-2026-11-15-excprobe)
- **Other FM-1 firmwares.** Felucca uses a 64+4-byte EP1 buffer, sets RX_MAX to 0xff, clamps the count after reception, and holds the packet pending when the application queue is full. The x0x implementation also follows the EP1 layout. These sources document working musical behavior and common ancestry. They do not supply an independent bound against rejected packets. SIDOU initially associated an EP1 IN stall with a 4 mod 8 address. In the subsequent BSP issue, keeping the SIE register accessors out of line restored 10 of 10 identity replies from 0 of 10 at the same optimization and declared buffer alignment. The chip-level timing explanation remains a hypothesis. This is TX interoperability evidence. R3's failing read was in the CPU interrupt bank. [Felucca driver](https://github.com/hugelton/felucca-bsp/blob/2bd9dbe1fcbb8c67ba6721b2435f3fb0597a202a/chip/usb.c#L2047-L2078) · [x0x driver](https://github.com/charlesvestal/fm1-x0x/blob/6515e77d978c23414c80162558532f0b4312dd4f/firmware/src/usb.c) · [SIDOU alignment report](https://github.com/hugelton/Felucca/issues/293) · [On-device reproduction and fix](https://github.com/hugelton/felucca-bsp/issues/14)
- **macOS field reports.** Felucca measured 13 unanswered SysEx requests in 71 open/request/close cycles with clock running. After preserving queued SysEx and leaving the outstanding IN packet in the FIFO, 0 of 70 went unanswered. The firmware still drops stale notes and clock. The Ventura community thread and the SLOOP Sequoia report also distinguish working MIDI from missing USB audio on some composite firmwares. Record both port directions separately. WaveGraph's instrument test uses the FM-1's physical audio output. [Felucca measurements](https://github.com/hugelton/felucca-bsp/pull/44) · [FM-1 community discussion](https://www.reddit.com/r/MVaveFM1/comments/1xd8p6x/usb_audio_on_macos_for_felucca_based_firmwares/) · [SLOOP field report](https://github.com/isod89/sloop-fm1/issues/147)

## Four-strip acceptance test in Ableton Live

Run this on WaveGraph once the BSP can start MIDI for your configuration. Use the WaveGraph firmware for this test. The WaveGraph MIDI task already sends and receives on one USB-MIDI cable. The MIDI channel follows the mixer strip through instrument reassignment.

1. In Live's **Link, Tempo & MIDI** settings, enable **Track** for the WaveGraph FM-1 output. Make four empty MIDI tracks and select **WaveGraph FM-1** under **MIDI To** on each. Set their channel selectors to 1, 2, 3 and 4. Leave Sync and Remote off for the note test. Stop the FM-1 transport and listen to its audio output.
2. With the factory instrument assignments, run `cargo run --locked -p fm1-cli --example usb-window-clips` at the workspace root. Drag the four MIDI files from `target/usb-window-clips/` to the same start time in Arrangement, one per track. The four bars isolate one strip at a time. The two bass clips also contain CC74 cutoff changes. Use WaveGraph's mixer meters to identify the sounding strip. Swap one instrument on the panel and repeat to verify that the channel follows the strip.

| Live output | Factory strip | Test events (MIDI note number) |
| --- | --- | --- |
| Ch. 1 | Low TB-303 | 48, 48 with different cutoff values |
| Ch. 2 | High TB-303 | 60, 60 with different cutoff values |
| Ch. 3 | TR-808 | Kick 36, closed hat 42 |
| Ch. 4 | TR-909 | Clap 39, ride 51 |

3. For the return path, stop Live's clips, enable **Track** on the WaveGraph input and arm four recording tracks with **MIDI From** set to WaveGraph and channels 1–4 separately. Use **Monitor Auto**, and direct their MIDI outputs away from the FM-1 to avoid echo. Start the board's patterns and record. Check the clips for distinct notes, releases and 303 automation on the expected channels. With Live's output still enabled, disable then re-enable the WaveGraph input **Track** switch while the board is playing. Check that device-to-Live events resume, Live-to-device notes continue working, and no old burst or stuck note appears. For a true last-client close, close other MIDI clients and quit/relaunch Live. Test an actual cable reconnect separately.
4. Hold a panel note and a different USB note on one melodic strip, then disconnect USB. Only the USB-owned note should release. If either stays stuck, report the held note numbers, strip, instrument, macOS and Live version. The factory 303 supports CC74 cutoff and CC71 resonance. CC7 mixer-fader control is a separate feature. [WaveGraph routing and CC chart](https://github.com/fm1-rust/wavegraph/tree/9290b5db95452b433414ba3260e08097b241eccb/wavegraph-fm1#pattern-banks) · [Ableton port settings](https://help.ableton.com/hc/en-us/articles/209774205-Live-s-MIDI-Settings) · [Live routing](https://www.ableton.com/en/live-manual/12/routing-and-i-o/)

## Build and software checks

From the workspace root, with the BSP's pi32v2 toolchain environment set, run `./fm1/scripts/build-example.sh usb-window`. The build produces `target/firmware/usb-window.fwsc`. The R4 image was built with pi32v2 Rust 1.100.0-1, LLVM pi32v2-23.1.6-2 and the packager at `701a76faf103d1889d9bdf67987b872899c27257`. The native tests run with `cargo test --locked -p fm1 -p fm1-cli -p fm1-usb --all-features`.

The integration driver uses [fm1-emu at 588db9e](https://github.com/joakimeriksson/fm1-emu/tree/588db9e988ea818951dcf8fda8fdd11d5fcc77eb). In that checkout, apply this repository's `tools/usb-dma-probe/emulator/usb-host.patch` followed by `tools/usb-dma-probe/window/usb-wo-model.patch`. Copy `tools/usb-dma-probe/window/check_window.rs` to `crates/fm1-soc/examples/` and run the command in its header. Case bits 0–1 select the window, bit 2 selects EP0 and bit 3 selects fill 5A. The driver boots the compiled application, sends the USB requests, and retrieves all 130 pages from a new emulated boot with the retained record.

| Emulated condition | Expected observation |
| --- | --- |
| `zero-read-vector` or `zero-read-no-vector` with `fault-stop` or `fault-cross` | Source 1 reads as zero in both models. The first dispatches vector 1. The second leaves the fault latched until the deadline. Both retain the injected SRAM result. |
| `prefix-only` | A correct prefix and intact suffix without an attributed fault remain inconclusive. |
| `vector-readonly` or `reject-limit` on a guarded R4 trial | Firmware records a setup failure at the verified vector or comparator-enable register before GO. |
| `drop-address` on a delivery control | A USB completion with a stale DMA destination fails the positive control. |
| `r3-exception-zero` on the R3 image | Status `0x12`, requested `0xf0`, observed zero, and an untouched fill reproduce the eight guarded board records. |
| `r2-address-zero` on R2, or `r1-held-priority` on R1 | The board's earlier setup failures can be reproduced. R1's recorded status cannot identify its particular failed check. |

The fault tests inject committed bytes and latched faults independently. They verify that firmware records them and that the CLI classifies them. Only the hardware run measures which writes the FM-1 actually commits.
