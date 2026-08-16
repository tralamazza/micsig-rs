//! SCPI transport abstraction over TCP and USB.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::scpi;

/// Default SCPI-raw TCP port used by LXI instruments.
pub const DEFAULT_RAW_PORT: u16 = 5025;

use std::io::{BufReader, Write};
use std::net::TcpStream;

/// A connection to an instrument over the network.
///
/// Reads go through a `BufReader`: the SCPI framing needs byte-at-a-time
/// scanning, and unbuffered that costs one syscall per byte.
pub struct Instrument {
    reader: BufReader<TcpStream>,
    timeout: Duration,
}

impl Instrument {
    /// Connect to a raw SCPI-raw TCP socket.
    pub fn connect(address: &str, port: u16, timeout: Duration) -> Result<Self> {
        let stream = TcpStream::connect((address, port)).map_err(Error::Connect)?;
        Self::from_stream(stream, timeout)
    }

    /// Wrap an already-connected stream.
    pub fn from_stream(stream: TcpStream, timeout: Duration) -> Result<Self> {
        stream.set_read_timeout(Some(timeout)).map_err(Error::Io)?;
        stream.set_write_timeout(Some(timeout)).map_err(Error::Io)?;
        stream.set_nodelay(true).map_err(Error::Io)?;
        Ok(Self {
            reader: BufReader::with_capacity(64 * 1024, stream),
            timeout,
        })
    }
}

/// Common SCPI operations shared by all transports.
pub trait Scpi {
    fn send(&mut self, command: &str) -> Result<()>;
    fn query(&mut self, command: &str) -> Result<String>;
    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>>;

    /// Query the instrument identification string (`*IDN?`).
    fn idn(&mut self) -> Result<String> {
        self.query("*IDN?")
    }
}

impl Scpi for Instrument {
    fn send(&mut self, command: &str) -> Result<()> {
        let mut buf = String::with_capacity(command.len() + 1);
        buf.push_str(command);
        buf.push('\n');
        let stream = self.reader.get_mut();
        stream.write_all(buf.as_bytes()).map_err(Error::Io)?;
        stream.flush().map_err(Error::Io)
    }

    fn query(&mut self, command: &str) -> Result<String> {
        let mut bytes = self.query_raw(command)?;
        while matches!(bytes.last(), Some(b'\n') | Some(b'\r')) {
            bytes.pop();
        }
        String::from_utf8(bytes).map_err(|e| Error::Encoding(e.to_string()))
    }

    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>> {
        self.send(command)?;
        scpi::read_response(&mut self.reader, self.timeout)
    }
}
