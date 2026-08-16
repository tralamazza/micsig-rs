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
