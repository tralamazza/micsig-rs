# micsig-rs

Command-line tool to interface with a Micsig oscilloscope over SCPI (USB, LAN,
WiFi). Commands are drawn from `doc/SCPI Programming Guide- Micsig Oscilloscope.pdf`.

## Build

```
cargo build --release
```

## Usage

```
micsig <command> [options]
```

All commands work over TCP (LAN/WiFi) and, with `-u/--usb`, over USBTMC.

### `scpi` — send a command

```
micsig scpi [options] <scpi-command>
```

Options: `-a/--address` (default `127.0.0.1`), `-p/--port` (default `5025`),
`-t/--timeout` (default `3`), `-x/--hex`, `-i/--interactive`, `-u/--usb`.

```
micsig scpi "*IDN?"
micsig scpi -u "*IDN?"          # over USB
micsig scpi -x ":WAVeform:PREamble?"
micsig scpi -i                  # repl
```

### `screenshot` — capture the screen

```
micsig screenshot [options] [--file <name>]
```

Writes the image captured via `:SYS:SCR?` (JFIF/JPEG on the MHO series, despite
the manual calling it PNG). Without `--file`, the image is saved to
`screenshot_<addr>_<timestamp>.jpg`. Use `--file -` for stdout.

### `waveform` — capture channel data

```
micsig waveform [options] [--channel <1-4>] [--mode <normal|maximum|raw>] [--file <name>]
```

Sets `:WAVeform:SOURce`, `:WAVeform:MODE` and `:WAVeform:FORMat`, reads
`:WAVeform:DATA?`, decodes the 16-bit samples (binary or ASCII-hex,
auto-detected), and scales them to volts using `:WAVeform:PREamble?`. Emits CSV
(`sample,time_s,voltage_v`).

`--mode raw` reads the full memory depth and is only valid while the scope is
stopped (`micsig scpi ":MENU:STOP"`).

### `benchmark` — measure request latency

```
micsig benchmark [options] [--count <n>]
```

Sends `n` `*IDN?` requests and reports requests/second.

### `discover` — probe a host

```
micsig discover [options] [--ports <p>...]
```

Probes ports and reports any responding instrument.

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
