//! Regression tests for transport edge cases, driven by a scripted mock socket.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use micsig_rs::transport::{Instrument, Scpi};

/// Serve one connection: read a request, run `reply`, then act on the result.
fn serve(reply: impl FnOnce(&mut std::net::TcpStream) + Send + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 256];
            let _ = stream.read(&mut buf);
            reply(&mut stream);
        }
    });
    port
}

fn connect(port: u16, secs: u64) -> Instrument {
    Instrument::connect("127.0.0.1", port, Duration::from_secs(secs)).unwrap()
}

#[test]
fn eof_before_any_data_is_an_error_not_a_nul_byte() {
    // Instrument accepts the connection then hangs up without replying.
    let port = serve(|stream| {
        stream.shutdown(std::net::Shutdown::Both).ok();
    });
    let mut inst = connect(port, 2);
    let got = inst.query_raw("*IDN?");
    assert!(
        got.is_err(),
        "expected an error on EOF, got {:?}",
        got.map(|v| v.to_vec())
    );
}

#[test]
fn block_without_trailing_newline_returns_promptly() {
    // A definite-length block with no trailing newline, connection held open.
    let payload = b"\x89PNG\r\n\x1a\nfake";
    let port = serve(move |stream| {
        let hdr = format!("#{}{}", payload.len().to_string().len(), payload.len());
        stream.write_all(hdr.as_bytes()).unwrap();
        stream.write_all(payload).unwrap();
        stream.flush().unwrap();
        // Hold the connection open so a blocking read would stall.
        thread::sleep(Duration::from_secs(30));
    });
    let mut inst = connect(port, 5);
    let start = Instant::now();
    let got = inst.query_raw(":SYS:SCR?").unwrap();
    let elapsed = start.elapsed();
    // query_raw returns the wire message; the header is stripped by the caller.
    assert_eq!(micsig_rs::scpi::unwrap_block(&got), payload);
    assert!(
        elapsed < Duration::from_secs(1),
        "block read stalled for {elapsed:?} waiting for an optional newline"
    );
}

#[test]
fn discover_resolves_hostnames() {
    let port = serve(|stream| {
        stream.write_all(b"Micsig,MHO14-200N,1,1.0\n").unwrap();
    });
    let found = micsig_rs::discover::probe_host("localhost", port, Duration::from_secs(2));
    assert!(found.is_some(), "hostname 'localhost' failed to resolve");
}

/// `:WAVeform:DATA?` declares a sample count, not a byte count. Treating it as
/// bytes silently dropped three quarters of every trace.
#[test]
fn waveform_block_length_is_a_sample_count() {
    let samples = 1000usize;
    let mut msg = format!("#9{samples:09}").into_bytes();
    for i in 0..samples {
        msg.extend_from_slice(format!("{:04X}", i as u16).as_bytes());
    }
    msg.extend_from_slice(b"\r\n\r\n\0"); // terminators + USB alignment padding

    let decoded = micsig_rs::waveform::decode_data_block(&msg);
    assert_eq!(decoded.len(), samples, "expected all {samples} samples");
    assert_eq!(decoded[0], 0);
    assert_eq!(decoded[999], 999);
}

/// The MHO series corrupts the APP0 marker, and not to a stable value: `58 00`
/// in most captures, `D8 00` in at least one observed on hardware.
#[test]
fn screenshot_jfif_marker_is_repaired() {
    for bad in [0x58u8, 0xD8, 0x00, 0x7F] {
        let mut img = vec![
            0xFF, 0xD8, bad, 0x00, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
        ];
        micsig_rs::screenshot::repair_jfif_marker(&mut img);
        assert_eq!(
            &img[..4],
            &[0xFF, 0xD8, 0xFF, 0xE0],
            "byte {bad:#04x} not repaired"
        );
    }

    // An already-valid image must be left alone.
    let mut ok = vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
    ];
    let before = ok.clone();
    micsig_rs::screenshot::repair_jfif_marker(&mut ok);
    assert_eq!(ok, before);
}

/// Anything that is not a JFIF APP0 header must not be rewritten.
#[test]
fn screenshot_repair_leaves_other_data_alone() {
    // PNG, and a JPEG whose APP0 body does not say "JFIF".
    for mut data in [
        b"\x89PNG\r\n\x1a\nfake".to_vec(),
        vec![
            0xFF, 0xD8, 0x58, 0x00, 0x00, 0x10, b'E', b'x', b'i', b'f', 0x00, 0x01,
        ],
    ] {
        let before = data.clone();
        micsig_rs::screenshot::repair_jfif_marker(&mut data);
        assert_eq!(data, before);
    }
}

