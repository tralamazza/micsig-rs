//! Automatic measurements via the `:MEASure:*` subsystem.
//!
//! Reading a measurement takes three steps, not one. `:MEASure:<item>? CH1`
//! on its own answers `Error:SCPI param error!` unless the item has first been
//! added with `:MEASure:OPEN <item>,CH1`, and for a short while after that it
//! answers `--` instead of a number. [`read`] does the whole dance: open every
//! requested item, wait, query them, then close them again so the instrument
//! is left as it was found.
//!
//! Two spellings in the programming guide are wrong. It lists the rise and
//! fall items as "RISE time" and "FALL time", with a space; the instrument
//! rejects `RISE` and accepts `RISetime`. `FALL` happens to work, so the
//! inconsistency is the firmware's, not a transcription slip.

use std::thread::sleep;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::transport::Scpi;

/// How long to wait after `:MEASure:OPEN` before the value is available.
///
/// Measured on an MHO14-200N (firmware 1.143.72) over a batch of eight items:
/// with no wait only five had a value, at 200 ms all eight did. 400 ms is
/// double the smallest interval that worked.
const SETTLE: Duration = Duration::from_millis(400);

/// A measurement the instrument can compute, with the keyword it answers to.
///
/// Every variant here was confirmed to return a value on an MHO14-200N
/// against the 1 kHz probe-compensation square wave. The guide also lists
/// `ACRMS`, which only ever answered `--`, and `+RATE`/`-RATE`, which are
/// rejected outright; those are left out. `DELAy` and `PHASe` are omitted for
/// a different reason — they take two sources, which does not fit the
/// one-channel shape of this command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Item {
    /// Frequency, Hz.
    Freq,
    /// Period, s.
    Period,
    /// Peak-to-peak amplitude, V.
    Pkpk,
    /// Amplitude between the high and low levels, V.
    Amp,
    /// Maximum sample, V.
    Max,
    /// Minimum sample, V.
    Min,
    /// Top (logic high) level, V.
    High,
    /// Base (logic low) level, V.
    Low,
    /// Arithmetic mean, V.
    Mean,
    /// Mean over one cycle, V.
    Cmean,
    /// Root mean square, V.
    Rms,
    /// RMS over one cycle, V.
    Crms,
    /// Rise time, s.
    Rise,
    /// Fall time, s.
    Fall,
    /// Positive duty cycle, as a fraction.
    Pduty,
    /// Negative duty cycle, as a fraction.
    Nduty,
    /// Positive pulse width, s.
    Pwidth,
    /// Negative pulse width, s.
    Nwidth,
    /// Burst width, s.
    Burst,
    /// Rising-edge overshoot, as a fraction.
    Rov,
    /// Falling-edge overshoot, as a fraction.
    Fov,
}

