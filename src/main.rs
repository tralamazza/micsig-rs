use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use micsig_rs::benchmark;
use micsig_rs::decode;
use micsig_rs::discover;
use micsig_rs::measure;
use micsig_rs::screenshot;
use micsig_rs::transport::{DEFAULT_RAW_PORT, Instrument, Scpi};
use micsig_rs::usb::UsbInstrument;
use micsig_rs::waveform;
use micsig_rs::{Error, Result};

/// Interface with a Micsig oscilloscope over SCPI.
#[derive(Parser)]
#[command(name = "micsig", version, about)]
#[command(after_help = "\
With neither --usb nor --address, the USB bus is searched for a Micsig \
instrument. Use --address for a LAN/WiFi scope.")]
struct Cli {
    #[command(flatten)]
    conn: ConnectionArgs,

    #[command(subcommand)]
    command: Command,
}

/// Connection options, shared by every subcommand.
#[derive(Args, Clone)]
struct ConnectionArgs {
    /// Connect over USB (USBTMC). The default when no --address is given.
    #[arg(short = 'u', long, global = true, conflicts_with = "address")]
    usb: bool,

    /// Instrument IP address or hostname; selects the TCP transport.
    #[arg(short, long, global = true)]
    address: Option<String>,

    /// TCP port to use with --address.
    #[arg(short, long, global = true, default_value_t = DEFAULT_RAW_PORT)]
    port: u16,

    /// Response timeout, e.g. `3`, `0.5`, `500ms`. Per-command default if unset.
    #[arg(short, long, global = true, value_parser = parse_timeout)]
    timeout: Option<Duration>,
}

/// Parse a timeout as bare seconds (`3`, `0.5`) or with a unit (`500ms`, `2s`).
fn parse_timeout(s: &str) -> std::result::Result<Duration, String> {
    let s = s.trim();
    let (value, scale) = match s.strip_suffix("ms") {
        Some(v) => (v, 1e-3),
        None => (s.strip_suffix('s').unwrap_or(s), 1.0),
    };
    let secs: f64 = value
        .trim()
        .parse()
        .map_err(|_| format!("invalid timeout '{s}'"))?;
    if !secs.is_finite() || secs <= 0.0 {
        return Err(format!("timeout must be positive, got '{s}'"));
    }
    Ok(Duration::from_secs_f64(secs * scale))
}

impl ConnectionArgs {
    /// Open a connection, falling back to USB when no address was given.
    fn open(&self, default_timeout: u64) -> Result<AnyTransport> {
        let timeout = self
            .timeout
            .unwrap_or_else(|| Duration::from_secs(default_timeout));
        match &self.address {
            // clap guarantees --address and --usb are mutually exclusive.
            Some(address) => Ok(AnyTransport::Tcp(Instrument::connect(
                address, self.port, timeout,
            )?)),
            None => UsbInstrument::connect(timeout)
                .map(AnyTransport::Usb)
                .map_err(|e| {
                    if self.usb {
                        e
                    } else {
                        // Auto-detect: say what was tried, not just "not found".
                        // Message, not UsbMsg: `e` already carries its own prefix.
                        Error::Message(format!("{e}. Pass --address <host> for a LAN/WiFi scope"))
                    }
                }),
        }
    }

    /// A short label for the connection, used in generated filenames.
    fn label(&self) -> String {
        match &self.address {
            Some(address) => address.clone(),
            None => "usb".to_string(),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Send a SCPI command and print the response.
    Scpi(ScpiArgs),
    /// Capture the current screen image.
    Screenshot(ScreenshotArgs),
    /// Capture and export waveform data.
    Waveform(WaveformArgs),
    /// Benchmark request latency by sending repeated `*IDN?` requests.
    Benchmark(BenchmarkArgs),
    /// List instruments: USB devices, or a TCP host given --address.
    Discover(DiscoverArgs),
    /// Read the instrument's automatic measurements for a channel.
    Measure(MeasureArgs),
    /// Read decoded serial bus frames as CSV.
    Decode(DecodeArgs),
}

#[derive(Args)]
struct ScpiArgs {
    /// Print the response as hexadecimal.
    #[arg(short = 'x', long)]
    hex: bool,

    /// Enter interactive mode (repl).
    #[arg(short, long, conflicts_with = "command")]
    interactive: bool,

    /// The SCPI command to send (e.g. `*IDN?`).
    #[arg(required_unless_present = "interactive")]
    command: Option<String>,
}

#[derive(Args)]
struct ScreenshotArgs {
    /// Output filename, or `-` for stdout.
    /// Defaults to `screenshot_<target>_<timestamp>.jpg`.
    #[arg(short, long)]
    file: Option<String>,
}

#[derive(Args)]
struct WaveformArgs {
    /// Channel to read.
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=4))]
    channel: u8,

    /// Read mode. `raw` reads full memory depth and needs a stopped scope.
    #[arg(short = 'm', long, value_enum, default_value_t = ModeArg::Normal)]
    mode: ModeArg,

    /// Output CSV filename, or `-` for stdout (the default).
    #[arg(short, long)]
    file: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