#[test]
fn truncated_block_payload_is_an_error() {
    // Declares 100 bytes but sends 10 then closes.
    let port = serve(|stream| {
        stream.write_all(b"#3100").unwrap();
        stream.write_all(b"0123456789").unwrap();
        stream.shutdown(std::net::Shutdown::Both).ok();
    });
    let mut inst = connect(port, 2);
    assert!(inst.query_raw(":SYS:SCR?").is_err());
}

/// `:WAVeform:FORMat ASCii` returns comma-separated volts in scientific
/// notation, which the sample decoder would happily reinterpret as
/// little-endian i16 and turn into nonsense.
#[test]
fn ascii_volts_payload_is_recognised() {
    let ascii = b"1.148325e-02,1.658691e-02,1.531099e-02";
    assert!(micsig_rs::waveform::looks_like_ascii_volts(ascii));

    // Real hex sample payloads must not be mistaken for it.
    for hex in [
        &b"FFFF000300000001"[..],
        &b"0002, FFFF 0000"[..],
        &b"#9000062500"[..],
    ] {
        assert!(
            !micsig_rs::waveform::looks_like_ascii_volts(hex),
            "hex payload {:?} misread as ASCii volts",
            String::from_utf8_lossy(hex)
        );
    }
    assert!(!micsig_rs::waveform::looks_like_ascii_volts(b""));
}

/// Build a block of `n` ASCII-hex samples with distinct, in-range values, so
/// pages can be told apart when concatenated.
fn hex_block(n: usize) -> Vec<u8> {
    let mut block = format!("#9{n:09}").into_bytes();
    for i in 0..n {
        block.extend_from_slice(format!("{:04X}", (i % 0x7FFF) as u16).as_bytes());
    }
    block.push(b'\n');
    block
}

/// A scripted stand-in for an instrument, so `capture` can be exercised
/// without hardware. `:WAVeform:DATA?` pops one page per call.
struct FakeScope {
    pages: Vec<Vec<u8>>,
    page: usize,
    log: Vec<String>,
}

impl FakeScope {
    /// A scope holding `counts` pages, followed by the empty block a real
    /// MHO14-200N sends to mark the end of the record.
    fn with_pages(counts: &[usize]) -> Self {
        let mut pages: Vec<Vec<u8>> = counts.iter().map(|&n| hex_block(n)).collect();
        pages.push(b"#9000000000\n".to_vec());
        FakeScope {
            pages,
            page: 0,
            log: Vec::new(),
        }
    }
}

impl Scpi for FakeScope {
    fn send(&mut self, command: &str) -> Result<(), micsig_rs::error::Error> {
        self.log.push(command.to_string());
        // Writing the mode rewinds the read cursor, as the firmware does.
        if command.starts_with(":WAVeform:MODE") {
            self.page = 0;
        }
        Ok(())
    }

    fn query(&mut self, command: &str) -> Result<String, micsig_rs::error::Error> {
        self.log.push(command.to_string());
        Ok(match command {
            c if c.starts_with(":CHANnel") => "1".to_string(),
            ":WAVeform:PREamble?" => "0,0,1,1.0E-9,0.0,0.0,1.0,0.0,0.0".to_string(),
            _ => "0".to_string(),
        })
    }

    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>, micsig_rs::error::Error> {
        self.log.push(command.to_string());
        let out = self.pages.get(self.page).cloned().unwrap_or_default();
        self.page += 1;
        Ok(out)
    }
}

/// `:WAVeform:DATA?` caps each response at 62500 samples and continues from
/// there on the next call, so a single read returns a fraction of the record.
/// Measured on an MHO14-200N (firmware 1.143.72): a 110000-sample `NORMal`
/// record arrives as 62500 + 47500 + an empty block.
#[test]
fn waveform_capture_drains_every_page() {
    let mut scope = FakeScope::with_pages(&[62500, 47500]);
    let wave = micsig_rs::waveform::capture(&mut scope, 1, micsig_rs::waveform::Mode::Normal)
        .expect("capture should succeed");
    assert_eq!(
        wave.samples.len(),
        110000,
        "capture stopped at the first page"
    );

    // The second page must follow the first, not overwrite it.
    assert_eq!(wave.samples[0], 0x0000);
    assert_eq!(wave.samples[62499], ((62499 % 0x7FFF) as u16) as i16);
    assert_eq!(wave.samples[62500], 0x0000);

    let reads = scope.log.iter().filter(|c| *c == ":WAVeform:DATA?").count();
    assert_eq!(reads, 3, "should read until the empty terminating block");
}

