//! SCPI wire-format helpers: response parsing and IEEE 488.2 block data.

use std::io::Read;
use std::time::Duration;

use crate::error::{Error, Result};

/// Read a complete SCPI response from a stream. Handles two cases:
///
/// 1. A plain text response terminated by a newline.
/// 2. An IEEE 488.2 definite-length block (e.g. `#9000358370<data>`).
///
/// The returned bytes are the raw wire message: for a block response the
/// `#<n><length>` header is *included*, so that callers can apply the right
/// interpretation of the length field (see [`unwrap_block`]). Both this and
/// the USB transport therefore hand back the same shape of data.
///
/// A trailing newline after a block is deliberately not consumed here — doing
/// so costs a full read timeout whenever the instrument does not send one.
/// Any leftover terminator is skipped at the start of the *next* response.
pub fn read_response(stream: &mut impl Read, timeout: Duration) -> Result<Vec<u8>> {
    // Skip terminators left over from a previous response.
    let mut first = [0u8; 1];
    loop {
        read_one(stream, &mut first, timeout)?;
        if first[0] != b'\n' && first[0] != b'\r' {
            break;
        }
    }

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
            Err(e) if is_timeout(&e) => return Err(Error::Timeout(timeout)),
            Err(e) => return Err(Error::Io(e)),
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(buf)
}

/// Read an IEEE 488.2 definite-length block: the `#` has already been consumed.
/// Returns the full message including the reconstructed header.
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

    let mut message = Vec::with_capacity(2 + digits + length);
    message.push(b'#');
    message.push(count[0]);
    message.extend_from_slice(&length_buf);
    message.extend_from_slice(&payload);
    Ok(message)
}

fn read_one(stream: &mut impl Read, out: &mut [u8; 1], timeout: Duration) -> Result<()> {
    match stream.read(out) {
        Ok(0) => Err(Error::Eof),
        Ok(_) => Ok(()),
        Err(e) if is_timeout(&e) => Err(Error::Timeout(timeout)),
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
            Err(e) if is_timeout(&e) => return Err(Error::Timeout(timeout)),
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
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

/// Split a block response into its declared length field and everything that
/// follows the header. Returns `None` if `data` is not a block.
///
/// Note that the meaning of the length field is *command dependent* on Micsig
/// firmware, so this function deliberately does not interpret it:
///
/// - `:SYS:SCR?` reports a byte count.
/// - `:WAVeform:DATA?` reports a **sample** count; the payload is four ASCII
///   hex characters per sample, so it is four times longer than the field
///   suggests (measured on an MHO14-200N, firmware 1.97.70).
pub fn block_parts(data: &[u8]) -> Option<(usize, &[u8])> {
    if data.first() != Some(&b'#') {
        return None;
    }
    let (declared, header_len) = parse_block_header(data).ok()?;
    Some((declared, &data[header_len..]))
}

/// Strip an IEEE 488.2 definite-length block header, treating the length field
/// as a byte count. Correct for `:SYS:SCR?`; see [`block_parts`] for why it is
/// *not* correct for `:WAVeform:DATA?`. Plain text passes through unchanged.
pub fn unwrap_block(data: &[u8]) -> Vec<u8> {
    match block_parts(data) {
        Some((length, payload)) => payload[..length.min(payload.len())].to_vec(),
        None => data.to_vec(),
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

/// Return true if a command string is a query.
///
/// In SCPI the `?` terminates the *header*, so anything after the first space
/// is a parameter and the marker need not be the last character. Testing only
/// for a trailing `?`, as lxi-tools' `question()` helper does, misses every
/// parameterised query — and there are many, since this instrument takes the
/// source as an argument: `:MEASure:PKPK? CH1`, `:BUS1:LEVel? CH1`,
/// `:TRIGger:LIN:DATA? S1`. Those were sent without ever reading the reply,
/// so the value was silently discarded and the next request read it instead.
pub fn is_query(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .is_some_and(|header| header.contains('?'))
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

    /// The `?` ends the header, not the command, so a query that takes its
    /// source as a parameter still expects a reply. Missing these meant the
    /// response was left in the buffer for the next request to pick up.
    #[test]
    fn detects_parameterised_queries() {
        for q in [
            ":MEASure:PKPK? CH1",
            ":MEASure:DELAy? CH2,CH3,FRISe,FRISe",
            ":BUS1:LEVel? CH1",
            ":TRIGger:LIN:DATA? S1",
            "  :MEASure:FREQ?  CH1  ",
        ] {
            assert!(is_query(q), "{q} should be a query");
        }

        // A `?` in a parameter is not a query marker; only the header counts.
        assert!(!is_query(":SYSTem:NAME \"why?\""));
        assert!(!is_query(""));
    }
}