enum ModeArg {
    Normal,
    Maximum,
    Raw,
}

impl From<ModeArg> for waveform::Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Normal => waveform::Mode::Normal,
            ModeArg::Maximum => waveform::Mode::Maximum,
            ModeArg::Raw => waveform::Mode::Raw,
        }
    }
}

#[derive(Args)]
struct BenchmarkArgs {
    /// Number of requests to send.
    #[arg(short, long, default_value = "100")]
    count: NonZeroUsize,
}

#[derive(Args)]
struct MeasureArgs {
    /// Channel to measure.
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=4))]
    channel: u8,

    /// Measurements to read, comma-separated. Defaults to a general-purpose set.
    #[arg(short, long, value_enum, num_args = 1.., value_delimiter = ',')]
    items: Option<Vec<measure::Item>>,

    /// Read every supported measurement.
    #[arg(long, conflicts_with = "items")]
    all: bool,

    /// Emit `item,value,unit` CSV instead of an aligned table.
    #[arg(long)]
    csv: bool,
}

#[derive(Args)]
struct DecodeArgs {
    /// Bus to read.
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=2))]
    bus: u8,

    /// Output CSV filename, or `-` for stdout (the default).
    #[arg(short, long)]
    file: Option<String>,
}

#[derive(Args)]
struct DiscoverArgs {
    /// Ports to probe on --address. Defaults to --port.
    #[arg(long, num_args = 1.., value_delimiter = ',')]
    ports: Option<Vec<u16>>,
}

/// Write to stdout, exiting quietly if the reader has closed the pipe.
///
/// Rust ignores SIGPIPE at startup, so `println!` *panics* on EPIPE — piping
/// any of this tool's output into `head` produced a Rust backtrace. Restoring
/// the default SIGPIPE disposition would fix that, but it would also turn a
/// write to a half-closed TCP socket into a fatal signal instead of a clean
/// error, so the pipe is handled here instead.
fn out_write(args: std::fmt::Arguments<'_>, newline: bool) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let wrote = if newline {
        writeln!(out, "{args}")
    } else {
        write!(out, "{args}")
    };
    if wrote.is_err() || out.flush().is_err() {
        std::process::exit(0);
    }
}

macro_rules! outln {
    () => { out_write(format_args!(""), true) };
    ($($arg:tt)*) => { out_write(format_args!($($arg)*), true) };
}

macro_rules! out {
    ($($arg:tt)*) => { out_write(format_args!($($arg)*), false) };
}

/// A transport that can be either TCP or USB.
enum AnyTransport {
    Tcp(Instrument),
    Usb(UsbInstrument),
}

impl Scpi for AnyTransport {
    fn send(&mut self, command: &str) -> Result<()> {
        match self {
            AnyTransport::Tcp(t) => t.send(command),
            AnyTransport::Usb(u) => u.send(command),
        }
    }

    fn query(&mut self, command: &str) -> Result<String> {
        match self {
            AnyTransport::Tcp(t) => t.query(command),
            AnyTransport::Usb(u) => u.query(command),
        }
    }

    fn query_raw(&mut self, command: &str) -> Result<Vec<u8>> {
        match self {
            AnyTransport::Tcp(t) => t.query_raw(command),
            AnyTransport::Usb(u) => u.query_raw(command),
        }
    }
}

