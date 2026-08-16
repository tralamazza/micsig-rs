# micsig-rs

Command-line tool to interface with a Micsig oscilloscope over SCPI (USB, LAN,
WiFi). Commands are drawn from `doc/SCPI Programming Guide- Micsig Oscilloscope.pdf`.

## Build

```
cargo build --release      # binary at target/release/micsig
cargo install --path .     # or install it onto your PATH
```

## Platform support

| Platform | Status |
|---|---|
| macOS (aarch64) | Verified end-to-end against an MHO14-200N over USBTMC |
| Linux (aarch64, Ubuntu 26.04) | Builds, all tests pass; USB data path untested (no device) |
| Windows | Untested |

There are no `cfg(target_os)` branches in the source; the only platform-specific
dependency is libusb. Requires Rust 1.88 or newer.

### Linux

`libusb1-sys` bundles libusb and builds it if `pkg-config` cannot find one, so
a C compiler is the only hard requirement — verified on a clean Ubuntu 26.04
image with neither `pkg-config` nor libusb installed, where it statically links
a vendored libusb using the netlink backend. Installing the system libraries is
still preferable, since it links `libusb-1.0.so` and picks up the udev backend:

```
sudo apt install pkg-config libusb-1.0-0-dev libudev-dev   # Debian/Ubuntu
```

**USB permissions.** Without a udev rule, libusb cannot open the device and
every command fails. Create `/etc/udev/rules.d/99-micsig.rules`:

```
SUBSYSTEM=="usb", ATTR{idVendor}=="18d1", ATTR{idProduct}=="0007", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

then reload and replug:

```
sudo udevadm control --reload-rules && sudo udevadm trigger
```

`micsig discover` flags a device it can see but cannot open, and the connection
error names the udev rule rather than claiming nothing was found.

**Kernel `usbtmc` driver.** The scope's interface 1 is class `FE`/`03`, which
Linux's in-tree `usbtmc` driver binds, creating `/dev/usbtmc0` and holding the
interface. `micsig` asks libusb to auto-detach it on claim and reattach on
release, so the two can coexist; detaching needs the same permissions as above.

Note that `18d1` is Google's vendor ID (the scope runs Android), so an existing
`android-udev-rules` package may already grant access.

## Usage

```
micsig [connection options] <command> [command options]
```

### Connecting

Connection options are global and may appear before or after the subcommand:

| Option | Meaning |
|---|---|
| `-u`, `--usb` | Force USBTMC |
| `-a`, `--address <HOST>` | Force TCP to an IP or hostname |
| `-p`, `--port <PORT>` | TCP port for `--address` (default `5025`) |
| `-t`, `--timeout <T>` | `3`, `0.5`, `2s`, `500ms`; per-command default if unset |

With neither `--usb` nor `--address`, the USB bus is searched for a Micsig
instrument, so a USB-attached scope needs no flags at all. `--usb` and
`--address` are mutually exclusive.

```
micsig scpi "*IDN?"                 # auto: finds the USB scope
micsig -u scpi "*IDN?"              # force USB
micsig -a 10.0.0.5 scpi "*IDN?"     # force TCP
```

### `scpi` — send a command

```
micsig scpi [-x] [-i] <scpi-command>
```

`-x/--hex` prints the response as a hex dump; `-i/--interactive` starts a repl
(and cannot be combined with a command).

```
micsig scpi "*IDN?"
micsig scpi -x ":WAVeform:PREamble?"
micsig scpi -i
```

### `screenshot` — capture the screen

```
micsig screenshot [--file <name>]
```

Writes the image captured via `:SYS:SCR?` (JFIF/JPEG on the MHO series, despite
the manual calling it PNG). Without `--file`, the image is saved to
`screenshot_<target>_<timestamp>.jpg`, where `<target>` is the address or
`usb`. Use `--file -` for stdout.

### `waveform` — capture channel data

```
micsig waveform [--channel <1-4>] [--mode <normal|maximum|raw>] [--file <name>]
```

Sets `:WAVeform:SOURce`, `:WAVeform:MODE` and `:WAVeform:FORMat`, reads
`:WAVeform:DATA?`, decodes the 16-bit samples (binary or ASCII-hex,
auto-detected), and scales them to volts using `:WAVeform:PREamble?`. Emits CSV
(`sample,time_s,voltage_v`) to stdout, or to `--file`.

`--mode raw` reads the full memory depth and is only valid while the scope is
stopped (`micsig scpi ":MENU:STOP"`).

### `benchmark` — measure request latency

```
micsig benchmark [--count <n>]
```

Sends `n` `*IDN?` requests and reports requests/second.

### `discover` — list instruments

```
micsig discover [--ports <p,...>]
```

With no `--address`, enumerates Micsig instruments on the USB bus with their
product name and serial. With `--address`, probes that host over TCP; `--ports`
overrides the single `--port` with a list.

```
micsig discover                              # USB bus
micsig discover -a 10.0.0.5                  # one TCP port
micsig discover -a 10.0.0.5 --ports 5025,111 # several
```

## Notes

- TCP transport is SCPI-raw (default port `5025`, the LXI SCPI-raw port). Set
  `--port` to match your scope's LAN/WiFi SCPI service.
- USB transport is USBTMC via `rusb`, implemented in `src/usb.rs` (originally
  based on `rust-usbtmc`). It auto-detects the Micsig scope (`18d1:0007`),
  selects the USBTMC interface, and retries the STALL-then-data reads the
  scope performs.
- Queries are detected by a trailing `?`, matching lxi-tools' behaviour.
- IEEE 488.2 definite-length blocks (`#<n><length><data>`) are parsed for
  screenshot and waveform payloads, which may contain arbitrary bytes.
  `query_raw` returns the raw wire message including the block header on both
  transports; callers interpret the length field themselves (see below).

