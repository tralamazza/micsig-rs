use std::io;
use std::time::Duration;

use thiserror::Error;

/// Errors produced while talking to an instrument.
#[derive(Debug, Error)]
pub enum Error {
    #[error("connection failed: {0}")]
    Connect(#[source] io::Error),

    #[error("I/O error: {0}")]
    Io(#[source] io::Error),

    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),

    #[error("USB: {0}")]
    UsbMsg(String),

    #[error("failed to parse SCPI block header: {0}")]
    BlockHeader(String),

    #[error("block length mismatch: expected {expected} bytes, read {actual}")]
    BlockLength { expected: usize, actual: usize },

    #[error("instrument closed the connection before sending a response")]
    Eof,

    #[error("timeout ({}s) waiting for response", .0.as_secs_f64())]
    Timeout(Duration),

    #[error("invalid preamble from instrument: {0}")]
    Preamble(String),

    #[error("response is not valid UTF-8: {0}")]
    Encoding(String),

    #[error("could not resolve address '{0}'")]
    Resolve(String),
}

pub type Result<T> = std::result::Result<T, Error>;