fn scpi_command(conn: &ConnectionArgs, args: ScpiArgs) -> Result<()> {
    if args.interactive {
        return interactive(conn, args.hex);
    }
    let command = args.command.as_deref().unwrap_or_default();
    let mut inst = conn.open(3)?;
    if micsig_rs::scpi::is_query(command) {
        let resp = inst.query_raw(command)?;
        if args.hex {
            print_hex(&resp)?;
        } else {
            print_stdout(&resp)?;
        }
    } else {
        inst.send(command)?;
    }
    Ok(())
}

fn interactive(conn: &ConnectionArgs, hex: bool) -> Result<()> {
    use std::io::BufRead;

    let mut inst = conn.open(3)?;
    match &conn.address {
        Some(address) => outln!("Connected to {address}:{}", conn.port),
        None => outln!("Connected over USB"),
    }
    outln!("Entering interactive mode (ctrl-d to quit)\n");

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        out!("micsig> "); // flushes internally
        let Some(line) = lines.next() else { break };
        let Ok(line) = line else { break };
        let command = line.trim();
        if command.is_empty() {
            continue;
        }
        if command == "quit" || command == "exit" {
            break;
        }
        if micsig_rs::scpi::is_query(command) {
            match inst.query_raw(command) {
                Ok(resp) => {
                    let r = if hex {
                        print_hex(&resp)
                    } else {
                        print_stdout(&resp)
                    };
                    if let Err(e) = r {
                        eprintln!("{e}");
                    }
                }
                Err(e) => eprintln!("{e}"),
            }
        } else if let Err(e) = inst.send(command) {
            eprintln!("{e}");
        }
    }
    outln!();
    Ok(())
}

/// True if a response is unambiguously text: valid UTF-8 with no control
/// characters beyond the usual whitespace.
///
/// Binary payloads fail this on the first stray byte — a JPEG's `FF` is not
/// legal UTF-8, and its `00` bytes are control characters.
fn is_text_response(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|s| {
        !s.chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
    })
}

/// Render a response for stdout.
///
/// Text is stripped of its SCPI terminators and given exactly one newline, so
/// `$(micsig scpi ":CHANnel1:SCALe?")` yields `0.49` rather than `0.49\r`.
/// Anything else is passed through byte for byte — `micsig scpi ":SYS:SCR?"`
/// must still redirect to a usable file, which also means no newline is
/// appended to binary the way it once was.
fn render_response(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !is_text_response(bytes) {
        return Cow::Borrowed(bytes);
    }
    let end = bytes
        .iter()
        .rposition(|b| !matches!(b, b'\r' | b'\n'))
        .map_or(0, |i| i + 1);
    let mut out = Vec::with_capacity(end + 1);
    out.extend_from_slice(&bytes[..end]);
    out.push(b'\n');
    Cow::Owned(out)
}

/// Map a stdout write failure, exiting quietly when the reader has gone away
/// so that `| head` behaves like it does for any other Unix filter.
fn stdout_err(e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
    Error::Io(e)
}

/// Print a response to stdout.
fn print_stdout(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    out.write_all(&render_response(bytes)).map_err(stdout_err)?;
    out.flush().map_err(stdout_err)
}

/// Print a hex dump of the response, tolerating a closed pipe.
fn print_hex(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 16 == 0 {
            out.write_all(b"\n").map_err(stdout_err)?;
        }
        write!(out, "{b:02x} ").map_err(stdout_err)?;
    }
    out.write_all(b"\n").map_err(stdout_err)?;
    out.flush().map_err(stdout_err)
}

fn screenshot_command(conn: &ConnectionArgs, args: ScreenshotArgs) -> Result<()> {
    let mut inst = conn.open(10)?;
    let filename = match args.file.as_deref() {
        Some("-") => None,
        Some(f) => Some(f.to_string()),
        None => Some(default_filename(&conn.label(), "jpg")),
    };
    screenshot::save(&mut inst, filename.as_deref())?;
    if let Some(f) = &filename {
        outln!("Saved screenshot image to {f}");
    }
    Ok(())
}

fn waveform_command(conn: &ConnectionArgs, args: WaveformArgs) -> Result<()> {
    let mut inst = conn.open(10)?;
    let wave = waveform::capture(&mut inst, args.channel, args.mode.into())?;
    match args.file.as_deref() {
        Some("-") | None => {
            let mut out = std::io::BufWriter::new(std::io::stdout().lock());
            write_waveform_csv(&mut out, &wave).map_err(Error::Io)?;
        }
        Some(path) => {
            let f = std::fs::File::create(path).map_err(Error::Io)?;
            let mut out = std::io::BufWriter::new(f);
            write_waveform_csv(&mut out, &wave).map_err(Error::Io)?;
            outln!("Saved waveform data to {path}");
        }
    }
    Ok(())
}

