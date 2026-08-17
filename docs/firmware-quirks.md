# Firmware quirks

Things the instrument does that the programming guide does not mention, or
contradicts. Each entry was reproduced against hardware rather than inferred
from the manual — several of them silently corrupt data if you take the guide
at its word.

Verified against an MHO14-200N, first on firmware 1.97.70 and re-checked
end-to-end on 1.143.72 (that version string is itself unreliable — see the
`*IDN?` entry below). Entries are marked where the two firmwares differ.

Worth knowing before reading the rest: **the guide does not claim to cover this
model.** Its applicability line lists the MHO1, MHO3 and MHO6 series along with
MO3, MDO, ETO, STO, SATO, TO and ATO — no MHO14. That is a fair part of the
explanation for how long this list is. An earlier edition of the guide (May
2024, versus February 2026 for the vendored copy) is narrower still and
predates the `:SYS:SCR?` and `:WAVeform:DATA:<type>?` sections entirely.

- **The block length field is not always a byte count.** `:SYS:SCR?` reports
  bytes, but `:WAVeform:DATA?` reports a *sample* count and puts four ASCII hex
  characters on the wire per sample — the payload is 4x longer than the header
  says. Treating it as bytes silently truncates the trace to a quarter. The
  ratio was exactly 4.0 on every read measured.
- **`:WAVeform:DATA?` is a paged read, not a snapshot.** Each response carries
  at most 62500 samples and the next call resumes where it stopped, ending with
  an empty block (`#900000000`). On 1.143.72 a 110000-sample `NORMal` record
  arrives as 62500 + 47500 + empty, and those three payloads concatenate
  byte-for-byte into what the single-shot `:WAVeform:DATA:HEX?` returns. Read
  once and you get 57% of the trace with nothing to indicate it.
- **`:WAVeform:MODE` rewinds the read cursor.** Nothing else does —
  `:WAVeform:SOURce` and `:WAVeform:FORMat` writes leave it where it was, so a
  read that follows them returns the *next* page rather than the first. On
  1.97.70 this looked like "`:WAVeform:MODE` is mandatory", because without it
  the cursor was usually already exhausted and every read came back empty.
  `capture` sends it last, after the source, format and preamble.
- **`:WAVeform:FORMat WORD` still returns ASCII hex.** The preamble's `format`
  field reads 0 and `:WAVeform:FORMat?` answers `WORD`, so the wire format is
  sniffed rather than trusted.
- **Screenshots have a corrupt JFIF marker.** `:SYS:SCR?` puts garbage where
  JPEG requires the `FF E0` APP0 marker — `58 00` in most captures, `D8 00` in
  at least one, so the bad value is not even stable (1.143.72 gave `58 00` in
  24 of 24). The rest of the file is a valid baseline JPEG. `screenshot`
  rewrites those two bytes, anchoring on the surrounding SOI and `00 10 "JFIF"`
  rather than on the corrupt value; otherwise no viewer opens the file.
  This is by design across the product line rather than a fault in one unit:
  the guide's own `:SYS:SCR?` example prints the corruption as expected output,
  `#9000358370\FF\D8X\00\00\10JFIF\00\01\01\`, where that `X` is `0x58`. The
  same example also calls the result PNG; it is JPEG.
- **Back-to-back `:SYS:SCR?` returns an empty block.** Issued again before the
  previous capture finishes, the scope answers `#900000000` — on 1.143.72 the
  five captures following a successful one were all empty, so this is not
  limited to the immediate next request. `screenshot` retries up to five times
  at 700 ms intervals, which is enough to make back-to-back captures reliable,
  and reports an empty block as an error rather than writing a zero-byte file.
- **A disabled channel returns another channel's data.** `:WAVeform:DATA?` for
  a channel that is switched off does not error or return an empty block — it
  returns stale samples from whichever channel was last acquired. With only
  CH1 displayed, `-c 2` returned CH1's square wave and `-c 3` returned a copy
  of CH2's trace. `waveform` checks `:CHANnel<n>:DISPlay?` first and refuses.
- **`:WAVeform:PREamble?` only refreshes when `:WAVeform:SOURce` is written.**
  Query it out of that order and the scaling describes the previously selected
  channel. `capture` sets the source first, so its volts are correct — measured
  within 1% of the instrument's own pk-pk reading at 0.49, 1.0 and 2.0 V/div.
