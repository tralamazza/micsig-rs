//! Screen capture via the `:SYS:SCR?` command.

use crate::error::Result;
use crate::transport::Scpi;

/// Capture the current screen image bytes.
pub fn capture(inst: &mut impl Scpi) -> Result<Vec<u8>> {
    let raw = inst.query_raw(":SYS:SCR?")?;
    let mut image = crate::scpi::unwrap_block(&raw);
    repair_jfif_marker(&mut image);
    Ok(image)
}

/// Repair the APP0 marker in a Micsig screenshot.
///
/// An MHO14-200N (firmware 1.97.70) emits `FF D8 58 00` where JFIF requires
/// `FF D8 FF E0`; every other byte of the image is a well-formed JPEG, so the
/// two-byte fix yields a file that decoders accept. Without it `:SYS:SCR?`
/// output is rejected by every image viewer.
///
/// The patch is deliberately narrow: it only fires on the exact malformed
/// signature, followed by the `00 10 4A 46 49 46` ("JFIF" segment) that a
/// correct APP0 would carry.
pub fn repair_jfif_marker(image: &mut [u8]) {
    const BROKEN: &[u8] = &[0xFF, 0xD8, 0x58, 0x00, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
    if image.len() >= BROKEN.len() && image[..BROKEN.len()] == *BROKEN {
        image[2] = 0xFF;
        image[3] = 0xE0;
    }
}

/// Capture and write the screen image to a file. If `filename` is `None` or
/// `-`, the image bytes are written to stdout.
pub fn save(inst: &mut impl Scpi, filename: Option<&str>) -> Result<()> {
    let image = capture(inst)?;
    match filename {
        None | Some("-") => {
            use std::io::Write;
            std::io::stdout()
                .write_all(&image)
                .map_err(crate::error::Error::Io)?;
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
