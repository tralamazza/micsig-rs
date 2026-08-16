//! Waveform data acquisition via the `:WAVeform:*` command subsystem.

use crate::error::{Error, Result};
use crate::scpi::{self, Preamble};
use crate::transport::Scpi;

/// Formats understood by `:WAVeform:FORMat` that this module can decode.
///
/// The instrument also accepts `ASCii`, which is deliberately not offered
/// here: it returns comma-separated volts in scientific notation
/// (`1.148325e-02,...`) rather than raw samples, so it does not fit the
/// sample-plus-preamble model the rest of this module is built on, and
/// feeding it to [`decode_samples`] would silently yield nonsense. Both
/// variants below were verified on an MHO14-200N to put four ASCII hex
/// characters per sample on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 16-bit samples.
    Word,
    /// 8-bit samples.
    Byte,
}

impl Format {
    fn as_str(self) -> &'static str {
        match self {
            Format::Word => "WORD",
            Format::Byte => "BYTE",
        }
    }
}

/// Modes understood by `:WAVeform:MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Points as displayed on screen.
    Normal,
    /// Maximum valid points in the current run/stop state.
    Maximum,
    /// Full memory depth; only valid while the scope is stopped.
    Raw,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "NORMal",
            Mode::Maximum => "MAXimum",
            Mode::Raw => "RAW",
        }
    }
}

/// A captured waveform: raw samples plus the scaling preamble.
#[derive(Debug, Clone)]
pub struct Waveform {
    pub preamble: Preamble,
    pub samples: Vec<i16>,
}

/// Select the channel to read.
pub fn set_source(inst: &mut impl Scpi, channel: u8) -> Result<()> {
    inst.send(&format!(":WAVeform:SOURce CH{channel}"))
}

/// Set the waveform data format.
pub fn set_format(inst: &mut impl Scpi, format: Format) -> Result<()> {
    inst.send(&format!(":WAVeform:FORMat {}", format.as_str()))
}

/// Set the waveform read mode. This is not optional: with no `:WAVeform:MODE`
/// in the setup sequence an MHO14-200N answers `:WAVeform:DATA?` with an
/// empty block (`#900000000`) every time.
pub fn set_mode(inst: &mut impl Scpi, mode: Mode) -> Result<()> {
    inst.send(&format!(":WAVeform:MODE {}", mode.as_str()))
}

/// Read the preamble for the currently selected source.
///
/// The instrument only refreshes the preamble when `:WAVeform:SOURce` is
/// written, so [`capture`] sets the source before calling this. Query it
/// out of that order and the scaling describes whichever channel was
/// selected previously.
pub fn preamble(inst: &mut impl Scpi) -> Result<Preamble> {
    let resp = inst.query(":WAVeform:PREamble?")?;
    scpi::parse_preamble(&resp)
}

/// Fail if a channel is switched off.
///
/// `:WAVeform:DATA?` on a disabled channel does not return an error or an
/// empty block — it returns stale data belonging to whichever channel was
/// last acquired. Observed on an MHO14-200N: with only CH1 displayed,
/// `-c 2` returned CH1's square wave, and `-c 3` returned a copy of CH2's
/// trace. The result looks entirely plausible, so it has to be caught here.
pub fn ensure_channel_enabled(inst: &mut impl Scpi, channel: u8) -> Result<()> {
    let displayed = inst.query(&format!(":CHANnel{channel}:DISPlay?"))?;
    if displayed.trim() == "0" {
        return Err(Error::Message(format!(
            "channel {channel} is switched off; the instrument would return \
             another channel's data. Enable it with: \
             micsig scpi \":CHANnel{channel}:DISPlay ON\""
        )));
    }
    Ok(())
}

/// Upper bound on the number of `:WAVeform:DATA?` reads [`capture`] will make.
///
/// Each read returns at most 62500 samples, so this allows a little over 12 M
/// — one page past the deepest record an MHO14-200N offers (`:ACQuire:DEPTh?`
/// tops out at 11 M). It exists only so that a firmware that never returns the
/// terminating empty block cannot loop forever.
const MAX_PAGES: usize = 200;