- **The preamble describes the *previous* `:WAVeform:DATA?` read.** It is not a
  question of settling: after switching CH1 from 1 V/div to 0.2 V/div the
  preamble kept reporting the old `y_increment` for as long as it was asked —
  five seconds and twenty queries — and only caught up once a capture had been
  read. Scale a trace with a preamble fetched beforehand and the volts are
  wrong by the ratio between the two settings: 84.7 mV for a sample that was
  17.3 mV once the preamble was current, a 5x error for a 5x scale change. The
  time axis goes the same way. It is silent, and every value looks plausible,
  which is what makes it worth stating twice. `capture` reads the preamble
  after the data for this reason, and the result now tracks the instrument's
  own pk-pk within 1% across a scale change rather than only when nothing has
  moved.
- **`:WAVeform:FORMat ASCii` returns volts, not samples** — comma-separated
  scientific notation (`1.148325e-02,...`), already scaled. It does not fit the
  sample-plus-preamble model, so only `WORD` and `BYTE` are offered.
- **`*IDN?` reports an unstable firmware version.** On 1.97.70 the field
  alternated between `1.97.70` and `1.97.8`; on 1.143.72 it alternates between
  `1.143.72` and `1.143.9`. It holds one value for dozens of consecutive
  queries and then flips. The 1.97.70 notes tied this to `:BUS<n>:MODE TXT`,
  but on 1.143.72 it flipped without any correlated command. Do not parse the
  firmware field expecting it to be stable.
- **One command rejects its own documented short form.** `:ACQuire:DEPTh?`
  answers, `:ACQ:DEPT?` errors, which SCPI-99 §6.2.1 forbids — an instrument
  must accept both the exact long and the exact short form. This is the
  exception rather than the rule: of 25 commands checked, 24 took both
  spellings. An earlier version of this list claimed abbreviation was broken
  across the board, which was wrong; see
  [scpi-compliance.md](scpi-compliance.md) for the measurement.
- **Sending any mandated `*` command except `*IDN?` stalls the interface.**
  `*CLS`, `*ESE?`, `*ESR?`, `*OPC`, `*OPC?`, `*RST`, `*SRE?`, `*STB?`, `*TST?`
  and `*WAI` return nothing at all and leave the instrument unresponsive for a
  second or two, where an unrecognised header like `*FOO?` errors cleanly and
  immediately. Generic SCPI tooling that opens with `*CLS` or synchronises on
  `*OPC?` will see a timeout rather than an error. `micsig` sends no common
  command other than `*IDN?`. `*RST` is safe in the sense that it does nothing
  at all: against a deliberately dirtied instrument it changed none of sixteen
  settings, where `:MENU:RESet` changed eight of the same sixteen.
- **`:MENU:RESet` is a partial factory reset.** It restores vertical scale and
  position, probe ratio, timebase extent, trigger type and mode, averaging and
  graticule, but leaves channel enables, `:BUS<n>:TYPE`, `:WAVeform:FORMat`
  and `:TIMebase:MODE` exactly as they were. Do not assume a known state after
  issuing it.
- **A measurement must be opened before it can be read, and only ten fit.**
  `:MEASure:PKPK? CH1` answers `Error:SCPI param error!` until the item is
  added with `:MEASure:OPEN PKPK,CH1`, and for ~200 ms after that it answers
  `--` rather than a number. The instrument holds **exactly ten** items open
  at once: opening eleven or twelve leaves the surplus answering `--` however
  long they are given. `measure` opens in batches of ten and closes each batch
  afterwards.
- **The guide misspells the rise-time measurement.** It lists the items as
  "RISE time" and "FALL time", with a space. `RISE` is rejected;
  `RISetime` and its short form `RIS` work. `FALL` works as written, so the
  two are not even consistent with each other. `ACRMS` is documented but only
  ever answered `--`, and `+RATE`/`-RATE` are rejected outright.
- **`:BUS<n>:TYPE?` does not echo what you set.** `UART` reads back as `Uart`
  and `IIC` as `I2C`, so a readback cannot be compared against the keyword
  used to write it. The other five types echo verbatim.
