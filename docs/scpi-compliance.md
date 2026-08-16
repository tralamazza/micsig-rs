# SCPI-99 compliance

How far the instrument departs from the standard it nominally speaks. Measured
against [SCPI-99][scpi99] (IVI Foundation, May 1999, 819 pages) on an
MHO14-200N running firmware 1.143.72, cross-referenced with the vendored
[programming guide](SCPI%20Programming%20Guide-%20Micsig%20Oscilloscope.pdf).

[scpi99]: https://www.ivifoundation.org/downloads/SCPI/scpi-99.pdf

The short version: the scope speaks a SCPI-*shaped* dialect. Hierarchical
colon-separated headers, `?` for queries, IEEE 488.2 block payloads — but
almost none of the machinery the standard makes mandatory. Treat it as a
vendor protocol that happens to look like SCPI, not as a SCPI instrument.

## Mandatory commands

### IEEE 488.2 common commands

SCPI-99 §4.1.1: *"All SCPI instruments shall implement all the common commands
declared mandatory by IEEE 488.2."* Thirteen are listed. One works.

| Command | Instrument | In the guide |
|---|---|---|
| `*IDN?` | answers | documented |
| `*CLS` | no response, then ~1 s unresponsive | absent |
| `*ESE?` | no response, then ~1 s unresponsive | absent |
| `*ESR?` | no response, then ~1 s unresponsive | absent |
| `*OPC` | no response, then ~1 s unresponsive | absent |
| `*OPC?` | no response, then ~1 s unresponsive | absent |
| `*SRE?` | no response, then ~1 s unresponsive | absent |
| `*STB?` | no response, then ~1 s unresponsive | absent |
| `*TST?` | no response, then ~1 s unresponsive | absent |
| `*WAI` | no response, then ~1 s unresponsive | absent |
| `*RST` | no response, ~2 s unresponsive, **resets nothing** | absent |

The failure mode matters more than the count. An unrecognised header such as
`*FOO?` comes back immediately with `Error:SCPI Command error!` and leaves the
connection healthy. The *mandated* headers return nothing and stall the SCPI
service for about a second, so the parser evidently knows them and no handler
answers. Each row above was measured with an `*IDN?` health check before and
after, waiting for recovery in between.

`*RST` is worth calling out because it is the one that could have done damage.
It does not, and that was checked against a deliberately dirtied instrument
rather than an already-default one. Nine settings were pushed off their
defaults first — vertical scale, probe ratio 10x to 1x, CH2 and CH3 switched
on, timebase to 5 ms/div, trigger mode to `NORMal`, averaging to 32, waveform
format to `BYTE`, graticule to `GRID` — and confirmed by readback. `*RST` then
changed **none** of sixteen settings read back afterwards.

The control that makes that negative meaningful: `:MENU:RESet`, issued against
the identical dirty state, changed eight of the same sixteen — scale
`0.05`→`1.0`, position `0.0`→`3.0`, probe `1.0`→`10.0`, timebase
`0.005`→`0.002`, trigger type `PULSe`→`EDGE`, trigger mode `NORMal`→`AUTO`,
averaging `32`→`2`, graticule `GRID`→`FULL`. So the measurement can detect a
reset; `*RST` simply is not one.

`:MENU:RESet` (guide §3.2.2.5, 2026 edition only) is therefore the command to
use for defaults, with the caveat that it is a *partial* reset: channel enables,
`:BUS<n>:TYPE`, `:WAVeform:FORMat` and `:TIMebase:MODE` all survived it.

Practical consequence: generic tooling that opens a session with `*CLS` or
synchronises on `*OPC?` — ordinary PyVISA and lxi-tools practice — gets a
timeout rather than an error. `micsig` sends no common command other than
`*IDN?` (`src/transport.rs`), which is why it does not trip over this.

### SCPI required commands

SCPI-99 §4.2.1 lists the commands *"required in all SCPI instruments"*. None
are implemented, and none appear in the guide.

