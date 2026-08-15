use std::io;

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

    #[error("timeout ({0}s) waiting for response")]
    Timeout(u64),

    #[error("invalid preamble from instrument: {0}")]
    Preamble(String),
}

pub type Result<T> = std::result::Result<T, Error>;
