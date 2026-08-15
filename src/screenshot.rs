//! Screen capture via the `:SYS:SCR?` command.

use crate::error::Result;
use crate::transport::Scpi;

/// Capture the current screen image bytes.
pub fn capture(inst: &mut impl Scpi) -> Result<Vec<u8>> {
    let raw = inst.query_raw(":SYS:SCR?")?;
    Ok(crate::scpi::unwrap_block(&raw))
}

/// Capture and write the screen image to a file. If `filename` is `None` or
/// `-`, the image bytes are written to stdout.
pub fn save(inst: &mut impl Scpi, filename: Option<&str>) -> Result<()> {
    let image = capture(inst)?;
    match filename {
        None | Some("-") => {
            use std::io::Write;
            std::io::stdout().write_all(&image).map_err(crate::error::Error::Io)?;
        }
        Some(path) => std::fs::write(path, &image).map_err(crate::error::Error::Io)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    /// Spin up a fake instrument that replies to `:SYS:SCR?` with a small PNG
    /// wrapped in an IEEE 488.2 block header.
    fn fake_scope(png: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 256];
                let n = stream.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.contains(":SYS:SCR?"));
                let header = format!("#{}{}", png.len().to_string().len(), png.len());
                let mut resp = Vec::new();
                resp.extend_from_slice(header.as_bytes());
                resp.extend_from_slice(png);
                resp.push(b'\n');
                stream.write_all(&resp).unwrap();
            }
        });
        format!("127.0.0.1:{port}")
    }

    #[test]
    fn capture_reads_png_block() {
        let png: &'static [u8] = b"\x89PNG\r\n\x1a\nfake";
        let addr = fake_scope(png);
        let (host, port) = addr.split_once(':').unwrap();
        let mut inst = crate::transport::Instrument::connect(
            host,
            port.parse().unwrap(),
            Duration::from_secs(2),
        )
        .unwrap();
        let got = capture(&mut inst).unwrap();
        assert_eq!(got, png);
    }
}
