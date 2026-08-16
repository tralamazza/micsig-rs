//! Serial bus configuration, the setup half of [`crate::decode`].
//!
//! Every bus type names its parameters differently — the source channel is
//! `UART:RX` but `LIN:CHANnel`, `IIC:SCL` and `SPI:CLK`; the bit rate is
//! `USERbaud` for three types and `BANDrate` for ARINC 429. This module maps
//! one generic set of options onto whichever keywords the selected type wants,
//! and refuses options that type has no equivalent for rather than silently
//! dropping them.
//!
//! All seven types were confirmed on an MHO14-200N (firmware 1.143.72) to
//! accept the commands below and read the value back. Only UART has been
//! verified end to end against real traffic, decoding a 1 kHz square wave at
//! 2000 baud into frames of `0x55`.

use crate::error::{Error, Result};
use crate::transport::Scpi;

/// A serial bus protocol the instrument can decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BusType {
    Uart,
    Lin,
    Spi,
    Can,
    Iic,
    #[value(name = "1553b")]
    Mil1553b,
    #[value(name = "429")]
    Arinc429,
}

impl BusType {
    /// The `<type>` keyword for `:BUS<n>:TYPE`.
    ///
    /// Note the instrument does not echo these back verbatim: `UART` reads
    /// back as `Uart` and `IIC` as `I2C`, so a readback must not be compared
    /// against what was written.
    pub fn keyword(self) -> &'static str {
        match self {
            BusType::Uart => "UART",
            BusType::Lin => "LIN",
            BusType::Spi => "SPI",
            BusType::Can => "CAN",
            BusType::Iic => "IIC",
            BusType::Mil1553b => "1553B",
            BusType::Arinc429 => "429",
        }
    }

    /// The subsystem node its parameters live under, which is the keyword for
    /// every type.
    fn node(self) -> &'static str {
        self.keyword()
    }

    /// The parameter naming the primary input channel.
    fn source_key(self) -> &'static str {
        match self {
            BusType::Uart => "RX",
            BusType::Lin | BusType::Can => "CHANnel",
            BusType::Spi => "CLK",
            BusType::Iic => "SCL",
            BusType::Mil1553b | BusType::Arinc429 => "SOURce",
        }
    }

    /// The parameter naming the second input channel, for the two types that
    /// need a clock and a data line.
    fn data_key(self) -> Option<&'static str> {
        match self {
            BusType::Spi => Some("DATA"),
            BusType::Iic => Some("SDA"),
            _ => None,
        }
    }

    /// The parameter carrying a user-specified bit rate.
    fn baud_key(self) -> Option<&'static str> {
        match self {
            BusType::Uart | BusType::Lin | BusType::Can => Some("USERbaud"),
            BusType::Arinc429 => Some("BANDrate"),
            _ => None,
        }
    }

    /// The parameter carrying the word width in bits.
    fn width_key(self) -> Option<&'static str> {
        match self {
            BusType::Uart | BusType::Spi => Some("WIDTh"),
            _ => None,
        }
    }
}

/// Bus display mode. This is not cosmetic: the two report different frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Timestamps every frame, but only those drawn on screen.
    Grap,
    /// No timestamps, but reaches further back into the capture.
    Txt,
}

impl Mode {
    fn keyword(self) -> &'static str {
        match self {
            Mode::Grap => "GRAP",
            Mode::Txt => "TXT",
        }
    }
}

/// UART parity, `:BUS<n>:UART:CHECk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Parity {
    None,
    Odd,
    Even,
}

/// Idle line level, `:BUS<n>:<type>:IDLElvl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Idle {
    High,
    Low,
}

/// What to configure. Everything is optional; `None` leaves the instrument's
/// current setting alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct Config {
    pub bus_type: Option<BusType>,
    pub source: Option<u8>,
    pub data: Option<u8>,
    pub baud: Option<u32>,
    pub width: Option<u8>,
    pub parity: Option<Parity>,
    pub idle: Option<Idle>,
    pub mode: Option<Mode>,
    pub display: Option<bool>,
}

impl Config {
    /// True if nothing was asked for, so [`apply`] can be skipped entirely and
    /// an already-configured instrument left untouched.
    pub fn is_empty(&self) -> bool {
        self.bus_type.is_none()
            && self.source.is_none()
            && self.data.is_none()
            && self.baud.is_none()
            && self.width.is_none()
            && self.parity.is_none()
            && self.idle.is_none()
            && self.mode.is_none()
            && self.display.is_none()
    }
}