/// The mode write rewinds the page cursor, so it has to be the last thing sent
/// before the first read; ordering it earlier loses the front of the record.
#[test]
fn waveform_capture_sets_mode_after_the_other_setup() {
    let mut scope = FakeScope::with_pages(&[10]);
    micsig_rs::waveform::capture(&mut scope, 1, micsig_rs::waveform::Mode::Raw).unwrap();

    let mode = scope
        .log
        .iter()
        .position(|c| c.starts_with(":WAVeform:MODE"))
        .unwrap();
    let first_read = scope
        .log
        .iter()
        .position(|c| c == ":WAVeform:DATA?")
        .unwrap();
    for other in [
        ":WAVeform:SOURce",
        ":WAVeform:FORMat",
        ":WAVeform:PREamble?",
    ] {
        let at = scope.log.iter().position(|c| c.starts_with(other)).unwrap();
        assert!(at < mode, "{other} must be sent before :WAVeform:MODE");
    }
    assert!(mode < first_read);
}

/// A record that never terminates must be reported, not quietly truncated.
#[test]
fn waveform_capture_refuses_an_endless_record() {
    let mut scope = FakeScope {
        pages: Vec::new(),
        page: 0,
        log: Vec::new(),
    };
    scope.pages = (0..500)
        .map(|_| {
            let mut b = format!("#9{:09}", 4).into_bytes();
            b.extend_from_slice(b"0001000200030004\n");
            b
        })
        .collect();
    let err = micsig_rs::waveform::capture(&mut scope, 1, micsig_rs::waveform::Mode::Normal)
        .expect_err("should refuse a record with no terminating empty block");
    assert!(
        err.to_string().contains("truncated"),
        "unexpected error: {err}"
    );
}

/// A scope that answers `:SYS:SCR?` with `empties` empty blocks before the
/// real image, mimicking one that is still busy with a previous capture.
struct BusyScreen {
    empties: usize,
    calls: usize,
}

impl Scpi for BusyScreen {
    fn send(&mut self, _: &str) -> Result<(), micsig_rs::error::Error> {
        Ok(())
    }
    fn query(&mut self, _: &str) -> Result<String, micsig_rs::error::Error> {
        Ok(String::new())
    }
    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>, micsig_rs::error::Error> {
        assert_eq!(command, ":SYS:SCR?");
        self.calls += 1;
        if self.calls <= self.empties {
            return Ok(b"#9000000000\n".to_vec());
        }
        // Minimal JPEG with the firmware's corrupt APP0 marker.
        let img: &[u8] = &[
            0xFF, 0xD8, 0x58, 0x00, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
        ];
        let mut out = format!("#9{:09}", img.len()).into_bytes();
        out.extend_from_slice(img);
        Ok(out)
    }
}

/// Back-to-back `:SYS:SCR?` returns an empty block for a while, so a single
/// failed attempt is not a real failure. Capture retries, and still repairs
/// the marker on whichever attempt succeeds.
#[test]
fn screenshot_retries_while_the_instrument_is_busy() {
    let mut scope = BusyScreen {
        empties: 3,
        calls: 0,
    };
    let img = micsig_rs::screenshot::capture_with(&mut scope, 5, Duration::ZERO)
        .expect("should have retried past the empty blocks");
    assert_eq!(scope.calls, 4, "should stop retrying once an image arrives");
    assert_eq!(&img[..4], &[0xFF, 0xD8, 0xFF, 0xE0], "marker not repaired");
}

/// Retrying must still give up rather than loop, and must never hand back the
/// zero-byte image that would look like a successful capture.
#[test]
fn screenshot_gives_up_after_the_attempt_limit() {
    let mut scope = BusyScreen {
        empties: 100,
        calls: 0,
    };
    let err = micsig_rs::screenshot::capture_with(&mut scope, 3, Duration::ZERO)
        .expect_err("an always-busy instrument must be an error");
    assert_eq!(scope.calls, 3, "attempt limit not honoured");
    assert!(err.to_string().contains("empty screenshot"), "got: {err}");
}
