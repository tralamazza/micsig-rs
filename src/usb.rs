//! USBTMC transport over `rusb`.
//!
//! Based on `rust-usbtmc` (MIT OR Apache-2.0, (c) Rogério Adriano), heavily
//! adapted for Micsig scopes:
//! - selects the USBTMC interface (0xFE/0x03) instead of the first bulk one
//! - treats `kernel_driver_active` as unreliable on macOS
//! - retries reads on STALL (the scope STALLs the IN endpoint until data is
//!   ready) and truncates the first chunk to the declared transfer size.

use std::time::{Duration, Instant};

use rusb::{Context, DeviceHandle, Direction, TransferType, UsbContext};

use crate::error::{Error, Result};
use crate::transport::Scpi;

const MSGID_DEV_DEP_MSG_OUT: u8 = 1;
const MSGID_DEV_DEP_MSG_IN: u8 = 2;
const HEADER_BYTES: usize = 12;
const READ_BUFFER_SIZE: usize = 256 * 1024;

/// Micsig USB vendor/product IDs.
pub const MICSIG_VID: u16 = 0x18d1;
pub const MICSIG_PID: u16 = 0x0007;

#[derive(Debug, Clone, Copy)]
struct Endpoint {
    iface: u8,
    address: u8,
}

struct Connection {
    handle: DeviceHandle<Context>,
    out_endpoint: Endpoint,
    in_endpoint: Endpoint,
    detached_ifaces: Vec<u8>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for iface in &self.detached_ifaces {
            self.handle.attach_kernel_driver(*iface).ok();
        }
    }
}

/// A Micsig instrument seen on the USB bus.
#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub bus: u8,
    pub address: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: Option<String>,
    pub serial: Option<String>,
    /// False when the device could not be opened, typically a missing udev
    /// rule on Linux. It is still listed, but cannot be talked to.
    pub accessible: bool,
}

/// Enumerate Micsig instruments attached over USB.
///
/// Descriptor strings need the device to be opened, which can fail on a
/// permission-restricted bus; the device is still reported in that case, just
/// without its product and serial strings.
pub fn list_instruments() -> Result<Vec<UsbDevice>> {
    let context = Context::new().map_err(Error::Usb)?;
    let mut found = Vec::new();

    for device in context.devices().map_err(Error::Usb)?.iter() {
        let Ok(desc) = device.device_descriptor() else {
            continue;
        };
        if desc.vendor_id() != MICSIG_VID || desc.product_id() != MICSIG_PID {
            continue;
        }
        let (product, serial, accessible) = match device.open() {
            Ok(handle) => (
                handle.read_product_string_ascii(&desc).ok(),
                handle.read_serial_number_string_ascii(&desc).ok(),
                true,
            ),
            Err(_) => (None, None, false),
        };
        found.push(UsbDevice {
            bus: device.bus_number(),
            address: device.address(),
            vendor_id: desc.vendor_id(),
            product_id: desc.product_id(),
            product,
            serial,
            accessible,
        });
    }

    Ok(found)
}

/// A USBTMC connection to an instrument.
pub struct UsbInstrument {
    last_btag: u8,
    max_transfer_size: usize,
    timeout: Duration,
    connection: Option<Connection>,
}

impl UsbInstrument {
    /// Connect to the first Micsig instrument on the USB bus.
    pub fn connect(timeout: Duration) -> Result<Self> {
        let mut inst = Self {
            last_btag: 0,
            max_transfer_size: 4 * 1024 * 1024,
            timeout,
            connection: None,
        };
        inst.ensure_connection()?;
        Ok(inst)
    }

    fn next_btag(&mut self) -> u8 {
        self.last_btag = (self.last_btag % 255) + 1;
        self.last_btag
    }

    fn pack_dev_dep_msg_out_header(&mut self, transfer_size: usize, eom: bool) -> Vec<u8> {
        let btag = self.next_btag();
        let mut hdr = vec![MSGID_DEV_DEP_MSG_OUT, btag, !btag, 0x00];
        hdr.extend_from_slice(&(transfer_size as u32).to_le_bytes());
        hdr.push(u8::from(eom));
        hdr.extend_from_slice(&[0x00; 3]);
        hdr
    }

