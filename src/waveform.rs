//! Waveform data acquisition via the `:WAVeform:*` command subsystem.

use crate::error::Result;
use crate::scpi::{self, Preamble};
use crate::transport::Scpi;

/// Formats understood by `:WAVeform:FORMat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 16-bit signed words (two bytes per sample).
    Word,
    /// ASCII scientific notation, comma-separated.
    Ascii,
}

impl Format {
    fn as_str(self) -> &'static str {
        match self {
            Format::Word => "WORD",
            Format::Ascii => "ASCii",
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

/// Read the preamble for the currently selected source.
pub fn preamble(inst: &mut impl Scpi) -> Result<Preamble> {
    let resp = inst.query(":WAVeform:PREamble?")?;
    scpi::parse_preamble(&resp)
}

/// Capture the waveform for a channel and decode it into signed samples.
pub fn capture(inst: &mut impl Scpi, channel: u8) -> Result<Waveform> {
    set_source(inst, channel)?;
    set_format(inst, Format::Word)?;
    let preamble = preamble(inst)?;
    let raw = inst.query_raw(":WAVeform:DATA?")?;
    let raw = scpi::unwrap_block(&raw);
    let samples = decode_samples(&raw);
    Ok(Waveform { preamble, samples })
}

/// Decode waveform samples, auto-detecting the wire format. Some Micsig
/// scopes return 16-bit little-endian words, others return 4-character ASCII
/// hex per sample (`0002FFFF...`).
pub fn decode_samples(raw: &[u8]) -> Vec<i16> {
    if is_ascii_hex(raw) {
        decode_ascii_hex(raw)
    } else {
        decode_word_samples(raw)
    }
}

fn is_ascii_hex(raw: &[u8]) -> bool {
    !raw.is_empty()
        && raw
            .iter()
            .all(|b| b.is_ascii_hexdigit() || b.is_ascii_whitespace() || *b == b',')
}

/// Decode 4-character little-endian ASCII hex groups into signed samples.
/// Groups may be separated by commas or whitespace, or run together in a
/// continuous hex string.
pub fn decode_ascii_hex(raw: &[u8]) -> Vec<i16> {
    let text: String = raw
        .iter()
        .map(|&b| if b.is_ascii_whitespace() || b == b',' { ' ' } else { b as char })
        .collect();
    text.split_whitespace()
        .flat_map(|tok| {
            let tok = tok.strip_prefix("0x").unwrap_or(tok);
            // Split continuous hex into 4-character sample groups.
            tok.as_bytes()
                .chunks(4)
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
