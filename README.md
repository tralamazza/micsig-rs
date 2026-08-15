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
micsig waveform [options] [--channel <1-4>] [--file <name>]
```

Reads `:WAVeform:DATA?`, decodes the 16-bit samples (binary or ASCII-hex,
auto-detected), and scales them to volts using `:WAVeform:PREamble?`. Emits CSV
(`sample,time_s,voltage_v`).

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

## Testing

`cargo test` covers block-header parsing, preamble parsing, sample decoding,
and screenshot/benchmark round-trips against an in-process mock instrument.