impl Item {
    /// The `<item>` keyword this is known by on the wire.
    pub fn keyword(self) -> &'static str {
        match self {
            Item::Freq => "FREQ",
            Item::Period => "PERiod",
            Item::Pkpk => "PKPK",
            Item::Amp => "AMP",
            Item::Max => "MAX",
            Item::Min => "MIN",
            Item::High => "HIGH",
            Item::Low => "LOW",
            Item::Mean => "MEAN",
            Item::Cmean => "CMEAn",
            Item::Rms => "RMS",
            Item::Crms => "CRMS",
            // Not "RISE"; see the module docs.
            Item::Rise => "RISetime",
            Item::Fall => "FALL",
            Item::Pduty => "PDUTy",
            Item::Nduty => "NDUTy",
            Item::Pwidth => "PWIDth",
            Item::Nwidth => "NWIDth",
            Item::Burst => "BURStw",
            Item::Rov => "ROV",
            Item::Fov => "FOV",
        }
    }

    /// The unit the value is expressed in, for display. Duty cycles and
    /// overshoot come back as fractions and carry no unit.
    pub fn unit(self) -> &'static str {
        match self {
            Item::Freq => "Hz",
            Item::Period | Item::Rise | Item::Fall | Item::Pwidth | Item::Nwidth | Item::Burst => {
                "s"
            }
            Item::Pkpk
            | Item::Amp
            | Item::Max
            | Item::Min
            | Item::High
            | Item::Low
            | Item::Mean
            | Item::Cmean
            | Item::Rms
            | Item::Crms => "V",
            Item::Pduty | Item::Nduty | Item::Rov | Item::Fov => "",
        }
    }

    /// Every item this module knows how to read.
    pub fn all() -> &'static [Item] {
        &[
            Item::Freq,
            Item::Period,
            Item::Pkpk,
            Item::Amp,
            Item::Max,
            Item::Min,
            Item::High,
            Item::Low,
            Item::Mean,
            Item::Cmean,
            Item::Rms,
            Item::Crms,
            Item::Rise,
            Item::Fall,
            Item::Pduty,
            Item::Nduty,
            Item::Pwidth,
            Item::Nwidth,
            Item::Burst,
            Item::Rov,
            Item::Fov,
        ]
    }

    /// The set reported when none is asked for: enough to characterise a
    /// periodic signal at a glance.
    pub fn defaults() -> &'static [Item] {
        &[
            Item::Freq,
            Item::Period,
            Item::Pkpk,
            Item::Amp,
            Item::Max,
            Item::Min,
            Item::Mean,
            Item::Rms,
            Item::Pduty,
            Item::Rise,
            Item::Fall,
        ]
    }
}

/// One measurement result. `value` is `None` when the instrument answered
/// `--`, which it does when it cannot compute the item from the current
/// trace rather than when anything has gone wrong.
#[derive(Debug, Clone, Copy)]
pub struct Measurement {
    pub item: Item,
    pub value: Option<f64>,
}

/// How many measurement items the instrument will hold open at once.
///
/// Measured on an MHO14-200N (firmware 1.143.72) by opening a growing set and
/// querying each: 10 of 10 answer, the 11th and 12th return `--` no matter how
/// long they are given to settle. [`read`] works in chunks of this size, so
/// asking for more items costs another round of open/settle/close rather than
/// silently dropping the surplus.
const MAX_OPEN: usize = 10;

/// Read `items` for a channel.
///
/// Items are opened in batches so the settle is paid once per batch rather
/// than once per item. The instrument is restored afterwards: anything this
/// opened is closed again, including when a query fails part way through.
pub fn read(inst: &mut impl Scpi, channel: u8, items: &[Item]) -> Result<Vec<Measurement>> {
    crate::waveform::ensure_channel_enabled(inst, channel)?;

    let mut out = Vec::with_capacity(items.len());
    for batch in items.chunks(MAX_OPEN) {
        for item in batch {
            inst.send(&format!(":MEASure:OPEN {},CH{channel}", item.keyword()))?;
        }
        sleep(SETTLE);

        let result = query_all(inst, channel, batch);

        // Close on the way out whatever happened, so repeated runs do not pile
        // up measurement items on the instrument's display — and so the next
        // batch starts from an empty slate rather than hitting the limit.
        for item in batch {
            let _ = inst.send(&format!(":MEASure:CLOSe {},CH{channel}", item.keyword()));
        }
        out.extend(result?);
    }
    Ok(out)
}

fn query_all(inst: &mut impl Scpi, channel: u8, items: &[Item]) -> Result<Vec<Measurement>> {
    let mut out = Vec::with_capacity(items.len());
    for &item in items {
        let resp = inst.query(&format!(":MEASure:{}? CH{channel}", item.keyword()))?;
        out.push(Measurement {
            item,
            value: parse_value(&resp)?,
        });
    }
    Ok(out)
}