    fn pack_dev_dep_msg_in_header(&mut self, transfer_size: usize) -> Vec<u8> {
        let btag = self.next_btag();
        let mut hdr = vec![MSGID_DEV_DEP_MSG_IN, btag, !btag, 0x00];
        hdr.extend_from_slice(&(transfer_size as u32).to_le_bytes());
        hdr.extend_from_slice(&[0x00; 4]);
        hdr
    }

    fn find_endpoint(
        config_desc: &rusb::ConfigDescriptor,
        transfer_type: TransferType,
        direction: Direction,
    ) -> Option<Endpoint> {
        const USBTMC_CLASS: u8 = 0xFE;
        const USBTMC_SUBCLASS: u8 = 0x03;

        // Prefer the USBTMC (test & measurement) interface.
        for interface in config_desc.interfaces() {
            for interface_desc in interface.descriptors() {
                if interface_desc.class_code() != USBTMC_CLASS
                    || interface_desc.sub_class_code() != USBTMC_SUBCLASS
                {
                    continue;
                }
                for endpoint_desc in interface_desc.endpoint_descriptors() {
                    if endpoint_desc.transfer_type() == transfer_type
                        && endpoint_desc.direction() == direction
                    {
                        return Some(Endpoint {
                            iface: interface_desc.interface_number(),
                            address: endpoint_desc.address(),
                        });
                    }
                }
            }
        }

        // Fall back to the first matching endpoint on any interface.
        for interface in config_desc.interfaces() {
            for interface_desc in interface.descriptors() {
                for endpoint_desc in interface_desc.endpoint_descriptors() {
                    if endpoint_desc.transfer_type() == transfer_type
                        && endpoint_desc.direction() == direction
                    {
                        return Some(Endpoint {
                            iface: interface_desc.interface_number(),
                            address: endpoint_desc.address(),
                        });
                    }
                }
            }
        }

        None
    }

    /// Find the Micsig scope, claim its bulk endpoints, and cache the handle.
    fn ensure_connection(&mut self) -> Result<()> {
        if self.connection.is_some() {
            return Ok(());
        }

        let context = Context::new().map_err(Error::Usb)?;
        let devices = context.devices().map_err(Error::Usb)?;

        // Why a matching device could not be used. Reported instead of a bare
        // "not found", which on Linux sends people hunting for the wrong
        // problem when the real one is a missing udev rule.
        let mut rejected: Option<String> = None;

        for device in devices.iter() {
            let device_desc = match device.device_descriptor() {
                Ok(d) => d,
                Err(_) => continue,
            };
            if device_desc.vendor_id() != MICSIG_VID || device_desc.product_id() != MICSIG_PID {
                continue;
            }

            let config_desc = match device.active_config_descriptor() {
                Ok(c) => c,
                Err(e) => {
                    rejected.get_or_insert(format!("cannot read its configuration ({e})"));
                    continue;
                }
            };

            let out_endpoint =
                Self::find_endpoint(&config_desc, TransferType::Bulk, Direction::Out);
            let in_endpoint = Self::find_endpoint(&config_desc, TransferType::Bulk, Direction::In);
            let (Some(out_endpoint), Some(in_endpoint)) = (out_endpoint, in_endpoint) else {
                rejected.get_or_insert("it exposes no bulk endpoint pair".to_string());
                continue;
            };

            let handle = match device.open() {
                Ok(h) => h,
                Err(e) => {
                    rejected.get_or_insert(match e {
                        rusb::Error::Access => format!(
                            "permission denied ({e}). On Linux, install a udev rule for \
                             {MICSIG_VID:04x}:{MICSIG_PID:04x} or run as root — \
                             see docs/linux-setup.md"
                        ),
                        _ => format!("cannot open it ({e})"),
                    });
                    continue;
                }
            };

            // Lets libusb detach the kernel driver on claim and reattach on
            // release. On Linux the in-tree usbtmc driver binds this exact
            // interface class, so without this the claim below fails with
            // "resource busy". Unsupported on macOS, where it is a no-op.
            let auto_detach = handle.set_auto_detach_kernel_driver(true).is_ok();

            let mut detached_ifaces = Vec::new();
            let mut claimed = Vec::new();
            for iface in [out_endpoint.iface, in_endpoint.iface] {
                if claimed.contains(&iface) {
                    continue;
                }
                // kernel_driver_active is unreliable on macOS (reports true for
                // interfaces owned by user-space daemons, where detach fails).
                if !auto_detach
                    && handle.kernel_driver_active(iface).unwrap_or(false)
                    && handle.detach_kernel_driver(iface).is_ok()
                {
                    detached_ifaces.push(iface);
                }
                handle.claim_interface(iface).map_err(|e| match e {
                    rusb::Error::Busy => Error::UsbMsg(format!(
                        "interface {iface} is busy ({e}); another process or the kernel \
                         usbtmc driver holds it"
                    )),
                    rusb::Error::Access => Error::UsbMsg(format!(
                        "permission denied claiming interface {iface} ({e}); \
                         see docs/linux-setup.md"
                    )),
                    other => Error::Usb(other),
                })?;
                claimed.push(iface);
            }

            self.connection = Some(Connection {
                handle,
                out_endpoint,
                in_endpoint,
                detached_ifaces,
            });
            return Ok(());
        }

        Err(Error::UsbMsg(match rejected {
            Some(reason) => format!(
                "found a Micsig instrument ({MICSIG_VID:04x}:{MICSIG_PID:04x}) but {reason}"
            ),
            None => "Micsig instrument not found on USB bus".to_string(),
        }))
    }

