//! Device discovery.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
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
    // connect_timeout, not connect: a filtered port would otherwise hang for
    // the OS default (~75s) regardless of --timeout.
    let stream =
        TcpStream::connect_timeout(&address, timeout).map_err(crate::error::Error::Connect)?;
    let mut inst = Instrument::from_stream(stream, timeout)?;
    inst.idn()
}

/// Resolve a host (IP literal or hostname) to its candidate socket addresses.
pub fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| crate::error::Error::Resolve(host.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(crate::error::Error::Resolve(host.to_string()));
    }
    Ok(addrs)
}

/// Scan a single host and report whether it responds to `*IDN?`. Accepts both
/// IP literals and hostnames; every resolved address is tried in turn.
pub fn probe_host(host: &str, port: u16, timeout: Duration) -> Option<Device> {
    for addr in resolve(host, port).ok()? {
        if let Ok(id) = probe(addr, timeout)
            && !id.trim().is_empty()
        {
            return Some(Device { address: addr, id });
        }
    }
    None
}

/// Scan a base address across a range of ports.
pub fn scan_ports(host: &str, ports: &[u16], timeout: Duration) -> Vec<Device> {
    ports
        .iter()
        .filter_map(|&p| probe_host(host, p, timeout))
        .collect()
}
