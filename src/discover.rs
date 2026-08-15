//! Device discovery.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::Result;
use crate::transport::{Instrument, Scpi};

/// A discovered instrument.
#[derive(Debug, Clone)]
pub struct Device {
    pub address: SocketAddr,
    pub id: String,
}

/// Try to read the `*IDN?` of an instrument at the given address.
fn probe(address: SocketAddr, timeout: Duration) -> Result<String> {
    let stream = TcpStream::connect(address).map_err(crate::error::Error::Connect)?;
    let mut inst = Instrument::from_stream(stream, timeout)?;
    inst.idn()
}

/// Scan a single IP address (a `host:port` string) and report whether it
/// responds to `*IDN?`.
pub fn probe_host(host: &str, port: u16, timeout: Duration) -> Option<Device> {
    let addr = format!("{host}:{port}").parse().ok()?;
    match probe(addr, timeout) {
        Ok(id) if !id.trim().is_empty() => Some(Device { address: addr, id }),
        _ => None,
    }
}

/// Scan a base address across a range of ports.
pub fn scan_ports(host: &str, ports: &[u16], timeout: Duration) -> Vec<Device> {
    ports
        .iter()
        .filter_map(|&p| probe_host(host, p, timeout))
        .collect()
}