    /// The live connection. Callers run `ensure_connection` first, so this is
    /// an error rather than a panic only to keep a refactor from introducing
    /// one.
    fn connection(&self) -> Result<&Connection> {
        self.connection
            .as_ref()
            .ok_or_else(|| Error::UsbMsg("not connected".into()))
    }

    fn write_data(&mut self, data: &[u8]) -> Result<()> {
        self.ensure_connection()?;
        let timeout = self.timeout;
        let max_transfer_size = self.max_transfer_size;

        let mut offset = 0;
        loop {
            let remaining = data.len() - offset;
            let chunk_size = remaining.min(max_transfer_size);
            let eom = offset + chunk_size == data.len();

            let mut req = self.pack_dev_dep_msg_out_header(chunk_size, eom);
            req.extend_from_slice(&data[offset..offset + chunk_size]);
            let padding = (4 - (chunk_size % 4)) % 4;
            req.resize(req.len() + padding, 0x00);

            let connection = self.connection()?;
            connection
                .handle
                .write_bulk(connection.out_endpoint.address, &req, timeout)
                .map_err(Error::Usb)?;

            offset += chunk_size;
            if offset >= data.len() {
                break;
            }
        }

        Ok(())
    }

    fn send_command(&mut self, command: &[u8]) -> Result<()> {
        self.write_data(command)?;
        // Request the response: the scope only delivers data on bulk IN after
        // the host sends a DEV_DEP_MSG_IN request header.
        let req = self.pack_dev_dep_msg_in_header(self.max_transfer_size);
        let connection = self.connection()?;
        connection
            .handle
            .write_bulk(connection.out_endpoint.address, &req, self.timeout)
            .map_err(Error::Usb)?;
        Ok(())
    }

