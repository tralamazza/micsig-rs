//! SCPI transport abstraction over TCP and USB.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::scpi;

/// Default SCPI-raw TCP port used by LXI instruments.
pub const DEFAULT_RAW_PORT: u16 = 5025;

use std::io::{Read, Write};
use std::net::TcpStream;

/// A connection to an instrument over the network.
pub struct Instrument {
    stream: TcpStream,
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
        stream
            .set_write_timeout(Some(timeout))
            .map_err(Error::Io)?;
        stream.set_nodelay(true).map_err(Error::Io)?;
        Ok(Self { stream, timeout })
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
        self.stream.write_all(buf.as_bytes()).map_err(Error::Io)?;
        self.stream.flush().map_err(Error::Io)
    }

    fn query(&mut self, command: &str) -> Result<String> {
        self.send(command)?;
        let mut buf = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        loop {
            match self.stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    buf.push(byte[0]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Err(Error::Timeout(self.timeout.as_secs()));
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        String::from_utf8(buf).map_err(|e| Error::BlockHeader(e.to_string()))
    }

    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>> {
        self.send(command)?;
        scpi::read_response(&mut self.stream, self.timeout)
    }
}