## Firmware quirks

Verified against an MHO14-200N running firmware 1.97.70:

- **The block length field is not always a byte count.** `:SYS:SCR?` reports
  bytes, but `:WAVeform:DATA?` reports a *sample* count and puts four ASCII hex
  characters on the wire per sample — the payload is 4x longer than the header
  says. Treating it as bytes silently truncates the trace to a quarter.
- **`:WAVeform:MODE` is mandatory.** Omit it and `:WAVeform:DATA?` returns an
  empty block (`#900000000`) every time, regardless of source and format.
- **`:WAVeform:FORMat WORD` still returns ASCII hex.** The preamble's `format`
  field reads 0 and `:WAVeform:FORMat?` answers `WORD`, so the wire format is
  sniffed rather than trusted.
- **Screenshots have a corrupt JFIF marker.** `:SYS:SCR?` emits `FF D8 58 00`
  where JPEG requires `FF D8 FF E0`; the rest of the file is a valid baseline
  JPEG. `screenshot` repairs those two bytes, otherwise no viewer opens it.
- Transfers cap at ~250 KB per `:WAVeform:DATA?`, so deep captures need
  `:WAVeform:STARt`/`:STOP` paging (not yet implemented).

### Known limitation: `waveform` over TCP is unverified

USBTMC frames each response with its own length, so the sample-count quirk
above is harmless there. Raw TCP has no such framing: `read_block` reads
exactly `<length>` bytes, which for `:WAVeform:DATA?` is the sample count, so
it will under-read by 4x and leave the rest in the socket, desyncing the
connection. Everything else (`scpi`, `screenshot`, `benchmark`, `discover`)
is transport-agnostic and fine. Fixing this properly needs command-aware
framing; it is untested because the unit on hand was only reachable over USB.

## Testing

`cargo test` covers block-header parsing, preamble parsing, sample decoding,
and screenshot/benchmark round-trips against an in-process mock instrument.
`tests/regression.rs` pins the transport edge cases: EOF mid-response, a block
with no trailing terminator, hostname resolution in `discover`, the waveform
sample-count length field, and the JFIF marker repair.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

`src/usb.rs` is derived from [`rust-usbtmc`](https://github.com/rogerioadris/rust-usbtmc)
(c) Rogério Adriano, also dual-licensed MIT OR Apache-2.0.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