- **A reconfigured bus needs about half a second before it decodes anything.**
  `:BUS<n>:DATA?` returns only the header row until the decoder catches up —
  ~0.49 s measured when switching a bus cold, against ~0.05 s when it was
  already decoding the same signal. `decode` polls rather than waiting a fixed
  interval, because a 500 ms wait sat right on that boundary and failed
  intermittently.
- **`:WAVeform:STARt`/`:STOP` are accepted but ignored.** Both take a value and
  read it back, but `:WAVeform:DATA?` pages the whole record regardless. The
  automatic paging above is the only way to read past the transfer cap.
- **A newly selected segment takes about 50 ms to reach the readout.**
  `:ACQuire:SEGMented:FRA1 <n>` answers its own query immediately, but
  `:WAVeform:DATA?` keeps serving the previously selected frame for a moment
  after it. Selecting five frames of a filled capture in turn and comparing
  digests: with no wait every single read returned the frame selected before
  it, at 25 ms two of five were still stale, and from 50 ms up every frame was
  correct. `segmented` waits 300 ms, six times the measured boundary, because
  the failure is a plausible-looking trace from the wrong segment.
- **`:ACQuire:SEGMented:NO?` reports the last burst's total, then bounces.**
  Read at 40 ms intervals starting the moment `:MENU:SINGLE` was sent, with 7
  segments armed over a capture that had stored 6, it answered `6, 0, 7, 0, 7,
  7, 7`. A capture that asks for fewer segments than the previous one appears
  to be complete before it has begun. `:TRIGger:STATus?` is stale over the same
  window. `segment::arm` ignores everything for 400 ms after arming and only
  ever counts upward afterwards.
- **`:MENU:RUN` does not restart a segmented burst.** On a stopped instrument
  holding a finished capture it returns to the running state and leaves
  `:ACQuire:SEGMented:NO?` exactly where it was, for as long as it was watched.
  `:MENU:SINGLE` is what arms a new one, and it works from either state.
- **`:ACQuire:SEGMented:QTY` validates nothing.** 100, 1000, 10000 and 100000
  were all accepted and echoed back unchanged. The guide says only "refer to
  the data manual" for the limit, so treat the readback as a record of what was
  asked for rather than what the hardware will do.
- **Trigger settings written over SCPI did not gate acquisition.** With
  `:TRIGger:MODE NORMal`, `:TRIGger:EDGE:SOURce CH1` and the level at 50 V
  against a 2 V pk-pk signal — all confirmed by readback — `:MENU:SINGLE` still
  completed immediately, segmented or not. Whatever the mechanism, do not
  expect to hold off a capture by setting an unreachable trigger level.
- **The display lags SCPI writes by roughly 0.15–0.5 s**, and longer after a
  write that re-arms acquisition (a timebase change). A screenshot taken
  immediately after a settings write can show the previous state.
- **Fixed on 1.143.72:** `:BUS<n>:MODE TXT` no longer takes over the screen or
  stops waveform capture. On 1.97.70 it switched the instrument into a
  full-screen decode view ("Please open the channel first!"), after which
  `:WAVeform:DATA?` returned an empty block in every mode. Configuring a bus
  still switches `:TRIGger:TYPE` to the bus trigger (`S1:UART Start Bit`),
  which has to be set back to `EDGE` by hand.
- **`:WAVeform:DATA:HEX?`, `:BIN?` and `:ASCii?` work despite the stated
  restriction.** The guide documents them in section 3.2.15.6 but notes they
  are "supported only on 12-bit oscilloscopes"; all three answer on this unit.
  Each returns the whole `NORMal` record in one response — 110000 samples in
  ~0.05 s — where `:WAVeform:DATA?` needs three reads. `HEX?` is four ASCII hex
  characters per sample, `BIN?` is 4-byte little-endian signed integers, and
  `ASCii?` is volts in scientific notation. All three declare a sample count,
  not a byte count. They return an empty block in `MAXimum` and `RAW` modes, so
  `capture` uses the paged `:WAVeform:DATA?` instead, which works in all three.
- Transfers cap at ~250 KB per `:WAVeform:DATA?` (62500 samples, or ~18000 in
  `ASCii` format). `capture` pages around it.

See also [protocol.md](protocol.md) for how this tool frames requests, and the
one known limitation it has not worked around.