/// Interpret one `:MEASure:<item>?` response.
///
/// `--` means the instrument has no value for this item, which is a result
/// rather than a failure. An `Error:` reply is a failure, and so is anything
/// that is neither — better to say so than to report a silent `None`.
pub fn parse_value(resp: &str) -> Result<Option<f64>> {
    let resp = resp.trim();
    if resp == "--" || resp.is_empty() {
        return Ok(None);
    }
    if let Some(rest) = resp.strip_prefix("Error:") {
        return Err(Error::Message(format!(
            "instrument rejected the measurement:{rest}"
        )));
    }
    resp.parse::<f64>()
        .map(Some)
        .map_err(|_| Error::Message(format!("unparseable measurement value {resp:?}")))
}

/// Format a value with an SI prefix, e.g. `999.996 Hz`, `2.098 V`, `1.000 ms`.
/// Unitless items are shown as a percentage, which is how the instrument's
/// own display presents duty cycle and overshoot.
pub fn format_value(item: Item, value: Option<f64>) -> String {
    let Some(v) = value else {
        return "--".to_string();
    };
    if item.unit().is_empty() {
        return format!("{:.2} %", v * 100.0);
    }
    let (scaled, prefix) = si_scale(v);
    format!("{scaled:.4} {prefix}{}", item.unit())
}

/// Scale a value into the engineering range `[1, 1000)` and return the SI
/// prefix that goes with it. Zero and non-finite values are left alone.
fn si_scale(v: f64) -> (f64, &'static str) {
    const PREFIXES: [(f64, &str); 9] = [
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1e0, ""),
        (1e-3, "m"),
        (1e-6, "u"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
    ];
    if v == 0.0 || !v.is_finite() {
        return (v, "");
    }
    let mag = v.abs();
    for (factor, prefix) in PREFIXES {
        if mag >= factor {
            return (v / factor, prefix);
        }
    }
    (v, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values_and_the_unavailable_marker() {
        assert_eq!(
            parse_value("999.995849609375").unwrap(),
            Some(999.995849609375)
        );
        assert_eq!(
            parse_value("9.999998146668077E-4").unwrap(),
            Some(9.999998146668077E-4)
        );
        assert_eq!(
            parse_value("-0.04593298211693764").unwrap(),
            Some(-0.04593298211693764)
        );
        // `--` is "no value for this item", not an error.
        assert_eq!(parse_value("--").unwrap(), None);
        assert_eq!(parse_value("  --  ").unwrap(), None);
        assert_eq!(parse_value("").unwrap(), None);
    }

    #[test]
    fn rejects_error_replies_rather_than_reporting_no_value() {
        let err = parse_value("Error:SCPI param error!").unwrap_err();
        assert!(err.to_string().contains("param error"), "got: {err}");
        assert!(parse_value("banana").is_err());
    }

    #[test]
    fn formats_with_si_prefixes() {
        assert_eq!(
            format_value(Item::Freq, Some(999.995849609375)),
            "999.9958 Hz"
        );
        assert_eq!(format_value(Item::Period, Some(1.0000035e-3)), "1.0000 ms");
        assert_eq!(
            format_value(Item::Pkpk, Some(2.0976061820983887)),
            "2.0976 V"
        );
        assert_eq!(format_value(Item::Rise, Some(2.7549e-6)), "2.7549 us");
        // Duty cycle arrives as a fraction and is shown as a percentage.
        assert_eq!(
            format_value(Item::Pduty, Some(0.4991520345211029)),
            "49.92 %"
        );
        assert_eq!(format_value(Item::Max, None), "--");
    }

    /// The guide writes these with a space ("RISE time"); the instrument
    /// rejects `RISE` and accepts `RISetime`.
    /// `all()` is hand-written, so it can drift from the enum. clap derives
    /// its variant list from the enum itself, which makes it the reference.
    #[test]
    fn all_lists_every_variant() {
        use clap::ValueEnum;
        assert_eq!(Item::all(), Item::value_variants());
    }

    #[test]
    fn rise_uses_the_spelling_the_instrument_accepts() {
        assert_eq!(Item::Rise.keyword(), "RISetime");
        assert_eq!(Item::Fall.keyword(), "FALL");
    }
}
