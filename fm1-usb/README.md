# fm1-usb

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

A host-side Rust library for the M-VAVE FM-1. It enumerates USB devices with
`nusb` and uses the system MIDI driver through `midir`. The firmware can keep
using its existing USB-MIDI descriptors and log protocol.

Read one snapshot from firmware serving `fm1::usb_midi::UsbLog`.

```rust,no_run
use fm1_usb::Device;
use std::{io::{self, Write}, time::Duration};

let device = Device::find()?;
let (_, snapshot) = device.log_ports(None, Duration::from_secs(2))?;
io::stdout().write_all(snapshot.as_bytes())?;
# Ok::<(), io::Error>(())
```

Automatic discovery requires exactly one USB device with the shared development
ID `1209:0001`. It sends the log request to candidate MIDI outputs and listens
for the framed ASCII response. This works across cached port names. Pass
`Some("FM-1 MIDI Piano")` to restrict the request to a named input/output pair.
The returned ports can be opened again for MIDI, further snapshots, or the
`rticx-timing` report. Use one diagnostic reader per device because log
replies carry no request ID and timing page selection is shared.

For a MIDI instrument, open the ports by name and supply an input callback.
This also works with firmware that doesn't implement the log protocol.

```rust,no_run
use fm1_usb::MidiPorts;
use std::time::Duration;

let ports = MidiPorts::by_name("FM-1 MIDI Piano")?;
let mut midi = ports.connect(|_timestamp, bytes| println!("{bytes:02x?}"))?;
midi.send(&[0x90, 60, 100])?;
std::thread::sleep(Duration::from_millis(200));
midi.send(&[0x80, 60, 0])?;
# Ok::<(), std::io::Error>(())
```

On Debian or Ubuntu, install `pkg-config` and `libasound2-dev` to build the
ALSA backend. macOS uses CoreMIDI.

### USB packet sizes on the FM-1

The normal MIDI API passes messages to the OS MIDI driver. The FM-1
advertises 64-byte bulk data packets. A longer SysEx message can span many
USB packets. The OS and host controller perform that packetization and
enumerate endpoint zero before this application connects. The optional
DMA diagnostic transport can deliberately send a 1023-byte packet on a
test alternate setting. The host API cannot set the device's maximum SRAM
write address. See the [FM-1 board qualification](../docs/usb-dma/README.md#qualifying-normal-midi-on-the-fm-1)
for the physical and software assumptions behind a controlled setup.

With the `dma-probe` feature, `dma_probe::window::WindowProbe` connects to
the `usb-window` bench firmware and retrieves the frozen SRAM for each trial
after a warm reboot. The `fm1 dma-window` command captures and replays these
records. [EP0 and EP1 experiment](https://github.com/fm1-rust/fm1-rust/blob/main/docs/usb-dma/window.md)

## License

Licensed under the [MIT License](https://github.com/fm1-rust/fm1-rust/blob/main/fm1-usb/LICENSE).

[crate-badge]: https://img.shields.io/crates/v/fm1-usb?logo=rust
[crate]: https://crates.io/crates/fm1-usb
[docs-badge]: https://docs.rs/fm1-usb/badge.svg
[docs]: https://docs.rs/fm1-usb/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-usb.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-usb.yml
