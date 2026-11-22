# fm1-cli

Read the M-VAVE FM-1's USB log or the hardware timing diagnostic from a host
terminal. From the workspace root, install the `fm1` command with a standard
Rust toolchain.

```sh
cargo install --locked --path fm1-cli
fm1 log
fm1 timing
```

`log` prints the current snapshot exactly as recorded, including newlines.
The firmware must serve `UsbLog` requests. With `rticx-timing` loaded, wait for
**DONE - USB REPORT** on the LCD, then use `timing` to read all 25 pages.
Report text goes to stdout and the selected USB location goes to stderr.

Connect one device using the development USB ID `1209:0001`. The tool looks
for a MIDI pair that answers the log request, including ports with a name
cached by macOS. To restrict discovery or change the per-request timeout,
use `fm1 log --port 'FM-1 MIDI Piano' --timeout 2.5`.

On Debian or Ubuntu, install `pkg-config` and `libasound2-dev` before building.
macOS uses the system CoreMIDI framework.

## License

Licensed under the [MIT License](LICENSE).