/// Write the trace as CSV.
///
/// Two things about a full-depth `--mode raw` capture, which is 11 million
/// rows, shape this. The caller must hand over a buffered writer, because one
/// `writeln!` per row straight at a `File` or a line-buffered stdout is one
/// write syscall per row, and that dominated the runtime of every export. And
/// volts are scaled a row at a time rather than up front: collecting them
/// first costs an 88 MB `Vec<f64>` that is read once and dropped.
fn write_waveform_csv(
    out: &mut impl std::io::Write,
    wave: &waveform::Waveform,
) -> std::io::Result<()> {
    writeln!(out, "sample,time_s,voltage_v")?;
    for (i, &sample) in wave.samples.iter().enumerate() {
        let t = wave.preamble.x_origin + i as f64 * wave.preamble.x_increment;
        let v = waveform::sample_to_volts(&wave.preamble, sample);
        writeln!(out, "{i},{t:.9e},{v:.9e}")?;
    }
    // BufWriter swallows errors on drop, so surface them here instead.
    out.flush()
}

fn benchmark_command(conn: &ConnectionArgs, args: BenchmarkArgs) -> Result<()> {
    let mut inst = conn.open(3)?;
    benchmark::run_with_progress(&mut inst, args.count.get())?;
    Ok(())
}

/// List instruments: USB devices by default, or a TCP host given --address.
fn discover_command(conn: &ConnectionArgs, args: DiscoverArgs) -> Result<()> {
    let timeout = conn.timeout.unwrap_or(Duration::from_secs(1));

    let Some(address) = &conn.address else {
        outln!("Scanning the USB bus for Micsig instruments...\n");
        let devices = micsig_rs::usb::list_instruments()?;
        if devices.is_empty() {
            outln!("No USB instruments found");
            return Ok(());
        }
        for d in &devices {
            let name = d.product.as_deref().unwrap_or("Micsig instrument");
            out!(
                "  Found \"{name}\" at bus {:03} device {:03} ({:04x}:{:04x})",
                d.bus,
                d.address,
                d.vendor_id,
                d.product_id
            );
            match &d.serial {
                Some(s) => outln!(" serial {s}"),
                None => outln!(),
            }
            if !d.accessible {
                outln!(
                    "    (cannot be opened: check permissions - on Linux this \
                     usually means a missing udev rule)"
                );
            }
        }
        outln!("Found {} device{}", devices.len(), plural(devices.len()));
        return Ok(());
    };

    let ports = args.ports.clone().unwrap_or_else(|| vec![conn.port]);
    outln!("Probing {address} for instruments - please wait...\n");
    let devices = discover::scan_ports(address, &ports, timeout);
    if devices.is_empty() {
        outln!("No devices found");
    } else {
        for d in &devices {
            outln!("  Found \"{}\" on address {}", d.id.trim(), d.address);
        }
        outln!("Found {} device{}", devices.len(), plural(devices.len()));
    }
    Ok(())
}

fn measure_command(conn: &ConnectionArgs, args: MeasureArgs) -> Result<()> {
    let items: Vec<measure::Item> = if args.all {
        measure::Item::all().to_vec()
    } else {
        args.items
            .unwrap_or_else(|| measure::Item::defaults().to_vec())
    };

    let mut inst = conn.open(10)?;
    let results = measure::read(&mut inst, args.channel, &items)?;

    if args.csv {
        outln!("item,value,unit");
        for m in &results {
            match m.value {
                Some(v) => outln!("{},{v:e},{}", m.item.keyword(), m.item.unit()),
                None => outln!("{},,{}", m.item.keyword(), m.item.unit()),
            }
        }
        return Ok(());
    }

    let width = results
        .iter()
        .map(|m| m.item.keyword().len())
        .max()
        .unwrap_or(0);
    for m in &results {
        outln!(
            "  {:<width$}  {}",
            m.item.keyword(),
            measure::format_value(m.item, m.value)
        );
    }
    if results.iter().all(|m| m.value.is_none()) {
        outln!(
            "\nNo measurement returned a value. The instrument reports `--` when \
             it cannot compute one from the current trace - check the signal is \
             triggered and fills a reasonable part of the screen."
        );
    }
    Ok(())
}

