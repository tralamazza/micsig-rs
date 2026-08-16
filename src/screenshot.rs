//! Screen capture via the `:SYS:SCR?` command.

use crate::error::Result;
use crate::transport::Scpi;

/// Capture the current screen image bytes.
///
/// Errors if the instrument returns an empty block, which it does when
/// `:SYS:SCR?` is issued again before the previous capture has completed;
/// writing the zero-byte result to a file would look like success.
pub fn capture(inst: &mut impl Scpi) -> Result<Vec<u8>> {
    let raw = inst.query_raw(":SYS:SCR?")?;
    let mut image = crate::scpi::unwrap_block(&raw);
    if image.is_empty() {
        return Err(crate::error::Error::Message(
            "instrument returned an empty screenshot; it may still be busy \
             with a previous capture"
                .into(),
        ));
    }
    repair_jfif_marker(&mut image);
    Ok(image)
}

/// Repair the APP0 marker in a Micsig screenshot.
///
/// An MHO14-200N (firmware 1.97.70) corrupts the two bytes where JFIF requires
/// the `FF E0` APP0 marker; every other byte is a well-formed JPEG, so
/// rewriting them yields a file decoders accept. Without this, `:SYS:SCR?`
/// output is rejected by every image viewer.
///
/// The corrupt bytes are not stable — `58 00` in most captures, `D8 00` in at
/// least one — so this anchors on the parts that *are* reliable: the SOI
/// marker and the `00 10 "JFIF"` APP0 body that follows. Matching the exact
/// bad signature instead would silently miss the variants.
pub fn repair_jfif_marker(image: &mut [u8]) {
    const SOI: &[u8] = &[0xFF, 0xD8];
    const APP0_BODY: &[u8] = &[0x00, 0x10, 0x4A, 0x46, 0x49, 0x46]; // len 16, "JFIF"
    const APP0_MARKER: &[u8] = &[0xFF, 0xE0];

    if image.len() >= 10
        && image[0..2] == *SOI
        && image[4..10] == *APP0_BODY
        && image[2..4] != *APP0_MARKER
    {
        image[2..4].copy_from_slice(APP0_MARKER);
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
