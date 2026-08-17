//! Segmented capture: a burst of triggers recorded as separate frames.
//!
//! In segmented mode the instrument spends its acquisition memory on many short
//! records rather than one long one, so a train of infrequent events is caught
//! without the dead time between them. `:ACQuire:SEGMented:QTY` sets how many
//! to arm for, `:NO?` counts the ones that have triggered so far, and
//! `:FRA1` picks which one the rest of the instrument — including
//! `:WAVeform:DATA?` — reads.
//!
//! That last part is the whole reason this is only a thin layer: reading a
//! segment is just [`crate::waveform::capture`] after selecting a frame. The
//! frames are genuinely distinct, confirmed on an MHO14-200N (firmware
//! 1.143.72) by exporting frames 1, 5, 9, 13 and 16 of a 16-segment capture and
//! finding five different digests over five equal-length records.

use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::transport::Scpi;

/// Turn segmented acquisition on or off.
pub fn set_enabled(inst: &mut impl Scpi, on: bool) -> Result<()> {
    inst.send(&format!(":ACQuire:SEGMented {}", u8::from(on)))
}

/// True if segmented acquisition is on.
pub fn enabled(inst: &mut impl Scpi) -> Result<bool> {
    Ok(inst.query(":ACQuire:SEGMented?")?.trim() == "1")
}

/// Ask for `count` segments.
///
/// The instrument accepts and echoes back anything given here — 100000 was
/// taken without complaint — so the readback is no guide to what the hardware
/// will actually fill. The guide says only "refer to the data manual".
pub fn set_count(inst: &mut impl Scpi, count: u32) -> Result<()> {
    inst.send(&format!(":ACQuire:SEGMented:QTY {count}"))
}

/// How many segments the instrument is armed for.
pub fn count(inst: &mut impl Scpi) -> Result<u32> {
    parse_count(&inst.query(":ACQuire:SEGMented:QTY?")?, "QTY")
}

/// How many segments have triggered and been stored.
pub fn captured(inst: &mut impl Scpi) -> Result<u32> {
    parse_count(&inst.query(":ACQuire:SEGMented:NO?")?, "NO")
}

/// Select the frame that `:WAVeform:DATA?` and the display will read.
///
/// `:DISType SINGLe` goes with it: `:FRA1` is documented as "the current frame
/// when displaying a single frame", and the fitting display uses the `:FRA2`
/// and `:FRA3` range instead.
///
/// Read the frame back immediately and you get the *previous* one — see
/// [`select_settled`], which is what callers should use.
pub fn select(inst: &mut impl Scpi, frame: u32) -> Result<()> {
    inst.send(":ACQuire:SEGMented:DISType SINGLe")?;
    inst.send(&format!(":ACQuire:SEGMented:FRA1 {frame}"))
}

/// How long [`select_settled`] waits by default.
///
/// The readout does not follow `:FRA1` instantly. Measured on an MHO14-200N
/// (firmware 1.143.72) by selecting five frames of a filled segmented capture
/// in turn and comparing digests against a settled reading: with no wait every
/// frame returned the previously selected one, at 25 ms two of five were still
/// stale, and from 50 ms upwards every frame was correct. This is six times
/// that boundary, and still inside the 0.15–0.5 s the display itself is known
/// to lag a write by.
pub const SETTLE: Duration = Duration::from_millis(300);

/// Select a frame and wait for the readout to catch up with it.
///
/// A plain wait rather than a poll, because nothing observable reports the
/// switch: `:FRA1?` answers with the new frame the moment it is written,
/// while `:WAVeform:DATA?` is still serving the old one, and the only way to
/// tell the two frames apart is to read them — which is the thing being made
/// reliable. Reading a stale frame is silent and plausible, so the margin here
/// is deliberately wide.
pub fn select_settled(inst: &mut impl Scpi, frame: u32, settle: Duration) -> Result<()> {
    select(inst, frame)?;
    if !settle.is_zero() {
        std::thread::sleep(settle);
    }
    Ok(())
}

fn parse_count(resp: &str, what: &str) -> Result<u32> {
    let text = resp.trim();
    text.parse().map_err(|_| {
        Error::Message(format!(
            ":ACQuire:SEGMented:{what}? returned {text:?}, which is not a segment count"
        ))
    })
}

/// How the fill loop in [`arm`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every requested segment triggered.
    Full,
    /// The instrument left the running state with segments to spare.
    Stopped,
    /// No new segment arrived within the stall window.
    Stalled,
    /// The overall deadline passed while segments were still arriving.
    TimedOut,
}

/// The result of arming a segmented capture.
#[derive(Debug, Clone, Copy)]
pub struct Capture {
    /// Segments that triggered.
    pub captured: u32,
    /// Segments asked for.
    pub requested: u32,
    pub outcome: Outcome,
}

impl Capture {
    /// True if fewer segments arrived than were asked for.
    pub fn is_partial(&self) -> bool {
        self.captured < self.requested
    }
}

/// How long after arming nothing the instrument reports can be believed.
///
/// `:ACQuire:SEGMented:NO?` keeps answering with the previous burst's total
/// for a moment after `:MENU:SINGLE`, then bounces while the acquisition engine
/// restarts. Sampled every 40 ms on an MHO14-200N (firmware 1.143.72) after
/// arming 7 segments over a burst that had captured 6: `6, 0, 7, 0, 7, 7, 7`.
/// Believing the leading `6` ends the capture before it has begun, which is
/// what this window exists to prevent. `:TRIGger:STATus?` is stale over the
/// same period.
pub const ARM_SETTLE: Duration = Duration::from_millis(400);