/// Apply `config` to a bus.
///
/// The type is written first, since the parameter keywords depend on it. When
/// no type is given the one already selected on the instrument is read back
/// and used, so `--baud` alone works on a bus that is already set up.
pub fn apply(inst: &mut impl Scpi, bus: u8, config: &Config) -> Result<()> {
    if config.is_empty() {
        return Ok(());
    }

    let bus_type = match config.bus_type {
        Some(t) => {
            inst.send(&format!(":BUS{bus}:TYPE {}", t.keyword()))?;
            t
        }
        None => current_type(inst, bus)?,
    };
    let node = bus_type.node();

    if let Some(ch) = config.source {
        inst.send(&format!(
            ":BUS{bus}:{node}:{} CH{ch}",
            bus_type.source_key()
        ))?;
    }
    if let Some(ch) = config.data {
        let key = bus_type.data_key().ok_or_else(|| {
            unsupported(
                bus_type,
                "--data",
                "it has a single input channel; use --source",
            )
        })?;
        inst.send(&format!(":BUS{bus}:{node}:{key} CH{ch}"))?;
    }
    if let Some(baud) = config.baud {
        let key = bus_type
            .baud_key()
            .ok_or_else(|| unsupported(bus_type, "--baud", "it has no user bit rate"))?;
        inst.send(&format!(":BUS{bus}:{node}:{key} {baud}"))?;
    }
    if let Some(width) = config.width {
        let key = bus_type
            .width_key()
            .ok_or_else(|| unsupported(bus_type, "--width", "its word width is fixed"))?;
        inst.send(&format!(":BUS{bus}:{node}:{key} {width}"))?;
    }
    if let Some(parity) = config.parity {
        if bus_type != BusType::Uart {
            return Err(unsupported(
                bus_type,
                "--parity",
                "only UART has a parity bit",
            ));
        }
        let v = match parity {
            Parity::None => "NONE",
            Parity::Odd => "ODD",
            Parity::Even => "EVEN",
        };
        inst.send(&format!(":BUS{bus}:{node}:CHECk {v}"))?;
    }
    if let Some(idle) = config.idle {
        let v = match idle {
            Idle::High => "high",
            Idle::Low => "low",
        };
        inst.send(&format!(":BUS{bus}:{node}:IDLElvl {v}"))?;
    }
    if let Some(mode) = config.mode {
        inst.send(&format!(":BUS{bus}:MODE {}", mode.keyword()))?;
    }
    if let Some(on) = config.display {
        inst.send(&format!(":BUS{bus}:DISPlay {}", u8::from(on)))?;
    }
    Ok(())
}

/// Read back which protocol a bus is currently set to.
///
/// The reply is not one of the keywords used to set it — `Uart` for `UART`
/// and `I2C` for `IIC` — so this matches case-insensitively on both spellings.
pub fn current_type(inst: &mut impl Scpi, bus: u8) -> Result<BusType> {
    let resp = inst.query(&format!(":BUS{bus}:TYPE?"))?;
    parse_type(&resp).ok_or_else(|| {
        Error::Message(format!(
            "bus {bus} reports an unrecognised type {:?}; set one with --type",
            resp.trim()
        ))
    })
}

/// Map a `:BUS<n>:TYPE?` reply back to a [`BusType`].
pub fn parse_type(resp: &str) -> Option<BusType> {
    match resp.trim().to_ascii_uppercase().as_str() {
        "UART" => Some(BusType::Uart),
        "LIN" => Some(BusType::Lin),
        "SPI" => Some(BusType::Spi),
        "CAN" | "CANFD" => Some(BusType::Can),
        // The instrument answers I2C even though it is set with IIC.
        "IIC" | "I2C" => Some(BusType::Iic),
        "1553B" => Some(BusType::Mil1553b),
        "429" => Some(BusType::Arinc429),
        _ => None,
    }
}

fn unsupported(bus_type: BusType, flag: &str, why: &str) -> Error {
    // "to a IIC bus" reads badly and picking the article per type is not worth
    // it, so name the type without one.
    Error::Message(format!(
        "{flag} does not apply to bus type {}: {why}",
        bus_type.keyword()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instrument does not echo the type keyword it was given.
    #[test]
    fn parses_the_types_the_instrument_reports() {
        assert_eq!(parse_type("Uart"), Some(BusType::Uart));
        assert_eq!(parse_type("I2C"), Some(BusType::Iic));
        assert_eq!(parse_type("IIC"), Some(BusType::Iic));
        assert_eq!(parse_type(" 1553B \n"), Some(BusType::Mil1553b));
        assert_eq!(parse_type("429"), Some(BusType::Arinc429));
        assert_eq!(parse_type("nonsense"), None);
    }

    /// Each type names the same concept differently; getting this mapping
    /// wrong sends a command the instrument rejects.
    #[test]
    fn maps_the_source_channel_per_type() {
        assert_eq!(BusType::Uart.source_key(), "RX");
        assert_eq!(BusType::Lin.source_key(), "CHANnel");
        assert_eq!(BusType::Can.source_key(), "CHANnel");
        assert_eq!(BusType::Spi.source_key(), "CLK");
        assert_eq!(BusType::Iic.source_key(), "SCL");
        assert_eq!(BusType::Arinc429.source_key(), "SOURce");
        // Only the two-wire protocols take a second channel.
        assert_eq!(BusType::Spi.data_key(), Some("DATA"));
        assert_eq!(BusType::Iic.data_key(), Some("SDA"));
        assert_eq!(BusType::Uart.data_key(), None);
        // ARINC 429 spells its bit rate differently from everyone else.
        assert_eq!(BusType::Arinc429.baud_key(), Some("BANDrate"));
        assert_eq!(BusType::Uart.baud_key(), Some("USERbaud"));
        assert_eq!(BusType::Iic.baud_key(), None);
    }

    #[test]
    fn empty_config_changes_nothing() {
        assert!(Config::default().is_empty());
        assert!(
            !Config {
                baud: Some(9600),
                ..Default::default()
            }
            .is_empty()
        );
    }
}
