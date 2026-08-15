//! Round-trip latency benchmark via repeated `*IDN?` requests.

use std::time::Instant;

use crate::error::Result;
use crate::transport::Scpi;

/// Send `count` `*IDN?` requests and return the throughput in requests/second.
pub fn run(inst: &mut impl Scpi, count: usize) -> Result<f64> {
    let start = Instant::now();
    for _ in 0..count {
        inst.idn()?;
    }
    let elapsed = start.elapsed();
    Ok(count as f64 / elapsed.as_secs_f64())
}

/// Benchmark with a progress indicator, mirroring lxi-tools' benchmark output.
pub fn run_with_progress(inst: &mut impl Scpi, count: usize) -> Result<f64> {
    eprintln!("Benchmarking by sending {count} ID requests. Please wait...");
    let start = Instant::now();
    for i in 0..count {
        inst.idn()?;
        eprint!("\r{}", i + 1);
    }
    let elapsed = start.elapsed();
    let result = count as f64 / elapsed.as_secs_f64();
    eprint!("\rResult: {result:.1} requests/second\n");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn benchmark_counts_round_trips() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 64];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                stream.write_all(b"Micsig,MDO5004,1,1.0\n").unwrap();
            }
        });
        let mut inst =
            crate::transport::Instrument::connect("127.0.0.1", addr.port(), Duration::from_secs(2))
                .unwrap();
        let rps = run(&mut inst, 10).unwrap();
        assert!(rps > 0.0);
    }
}
