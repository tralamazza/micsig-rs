use std::num::NonZeroUsize;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use micsig_rs::benchmark;
use micsig_rs::discover;
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
struct DiscoverArgs {
    /// Ports to probe on --address. Defaults to --port.
    #[arg(long, num_args = 1.., value_delimiter = ',')]
    ports: Option<Vec<u16>>,
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
    use std::io::{BufRead, Write};

    let mut inst = conn.open(3)?;
    match &conn.address {
        Some(address) => println!("Connected to {address}:{}", conn.port),
        None => println!("Connected over USB"),
    }
    println!("Entering interactive mode (ctrl-d to quit)\n");

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("micsig> ");
        std::io::stdout().flush().map_err(Error::Io)?;
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
    println!();
    Ok(())
}

/// Print a binary response to stdout, tolerating a closed pipe.
fn print_stdout(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    out.write_all(bytes).map_err(Error::Io)?;
    if !bytes.ends_with(b"\n") {
        out.write_all(b"\n").map_err(Error::Io)?;
    }
    out.flush().map_err(Error::Io)
}

/// Print a hex dump of the response, tolerating a closed pipe.
fn print_hex(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 16 == 0 {
            out.write_all(b"\n").map_err(Error::Io)?;
        }
        write!(out, "{b:02x} ").map_err(Error::Io)?;
    }
    out.write_all(b"\n").map_err(Error::Io)?;
    out.flush().map_err(Error::Io)
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
        println!("Saved screenshot image to {f}");
    }
    Ok(())
}

fn waveform_command(conn: &ConnectionArgs, args: WaveformArgs) -> Result<()> {
    let mut inst = conn.open(10)?;
    let wave = waveform::capture(&mut inst, args.channel, args.mode.into())?;
    let volts = waveform::samples_to_volts(&wave);
    match args.file.as_deref() {
        Some("-") | None => {
            let mut out = std::io::stdout().lock();
            write_waveform_csv(&mut out, &wave, &volts).map_err(Error::Io)?;
        }
        Some(path) => {
            let mut f = std::fs::File::create(path).map_err(Error::Io)?;
            write_waveform_csv(&mut f, &wave, &volts).map_err(Error::Io)?;
            println!("Saved waveform data to {path}");
        }
    }
    Ok(())
}

fn write_waveform_csv(
    out: &mut impl std::io::Write,
    wave: &waveform::Waveform,
    volts: &[f64],
) -> std::io::Result<()> {
    writeln!(out, "sample,time_s,voltage_v")?;
    for (i, v) in volts.iter().enumerate() {
        let t = wave.preamble.x_origin + i as f64 * wave.preamble.x_increment;
        writeln!(out, "{i},{t:.9e},{v:.9e}")?;
    }
    Ok(())
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
        println!("Scanning the USB bus for Micsig instruments...\n");
        let devices = micsig_rs::usb::list_instruments()?;
        if devices.is_empty() {
            println!("No USB instruments found");
            return Ok(());
        }
        for d in &devices {
            let name = d.product.as_deref().unwrap_or("Micsig instrument");
            print!(
                "  Found \"{name}\" at bus {:03} device {:03} ({:04x}:{:04x})",
                d.bus, d.address, d.vendor_id, d.product_id
            );
            match &d.serial {
                Some(s) => println!(" serial {s}"),
                None => println!(),
            }
            if !d.accessible {
                println!(
                    "    (cannot be opened: check permissions - on Linux this \
                     usually means a missing udev rule)"
                );
            }
        }
        println!("Found {} device{}", devices.len(), plural(devices.len()));
        return Ok(());
    };

    let ports = args.ports.clone().unwrap_or_else(|| vec![conn.port]);
    println!("Probing {address} for instruments - please wait...\n");
    let devices = discover::scan_ports(address, &ports, timeout);
    if devices.is_empty() {
        println!("No devices found");
    } else {
        for d in &devices {
            println!("  Found \"{}\" on address {}", d.id.trim(), d.address);
        }
        println!("Found {} device{}", devices.len(), plural(devices.len()));
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
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
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
    fn cli_verifies() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