/// Capture the waveform for a channel and decode it into signed samples.
///
/// `:WAVeform:DATA?` caps each response at ~250 KB (62500 samples), so a
/// single read returns only the first page of the record. Successive reads
/// continue where the previous one stopped and the instrument signals the end
/// with an empty block, which is what the loop below drains. Writing
/// `:WAVeform:MODE` is the only thing that rewinds that cursor, so it is sent
/// last during setup.
///
/// The undocumented `:WAVeform:DATA:HEX?` returns the whole record in one
/// response and was tried here as a fast path. It is not worth the extra code:
/// it only answers in `NORMal` mode, and measured over ten runs each it made
/// no difference to a full export (0.127 s median against 0.124 s for the
/// paged path), because the transfer is not the expensive part.
///
/// Verified on an MHO14-200N (firmware 1.143.72): the three pages of a
/// 110000-sample `NORMal` record concatenate byte-for-byte into the payload
/// that single-shot read returns, so the paging here loses nothing.
///
/// Verified over USBTMC. Over raw TCP the block framing under-reads each page
/// by 4x because the length field counts samples, not bytes — see the "Known
/// limitation" section in `docs/protocol.md`.
pub fn capture(inst: &mut impl Scpi, channel: u8, mode: Mode) -> Result<Waveform> {
    ensure_channel_enabled(inst, channel)?;
    set_source(inst, channel)?;
    set_format(inst, Format::Word)?;
    let preamble = preamble(inst)?;
    // Last, so the read cursor starts at the beginning of the record.
    set_mode(inst, mode)?;

    let mut samples = Vec::new();
    let mut drained = false;
    for _ in 0..MAX_PAGES {
        let raw = inst.query_raw(":WAVeform:DATA?")?;
        if looks_like_ascii_volts(&raw) {
            return Err(Error::Message(
                "instrument is returning ASCii-format volts, not raw samples; \
                 `:WAVeform:FORMat WORD` did not take effect"
                    .into(),
            ));
        }
        let page = decode_data_block(&raw);
        if page.is_empty() {
            drained = true;
            break;
        }
        samples.extend_from_slice(&page);
    }

    // Returning a partial record silently is the bug this paging loop exists
    // to fix, so refuse rather than hand back a truncated trace.
    if !drained {
        return Err(Error::Message(format!(
            "instrument was still returning data after {MAX_PAGES} reads \
             ({} samples); refusing to report a truncated capture",
            samples.len()
        )));
    }

    if samples.is_empty() {
        return Err(Error::Message(format!(
            "instrument returned no samples for channel {channel}; \
             the scope may be mid-acquisition, or try a different --mode"
        )));
    }
    Ok(Waveform { preamble, samples })
}

/// True if a `:WAVeform:DATA?` payload is ASCii-format volts rather than the
/// hex/binary samples this module decodes. Scientific notation is
/// unmistakable: hex sample data contains neither `.` nor an exponent sign.
pub fn looks_like_ascii_volts(raw: &[u8]) -> bool {
    let head = &raw[..raw.len().min(64)];
    head.contains(&b'.') && head.iter().any(|b| matches!(b, b'e' | b'E'))
}

/// Decode a `:WAVeform:DATA?` response.
///
/// The block header's length field is a **sample** count here, not a byte
/// count, so the payload must be taken to the end of the message rather than
/// truncated at `declared` bytes — doing the latter drops three quarters of
/// the trace. The declared count is used only to trim any trailing
/// terminators the firmware appends (`\r\n\r\n` on an MHO14-200N).
pub fn decode_data_block(raw: &[u8]) -> Vec<i16> {
    let (declared, payload) = match scpi::block_parts(raw) {
        Some((declared, payload)) => (Some(declared), payload),
        None => (None, raw),
    };

    // Trim the terminators and USB alignment padding that follow the payload;
    // a stray NUL would otherwise defeat the ASCII-hex sniff below.
    let end = payload
        .iter()
        .rposition(|b| !matches!(b, b'\r' | b'\n' | b'\0'))
        .map_or(0, |i| i + 1);
    let mut samples = decode_samples(&payload[..end]);

    if let Some(n) = declared
        && n > 0
        && samples.len() > n
    {
        samples.truncate(n);
    }
    samples
}

/// Decode waveform samples, auto-detecting the wire format.
///
/// The format must be sniffed rather than read from the preamble: an
/// MHO14-200N reports `format` 0 in its preamble and answers
/// `:WAVeform:FORMat?` with `WORD`, yet still puts four ASCII hex characters
/// on the wire per sample. Other units are documented to send 16-bit
/// little-endian words, so both are handled.
pub fn decode_samples(raw: &[u8]) -> Vec<i16> {
    if is_ascii_hex(raw) {
        decode_ascii_hex(raw)
    } else {
        decode_word_samples(raw)
    }
}

