//! SCPI wire-format helpers: response parsing and IEEE 488.2 block data.

use std::io::Read;
use std::time::Duration;

use crate::error::{Error, Result};

/// Read a complete SCPI response from a stream. Handles two cases:
///
/// 1. A plain text response terminated by a newline.
/// 2. An IEEE 488.2 definite-length block (e.g. `#9000358370<data>`)
///    optionally followed by a trailing newline.
pub fn read_response(stream: &mut impl Read, timeout: Duration) -> Result<Vec<u8>> {
    let mut first = [0u8; 1];
    read_one(stream, &mut first, timeout)?;

    if first[0] == b'#' {
        return read_block(stream, timeout);
    }

    // Plain text response: read the rest of the line.
    let mut buf = first.to_vec();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                buf.push(byte[0]);
            }
            Err(e) if is_timeout(&e) => return Err(Error::Timeout(timeout.as_secs())),
            Err(e) => return Err(Error::Io(e)),
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(buf)
}

/// Read an IEEE 488.2 definite-length block: the `#` has already been consumed.
fn read_block(stream: &mut impl Read, timeout: Duration) -> Result<Vec<u8>> {
    // Read the digit-count byte (e.g. the `9` in `#9...`).
    let mut count = [0u8; 1];
    read_one(stream, &mut count, timeout)?;
    let digits = match count[0] {
        b'0'..=b'9' => (count[0] - b'0') as usize,
        _ => return Err(Error::BlockHeader("missing digit count".into())),
    };

    // Read the length digits.
    let mut length_buf = vec![0u8; digits];
    read_exact_or_timeout(stream, &mut length_buf, timeout)?;
    let length_str = std::str::from_utf8(&length_buf)
        .map_err(|_| Error::BlockHeader("non-ascii length".into()))?;
    let length: usize = length_str
        .trim()
        .parse()
        .map_err(|_| Error::BlockHeader(format!("invalid length '{length_str}'")))?;

    // Read the payload, which may contain arbitrary bytes.
    let mut payload = vec![0u8; length];
    read_exact_or_timeout(stream, &mut payload, timeout)?;

    // Consume an optional trailing newline.
    consume_optional_newline(stream)?;

    Ok(payload)
}