    fn read_bytes(&mut self) -> Result<Vec<u8>> {
        self.ensure_connection()?;
        let deadline = Instant::now() + self.timeout;

        let connection = self.connection()?;
        let in_ep = connection.in_endpoint.address;

        let mut buf = vec![0u8; READ_BUFFER_SIZE];

        // Read the first chunk and parse the 12-byte response header.
        let first = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(self.timeout));
            }
            match connection.handle.read_bulk(in_ep, &mut buf, remaining) {
                Ok(n) => break n,
                Err(rusb::Error::Pipe) => {
                    // Device STALLs the IN endpoint when no data is ready.
                    connection.handle.clear_halt(in_ep).ok();
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(rusb::Error::Timeout) => {}
                Err(e) => return Err(Error::Usb(e)),
            }
        };

        if first < HEADER_BYTES {
            return Err(Error::BlockHeader("short USBTMC response".into()));
        }

        let transfer_size = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        let mut message = Vec::with_capacity(transfer_size);
        // The first chunk may contain padding beyond the declared payload.
        let first_payload = (first - HEADER_BYTES).min(transfer_size);
        message.extend_from_slice(&buf[HEADER_BYTES..HEADER_BYTES + first_payload]);

        // Keep reading until the full payload has arrived.
        while message.len() < transfer_size {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(self.timeout));
            }
            let want = (transfer_size - message.len()).min(READ_BUFFER_SIZE);
            // Bulk IN buffers must stay a multiple of the max packet size or
            // libusb reports Overflow when the device sends a full packet.
            let want = want.next_multiple_of(512).min(READ_BUFFER_SIZE);
            match connection
                .handle
                .read_bulk(in_ep, &mut buf[..want], remaining)
            {
                Ok(0) => {
                    return Err(Error::BlockLength {
                        expected: transfer_size,
                        actual: message.len(),
                    });
                }
                Ok(n) => {
                    let take = n.min(transfer_size - message.len());
                    message.extend_from_slice(&buf[..take]);
                }
                Err(rusb::Error::Pipe) => {
                    connection.handle.clear_halt(in_ep).ok();
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(rusb::Error::Timeout) => {}
                Err(e) => return Err(Error::Usb(e)),
            }
        }

        Ok(message)
    }

    fn query_bytes(&mut self, command: &str) -> Result<Vec<u8>> {
        self.send_command(command.as_bytes())?;
        self.read_bytes()
    }
}

impl Scpi for UsbInstrument {
    fn send(&mut self, command: &str) -> Result<()> {
        self.write_data(command.as_bytes())
    }

    fn query(&mut self, command: &str) -> Result<String> {
        let bytes = self.query_bytes(command)?;
        let mut bytes = bytes;
        while matches!(bytes.last(), Some(b'\n') | Some(b'\r')) {
            bytes.pop();
        }
        String::from_utf8(bytes).map_err(|e| Error::UsbMsg(format!("invalid UTF-8: {e}")))
    }

    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>> {
        self.query_bytes(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(inst: &mut UsbInstrument, msgid: u8) -> Vec<u8> {
        let btag = inst.next_btag();
        vec![msgid, btag, !btag, 0x00]
    }

    #[test]
    fn btag_increments_and_wraps() {
        let mut inst = UsbInstrument {
            last_btag: 0,
            max_transfer_size: 1024,
            timeout: Duration::from_secs(1),
            connection: None,
        };
        let hdr = headers(&mut inst, MSGID_DEV_DEP_MSG_OUT);
        assert_eq!(hdr, vec![MSGID_DEV_DEP_MSG_OUT, 1, !1u8, 0x00]);

        inst.last_btag = 254;
        assert_eq!(inst.next_btag(), 255);
        assert_eq!(inst.next_btag(), 1);
    }

    #[test]
    fn dev_dep_msg_out_header_encodes_size_and_eom() {
        let mut inst = UsbInstrument {
            last_btag: 0,
            max_transfer_size: 1024,
            timeout: Duration::from_secs(1),
            connection: None,
        };
        let hdr = inst.pack_dev_dep_msg_out_header(300, true);
        assert_eq!(hdr.len(), HEADER_BYTES);
        assert_eq!(&hdr[4..8], &300u32.to_le_bytes());
        assert_eq!(hdr[8], 0x01);
        assert_eq!(&hdr[9..12], &[0x00, 0x00, 0x00]);

        let hdr_no_eom = inst.pack_dev_dep_msg_out_header(1024, false);
        assert_eq!(hdr_no_eom[8], 0x00);
    }

    #[test]
    fn dev_dep_msg_in_header_encodes_size() {
        let mut inst = UsbInstrument {
            last_btag: 0,
            max_transfer_size: 1024,
            timeout: Duration::from_secs(1),
            connection: None,
        };
        let hdr = inst.pack_dev_dep_msg_in_header(1024);
        assert_eq!(hdr.len(), HEADER_BYTES);
        assert_eq!(&hdr[4..8], &1024u32.to_le_bytes());
        assert_eq!(hdr[8], 0x00);
        assert_eq!(hdr[9], 0x00);
    }
}
