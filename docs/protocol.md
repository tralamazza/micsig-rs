# Transport and protocol notes

How `micsig` talks to the instrument, as distinct from
[the ways the instrument misbehaves](firmware-quirks.md).

- TCP transport is SCPI-raw (default port `5025`, the LXI SCPI-raw port). Set
  `--port` to match your scope's LAN/WiFi SCPI service.
- USB transport is USBTMC via `rusb`, implemented in `src/usb.rs` (originally
  based on `rust-usbtmc`). It auto-detects the Micsig scope (`18d1:0007`),
  selects the USBTMC interface, and retries the STALL-then-data reads the
  scope performs.
- Queries are detected by a `?` anywhere in the command *header* — the first
  whitespace-delimited word — not by a trailing `?` as lxi-tools does. The `?`
  terminates the header in SCPI, so parameterised queries like
  `:MEASure:PKPK? CH1`, `:BUS1:LEVel? CH1` and `:TRIGger:LIN:DATA? S1` do not
  end with one. Treating those as commands meant they were sent and their
  replies never read, leaving the response in the buffer for whatever came
  next to collect.
- IEEE 488.2 definite-length blocks (`#<n><length><data>`) are parsed for
  screenshot and waveform payloads, which may contain arbitrary bytes.
  `query_raw` returns the raw wire message including the block header on both
  transports; callers interpret the length field themselves — which they have
  to, because [the length field is not always a byte
  count](firmware-quirks.md).

## Known limitation: `waveform` over TCP is unverified

USBTMC frames each response with its own length, so the sample-count quirk is
harmless there. Raw TCP has no such framing: `read_block` reads exactly
`<length>` bytes, which for `:WAVeform:DATA?` is the sample count, so it will
under-read by 4x and leave the rest in the socket, desyncing the connection.
The paging loop makes this worse rather than better, since each of the several
reads leaves three quarters of a page behind. Everything else (`scpi`,
`screenshot`, `benchmark`, `discover`) is transport-agnostic and fine.

Fixing this properly needs command-aware framing; it is untested because the
unit on hand was only reachable over USB.
