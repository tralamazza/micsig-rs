//! Command-line tool to interface with a Micsig oscilloscope over SCPI.

pub mod benchmark;
pub mod decode;
pub mod discover;
pub mod error;
pub mod measure;
pub mod scpi;
pub mod screenshot;
pub mod transport;
pub mod usb;
pub mod waveform;

pub use error::{Error, Result};
pub use transport::{Instrument, Scpi};
pub use usb::UsbInstrument;