| Command | Instrument |
|---|---|
| `:SYSTem:ERRor?` / `:SYSTem:ERRor:NEXT?` | `Error:SCPI Command error!` |
| `:SYSTem:VERSion?` | `Error:SCPI Command error!` |
| `:STATus:OPERation?` / `:CONDition?` / `:ENABle?` | `Error:SCPI Command error!` |
| `:STATus:QUEStionable?` / `:CONDition?` / `:ENABle?` | `Error:SCPI Command error!` |
| `:STATus:PRESet` | no response (a setter, so this is inconclusive) |

There is no `:SYSTem` subsystem at all — the string does not occur anywhere in
the guide. The five `STATus` hits in the guide are `:TRIGger:STATus` and
`:TRIGger:LOGic:STATus`, unrelated to the SCPI status model.

## Error reporting

SCPI-99 §21.8 requires an error/event queue, read with
`SYSTem:ERRor[:NEXT]?`, returning `<number>,"<description>"` — negative codes
reserved by the standard, `-100,"Command error"` being the generic one, and
`0` meaning no error.

This instrument has no queue. It reports failure by answering the failing
query with the literal string `Error:SCPI Command error!`, so errors arrive
in-band, in the same channel as data, with no code and no way to distinguish
one failure from another. That is why `src/decode.rs` has to sniff responses
for a leading `Error:` rather than consulting a status register.

## Keyword abbreviation

SCPI-99 §6.2.1: *"A SCPI instrument shall accept only the exact short and the
exact long forms."* Both, that is — rejecting either is non-conforming.

Checked across 25 documented, parameterless queries, deriving each short form
from the guide's own mixed-case spelling (`:ACQuire:DEPTh` → `:ACQ:DEPT`):

| Result | Count |
|---|---|
| both forms accepted | 24 |
| long form only | 1 (`:ACQuire:DEPTh?`; `:ACQ:DEPT?` errors) |

So abbreviation is very nearly right, with one exception. An earlier revision
of [firmware-quirks.md](firmware-quirks.md) claimed the rule was unimplemented
across the board. That was drawn from two samples, and one of them was not a
valid data point: `COUPling` appears nowhere in the guide, so `:CHAN1:COUP?`
returning `DC` is an *undocumented command that works*, not a rejected long
form. Two further apparent failures were parent nodes that are not queryable —
`:DISPlay:PERSist:MODE?` and `:MEASure:COUNter:SOURce?` both answer in either
form.

## Block data

IEEE 488.2's definite-length block is `#<n><length><data>`, where `<length>`
counts **bytes**. `:SYS:SCR?` complies. `:WAVeform:DATA?` puts a *sample* count
there instead, and the payload runs four times longer than declared. See
[firmware-quirks.md](firmware-quirks.md) — it is the single most damaging
divergence here, because a conforming reader silently truncates the trace.

## Documentation requirement

SCPI-99 §4.2.3: *"The documentation for a SCPI instrument shall list the
version number for which the instrument complies. This information shall
appear on instrument specification sheets and related documents, as well as
the programming manual."*

No SCPI version is stated anywhere in the guide, in either the February 2026
or the May 2024 edition.

## Subsystem naming

SCPI-99 defines no oscilloscope instrument class, so the command tree is a
vendor convention rather than a standard one — the same broad shape Agilent
and Rigol scopes use. None of the following standard roots exist on this
instrument: `:SYSTem`, `:STATus`, `:INITiate`, `:ABORt`, `:SENSe`,
`:CALCulate`. Acquisition is started and stopped with `:RUN` and `:STOP`
rather than `:INITiate` and `:ABORt`.

This is not really a defect — §4.2.2 only requires that capabilities SCPI
*does* describe be implemented as specified — but it does mean a
class-conformant driver has nothing to bind to.

## Reproducing

The measurements above need an attached instrument. Nothing here is covered by
`cargo test`, which deliberately requires no hardware.

```
micsig scpi "*IDN?"          # health check
micsig scpi "*OPC?"          # expect a timeout, not an error
micsig scpi "*FOO?"          # expect Error:SCPI Command error!, immediately
micsig scpi ":SYSTem:ERRor?" # expect Error:SCPI Command error!
micsig scpi ":ACQuire:DEPTh?"; micsig scpi ":ACQ:DEPT?"
```

Allow a second after any `*` command other than `*IDN?` before the next
request, or it will time out too.