fn decode_command(conn: &ConnectionArgs, args: DecodeArgs) -> Result<()> {
    let mut inst = conn.open(10)?;
    let raw = decode::read(&mut inst, args.bus)?;
    let csv = decode::to_csv(&raw);
    if decode::frame_count(&raw) == 0 {
        return Err(Error::Message(format!(
            "bus {} decoded no frames; check it is configured and the signal \
             is present (see `:BUS{}:TYPE?`)",
            args.bus, args.bus
        )));
    }
    match args.file.as_deref() {
        Some("-") | None => print_stdout(csv.as_bytes())?,
        Some(path) => {
            std::fs::write(path, &csv).map_err(Error::Io)?;
            outln!(
                "Saved {} decoded frames to {path}",
                decode::frame_count(&raw)
            );
        }
    }
    Ok(())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn default_filename(address: &str, ext: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut name = format!("screenshot_{address}_{now}.{ext}");
    name.retain(|c| c != ':' && c != '/' && c != ' ');
    name
}

fn main() {
    let cli = Cli::parse();
    let conn = cli.conn;
    let result = match cli.command {
        Command::Scpi(args) => scpi_command(&conn, args),
        Command::Screenshot(args) => screenshot_command(&conn, args),
        Command::Waveform(args) => waveform_command(&conn, args),
        Command::Benchmark(args) => benchmark_command(&conn, args),
        Command::Discover(args) => discover_command(&conn, args),
        Command::Measure(args) => measure_command(&conn, args),
        Command::Decode(args) => decode_command(&conn, args),
    };
    if let Err(e) = result {
        // `micsig waveform | head` closes stdout early. Rust ignores SIGPIPE,
        // so the write surfaces as EPIPE; reporting it as a failure would be
        // noise for what is ordinary shell usage.
        if is_broken_pipe(&e) {
            std::process::exit(0);
        }
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// True if the error is a downstream reader closing the pipe.
fn is_broken_pipe(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == std::io::ErrorKind::BrokenPipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timeout_units() {
        assert_eq!(parse_timeout("3").unwrap(), Duration::from_secs(3));
        assert_eq!(parse_timeout("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_timeout("0.5").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_timeout("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(
            parse_timeout(" 1.5s ").unwrap(),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn rejects_bad_timeouts() {
        for bad in ["abc", "", "0", "-5", "nan", "inf", "5x"] {
            assert!(parse_timeout(bad).is_err(), "'{bad}' should be rejected");
        }
    }

    #[test]
    fn text_responses_lose_their_scpi_terminators() {
        // A shell capture of `micsig scpi ":CHANnel1:SCALe?"` should be `0.49`.
        assert_eq!(&*render_response(b"0.49\r\n"), b"0.49\n");
        assert_eq!(&*render_response(b"0.49"), b"0.49\n");
        assert_eq!(
            &*render_response(b"Micsig,MHO14-200N,1,1.0\r\n"),
            b"Micsig,MHO14-200N,1,1.0\n"
        );
        // Only trailing terminators go; internal newlines are content.
        assert_eq!(&*render_response(b"a\nb\r\n"), b"a\nb\n");
        // Degenerate cases still produce exactly one newline.
        assert_eq!(&*render_response(b"\r\n"), b"\n");
        assert_eq!(&*render_response(b""), b"\n");
    }

    #[test]
    fn binary_responses_pass_through_byte_for_byte() {
        // A JPEG must survive `micsig scpi ":SYS:SCR?" > shot.jpg` unchanged,
        // including the absence of an appended newline.
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0xD9];
        assert_eq!(&*render_response(&jpeg), &jpeg[..]);

        // Trailing 0x0A in binary must not be mistaken for a terminator.
        let binary = [0x00u8, 0x01, 0x02, 0x0A];
        assert_eq!(&*render_response(&binary), &binary[..]);

        assert!(!is_text_response(&jpeg));
        assert!(is_text_response(b"0.49\r\n"));
        assert!(is_text_response(b""));
    }

    #[test]
    fn cli_verifies() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
