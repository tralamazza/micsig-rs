//! Serial bus decode readout.
//!
//! Built on `:BUS<n>:DATA?`, which is **not in the programming guide** but is
//! implemented by the MHO14-200N (firmware 1.97.70). It reports far more than
//! the display shows — up to 1000 frames in testing.
//!
//! The response is semicolon-separated records of comma-separated fields, led
//! by a header row. Both the shape *and the number of frames* depend on
//! `:BUS<n>:MODE`, so neither mode dominates:
//!
//! - `GRAP`: `BeginX,EndX,Data,Color;0s,3.7ms,55,0xffadbdcc;...`
//!   Timestamps each frame, but only reports those drawn on screen.
//! - `TXT`:  `Ch,Data,Color;S1,55,0xffadbdcc;...`
//!   No timestamps, but reaches further back into the capture.
//!
//! Measured at two timebases, GRAP returned 5 frames and TXT 25 from the same
//! acquisition, so pick the mode for what you need. Both work whether or not
//! the bus is actually displayed.

use crate::error::{Error, Result};
use crate::transport::Scpi;

/// Read the decoded frames for a bus (1 or 2).
pub fn read(inst: &mut impl Scpi, bus: u8) -> Result<String> {
    let resp = inst.query(&format!(":BUS{bus}:DATA?"))?;
    if resp.starts_with("Error:") {
        return Err(Error::Message(format!(
            "instrument rejected the decode query for bus {bus}: {resp}. \
             Configure it first, e.g. `:BUS{bus}:TYPE UART`"
        )));
    }
    Ok(resp)
}

/// Convert a `:BUS<n>:DATA?` response into CSV, one record per line.
///
/// The instrument already uses commas between fields, so this only has to
/// turn the record separator into newlines. Empty trailing records, which
/// appear when the response ends in `;`, are dropped.
pub fn to_csv(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 16);
    for record in raw.split(';') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        out.push_str(record);
        out.push('\n');
    }
    out
}

/// Number of decoded frames in a response, excluding the header row.
pub fn frame_count(raw: &str) -> usize {
    to_csv(raw).lines().skip(1).count()
}

/// Read a bus, waiting for the decoder to catch up.
///
/// After the bus is reconfigured the instrument needs a moment before
/// `:BUS<n>:DATA?` has anything in it — measured at ~0.49 s on an MHO14-200N
/// (firmware 1.143.72) when switching a bus cold, against ~0.05 s when it was
/// already decoding the same signal. A fixed wait sat right on that boundary
/// and failed intermittently, so poll instead and stop as soon as frames
/// appear. The last response is returned either way, so an empty bus is still
/// reported by the caller rather than swallowed here.
pub fn read_settled(
    inst: &mut impl Scpi,
    bus: u8,
    attempts: usize,
    delay: std::time::Duration,
) -> Result<String> {
    let mut last = String::new();
    for attempt in 0..attempts.max(1) {
        if attempt > 0 && !delay.is_zero() {
            std::thread::sleep(delay);
        }
        last = read(inst, bus)?;
        if frame_count(&last) > 0 {
            break;
        }
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_text_mode_records() {
        let raw = "Ch,Data,Color;S1,55,0xffadbdcc;S1,AA,0xffadbdcc;";
        assert_eq!(
            to_csv(raw),
            "Ch,Data,Color\nS1,55,0xffadbdcc\nS1,AA,0xffadbdcc\n"
        );
        assert_eq!(frame_count(raw), 2);
    }

    #[test]
    fn converts_graphic_mode_records_with_timestamps() {
        let raw = "BeginX,EndX,Data,Color;0s,3.7ms,55,0xffadbdcc;4.2ms,8.7ms,55,0xffadbdcc";
        let csv = to_csv(raw);
        assert_eq!(csv.lines().next().unwrap(), "BeginX,EndX,Data,Color");
        assert_eq!(csv.lines().nth(1).unwrap(), "0s,3.7ms,55,0xffadbdcc");
        assert_eq!(frame_count(raw), 2);
    }

    #[test]
    fn header_only_response_has_no_frames() {
        assert_eq!(frame_count("Ch,Data,Color"), 0);
        assert_eq!(frame_count(""), 0);
    }
}