/// Arm a segmented capture and wait for it to fill.
///
/// Returns as soon as `count` segments have triggered, and gives up early if
/// none arrives for `stall` — a burst that has finished early is the normal
/// case, and waiting out the full `deadline` for it would make the common path
/// the slow one. `progress` is called with the running total whenever it grows,
/// for callers that want to show it.
///
/// `:MENU:SINGLE` is what starts a burst, including from a stopped instrument.
/// `:MENU:RUN` does not: on a stopped instrument holding a finished burst it
/// returns to the running state and leaves the segment count exactly where it
/// was.
///
/// The instrument is left stopped either way, which is what reading frames back
/// requires.
pub fn arm(
    inst: &mut impl Scpi,
    count: u32,
    deadline: Duration,
    stall: Duration,
    poll: Duration,
    mut progress: impl FnMut(u32, u32),
) -> Result<Capture> {
    if count == 0 {
        return Err(Error::Message("segment count must be at least 1".into()));
    }
    set_enabled(inst, true)?;
    set_count(inst, count)?;
    inst.send(":MENU:SINGLE")?;
    std::thread::sleep(ARM_SETTLE);

    let started = Instant::now();
    let mut last_change = started;
    let mut seen = 0;
    let outcome = loop {
        // Only ever upward: the count is not monotonic while the burst starts,
        // and a segment already reported cannot be un-captured.
        let now = captured(inst)?;
        if now > seen {
            seen = now;
            last_change = Instant::now();
            progress(seen, count);
        }
        if seen >= count {
            break Outcome::Full;
        }
        // A `STOP` here means the instrument gave up on the rest of the burst;
        // `WAIT` is what it reports while armed and filling.
        if crate::acquire::status(inst)?.eq_ignore_ascii_case("stop") {
            break Outcome::Stopped;
        }
        if last_change.elapsed() >= stall {
            break Outcome::Stalled;
        }
        if started.elapsed() >= deadline {
            break Outcome::TimedOut;
        }
        std::thread::sleep(poll);
    };

    inst.send(":MENU:STOP")?;
    // The count can advance between the last poll and the stop.
    let captured = captured(inst)?.max(seen);
    if captured != seen {
        progress(captured, count);
    }
    Ok(Capture {
        captured,
        requested: count,
        outcome,
    })
}

/// A parsed frame selection.
///
/// A newtype rather than a bare `Vec<u32>` because clap reads a `Vec` field as
/// "one value per occurrence of the flag", which is not what `--frames 1,4,7-9`
/// means: the whole spec is a single value that happens to expand to several
/// frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frames(pub Vec<u32>);

impl std::str::FromStr for Frames {
    type Err = String;

    fn from_str(spec: &str) -> std::result::Result<Self, Self::Err> {
        parse_frames(spec).map(Frames)
    }
}

/// Expand a frame selection like `1,4,7-9` into frame numbers.
///
/// Ranges are inclusive and may run backwards (`9-7` is the same set), out-of
/// order entries are kept in the order written, and duplicates are dropped so
/// that `1-3,2` exports three files rather than four.
pub fn parse_frames(spec: &str) -> std::result::Result<Vec<u32>, String> {
    let mut frames = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (lo, hi) = match part.split_once('-') {
            Some((a, b)) => (frame_number(a)?, frame_number(b)?),
            None => {
                let n = frame_number(part)?;
                (n, n)
            }
        };
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        frames.extend(lo..=hi);
    }
    if frames.is_empty() {
        return Err(format!("no frames in {spec:?}"));
    }
    let mut seen = std::collections::HashSet::new();
    frames.retain(|f| seen.insert(*f));
    Ok(frames)
}

fn frame_number(text: &str) -> std::result::Result<u32, String> {
    let text = text.trim();
    match text.parse() {
        Ok(0) => Err("frames are numbered from 1".to_string()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("invalid frame number {text:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frame_lists_and_ranges() {
        assert_eq!(parse_frames("3").unwrap(), vec![3]);
        assert_eq!(parse_frames("1,4,7").unwrap(), vec![1, 4, 7]);
        assert_eq!(parse_frames("2-5").unwrap(), vec![2, 3, 4, 5]);
        assert_eq!(parse_frames(" 1 , 3-5 ").unwrap(), vec![1, 3, 4, 5]);
        // Written order is kept; a backwards range still ascends.
        assert_eq!(parse_frames("9,1-2").unwrap(), vec![9, 1, 2]);
        assert_eq!(parse_frames("5-3").unwrap(), vec![3, 4, 5]);
        // Overlapping selections export one file each.
        assert_eq!(parse_frames("1-3,2").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn rejects_nonsense_frame_specs() {
        for bad in ["", " ", "0", "1-0", "abc", "1-x", "-", "1..3"] {
            assert!(parse_frames(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_unparsable_counts() {
        assert!(parse_count("16\r\n", "QTY").is_ok());
        assert_eq!(parse_count(" 1003 ", "NO").unwrap(), 1003);
        assert!(parse_count("Error:SCPI param error!", "NO").is_err());
    }

    #[test]
    fn a_short_capture_is_partial() {
        let full = Capture {
            captured: 16,
            requested: 16,
            outcome: Outcome::Full,
        };
        assert!(!full.is_partial());
        assert!(
            Capture {
                captured: 9,
                ..full
            }
            .is_partial()
        );
    }
}