fn is_ascii_hex(raw: &[u8]) -> bool {
    // Requiring a whole number of 4-character groups keeps short binary
    // payloads that happen to be all hex digits from being misread as text.
    let significant = raw
        .iter()
        .filter(|b| !b.is_ascii_whitespace() && **b != b',')
        .count();
    !raw.is_empty()
        && significant % 4 == 0
        && raw
            .iter()
            .all(|b| b.is_ascii_hexdigit() || b.is_ascii_whitespace() || *b == b',')
}

/// Decode 4-character big-endian ASCII hex groups into signed samples, e.g.
/// `"FFFF0003"` -> `[-1, 3]`. Groups may be separated by commas or whitespace,
/// or run together in a continuous hex string. A trailing partial group is
/// discarded rather than decoded as a short sample.
pub fn decode_ascii_hex(raw: &[u8]) -> Vec<i16> {
    let text: String = raw
        .iter()
        .map(|&b| {
            if b.is_ascii_whitespace() || b == b',' {
                ' '
            } else {
                b as char
            }
        })
        .collect();
    text.split_whitespace()
        .flat_map(|tok| {
            let tok = tok.strip_prefix("0x").unwrap_or(tok);
            // Split continuous hex into 4-character sample groups.
            tok.as_bytes()
                .chunks(4)
                .filter(|c| c.len() == 4)
                .map(|c| std::str::from_utf8(c).unwrap_or(""))
                .collect::<Vec<_>>()
        })
        .filter_map(|s| u16::from_str_radix(s, 16).ok().map(|v| v as i16))
        .collect()
}

/// Decode a little-endian 16-bit signed sample stream. Trailing bytes are
/// ignored if the stream has an odd length.
pub fn decode_word_samples(raw: &[u8]) -> Vec<i16> {
    raw.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// Convert raw samples to volts using the preamble scaling:
/// `v = (sample - y_reference) * y_increment + y_origin`.
pub fn samples_to_volts(wave: &Waveform) -> Vec<f64> {
    wave.samples
        .iter()
        .map(|&s| {
            (s as f64 - wave.preamble.y_reference as f64) * wave.preamble.y_increment
                + wave.preamble.y_origin
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_little_endian_words() {
        let raw = [0x00, 0x00, 0x01, 0x00, 0xFF, 0xFF, 0x00, 0x80];
        assert_eq!(decode_word_samples(&raw), vec![0i16, 1, -1, i16::MIN]);
    }

    #[test]
    fn ignores_trailing_byte() {
        let raw = [0x02, 0x00, 0xAA];
        assert_eq!(decode_word_samples(&raw), vec![2i16]);
    }

    #[test]
    fn scales_samples_to_volts() {
        let wave = Waveform {
            preamble: Preamble {
                format: 1,
                r#type: 2,
                count: 3,
                x_increment: 0.0,
                x_origin: 0.0,
                x_reference: 0,
                y_increment: 0.0625,
                y_origin: 3.96875,
                y_reference: 127,
            },
            samples: vec![127, 128, 0],
        };
        let volts = samples_to_volts(&wave);
        assert!((volts[0] - 3.96875).abs() < 1e-9);
        assert!((volts[1] - 4.03125).abs() < 1e-9);
        assert!((volts[2] + 3.96875).abs() < 1e-9);
    }
}

#[cfg(test)]
mod hex_tests {
    use super::*;

    #[test]
    fn decodes_ascii_hex_samples() {
        let raw = b"0002000000030002FFFF00000002";
        let samples = decode_samples(raw);
        assert_eq!(samples, vec![0x0002i16, 0, 0x0003, 0x0002, -1, 0, 0x0002]);
    }

    #[test]
    fn decodes_ascii_hex_with_commas_and_spaces() {
        let raw = b"0002, FFFF 0000";
        let samples = decode_samples(raw);
        assert_eq!(samples, vec![2i16, -1, 0]);
    }

    #[test]
    fn binary_still_decodes_as_words() {
        let raw = [0x02, 0x00, 0xFF, 0xFF];
        assert_eq!(decode_samples(&raw), vec![2i16, -1]);
    }
}