fn read_one(stream: &mut impl Read, out: &mut [u8; 1], timeout: Duration) -> Result<()> {
    match stream.read(out) {
        Ok(0) | Ok(_) => Ok(()),
        Err(e) if is_timeout(&e) => Err(Error::Timeout(timeout.as_secs())),
        Err(e) => Err(Error::Io(e)),
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut
}

fn read_exact_or_timeout(stream: &mut impl Read, out: &mut [u8], timeout: Duration) -> Result<()> {
    let mut filled = 0;
    while filled < out.len() {
        match stream.read(&mut out[filled..]) {
            Ok(0) => {
                return Err(Error::BlockLength {
                    expected: out.len(),
                    actual: filled,
                });
            }
            Ok(n) => filled += n,
            Err(e) if is_timeout(&e) => return Err(Error::Timeout(timeout.as_secs())),
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn consume_optional_newline(stream: &mut impl Read) -> Result<()> {
    let mut byte = [0u8; 1];
    match stream.read(&mut byte) {
        Ok(0) | Ok(_) => Ok(()),
        Err(e) if is_timeout(&e) => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Parse an IEEE 488.2 definite-length block header of the form `#<n><length>`
/// where `<n>` is the number of digits in `<length>`. Returns the payload byte
/// count and the total header length (the `#`, the digit count, and the length
/// digits).
///
/// Example: `#9000358370...` -> `#` + `9` + `000358370` -> 358370 payload bytes.
pub fn parse_block_header(header: &[u8]) -> Result<(usize, usize)> {
    if header.first() != Some(&b'#') {
        return Err(Error::BlockHeader("missing '#'".into()));
    }
    let digit_count = match header.get(1) {
        Some(d @ b'0'..=b'9') => (d - b'0') as usize,
        _ => return Err(Error::BlockHeader("missing digit count".into())),
    };
    let length_start = 2;
    let length_end = length_start + digit_count;
    if header.len() < length_end {
        return Err(Error::BlockHeader("truncated length field".into()));
    }
    let length_str = std::str::from_utf8(&header[length_start..length_end])
        .map_err(|_| Error::BlockHeader("non-ascii length".into()))?;
    let length: usize = length_str
        .trim()
        .parse()
        .map_err(|_| Error::BlockHeader(format!("invalid length '{length_str}'")))?;
    Ok((length, length_end))
}

/// Strip an IEEE 488.2 definite-length block header from a payload if present
/// (Micsig wraps binary payloads like screenshots and waveform data in one).
/// Plain text responses pass through unchanged.
pub fn unwrap_block(data: &[u8]) -> Vec<u8> {
    if data.first() != Some(&b'#') {
        return data.to_vec();
    }
    match parse_block_header(data) {
        Ok((length, header_len)) => {
            let end = (header_len + length).min(data.len());
            data[header_len..end].to_vec()
        }
        Err(_) => data.to_vec(),
    }
}

/// A parsed preamble returned by `:WAVeform:PREamble?`.
#[derive(Debug, Clone, PartialEq)]
pub struct Preamble {
    pub format: i32,
    pub r#type: i32,
    pub count: i32,
    pub x_increment: f64,
    pub x_origin: f64,
    pub x_reference: i32,
    pub y_increment: f64,
    pub y_origin: f64,
    pub y_reference: i32,
}

/// Parse the nine comma-separated values returned by `:WAVeform:PREamble?`.
pub fn parse_preamble(s: &str) -> Result<Preamble> {
    let fields: Vec<&str> = s.split(',').map(str::trim).collect();
    if fields.len() != 9 {
        return Err(Error::Preamble(format!(
            "expected 9 fields, got {}",
            fields.len()
        )));
    }
    let f = |i: usize| -> Result<f64> {
        fields[i]
            .parse::<f64>()
            .map_err(|_| Error::Preamble(format!("non-numeric field {}: '{}'", i, fields[i])))
    };
    Ok(Preamble {
        format: f(0)? as i32,
        r#type: f(1)? as i32,
        count: f(2)? as i32,
        x_increment: f(3)?,
        x_origin: f(4)?,
        x_reference: f(5)? as i32,
        y_increment: f(6)?,
        y_origin: f(7)?,
        y_reference: f(8)? as i32,
    })
}

/// Return true if a command string is a query (ends in `?`), matching the
/// behaviour of lxi-tools' `question()` helper.
pub fn is_query(command: &str) -> bool {
    command.trim_end().ends_with('?')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_block_header() {
        // #210<10 bytes> -> header is '#', '2', '1', '0' (4 bytes)
        let (len, header) = parse_block_header(b"#210").unwrap();
        assert_eq!(len, 10);
        assert_eq!(header, 4);
    }

    #[test]
    fn parses_micsig_screenshot_header() {
        let header = b"#9000358370";
        let (len, header_len) = parse_block_header(header).unwrap();
        assert_eq!(len, 358370);
        assert_eq!(header_len, 11);
    }

    #[test]
    fn rejects_missing_hash() {
        assert!(parse_block_header(b"9000358370").is_err());
    }

    #[test]
    fn rejects_bad_length() {
        assert!(parse_block_header(b"#9zzzzzzzzz").is_err());
    }

    #[test]
    fn parses_preamble() {
        let p = parse_preamble("1,2,1,0.000000,-0.001488,0,0.062500,3.968750,127").unwrap();
        assert_eq!(p.format, 1);
        assert_eq!(p.r#type, 2);
        assert_eq!(p.x_increment, 0.0);
        assert_eq!(p.y_increment, 0.0625);
        assert_eq!(p.y_origin, 3.96875);
        assert_eq!(p.y_reference, 127);
    }

    #[test]
    fn rejects_short_preamble() {
        assert!(parse_preamble("1,2,3").is_err());
    }

    #[test]
    fn detects_queries() {
        assert!(is_query("*IDN?"));
        assert!(is_query(":WAVeform:DATA?"));
        assert!(!is_query(":MENU:RUN"));
        assert!(!is_query(":CHANnel1:SCALe 0.5"));
    }
}
