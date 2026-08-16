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
struct Cli {
    #[command(subcommand)]
    command: Command,
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
    /// Probe a host for a responding instrument.
    Discover(DiscoverArgs),
}

#[derive(Args)]
struct ScpiArgs {
    /// Connect over USB instead of TCP.
    #[arg(short = 'u', long)]
    usb: bool,

    /// Instrument IP address or hostname.
    #[arg(short, long, default_value = "127.0.0.1")]
    address: String,

    /// Raw SCPI-raw TCP port.
    #[arg(short, long, default_value_t = DEFAULT_RAW_PORT)]
    port: u16,

    /// Response timeout in seconds.
    #[arg(short, long, default_value_t = 3)]
    timeout: u64,

    /// Print the response as hexadecimal.
    #[arg(short = 'x', long)]
    hex: bool,

    /// Enter interactive mode (repl).
    #[arg(short, long)]
    interactive: bool,

    /// The SCPI command to send (e.g. `*IDN?`).
    #[arg(required_unless_present = "interactive")]
    command: Option<String>,
}

#[derive(Args)]
struct ScreenshotArgs {
    /// Connect over USB instead of TCP.
    #[arg(short = 'u', long)]
    usb: bool,

    /// Instrument IP address or hostname.
    #[arg(short, long, default_value = "127.0.0.1")]
    address: String,

    /// Raw SCPI-raw TCP port.
    #[arg(short, long, default_value_t = DEFAULT_RAW_PORT)]
    port: u16,

    /// Response timeout in seconds.
    #[arg(short, long, default_value_t = 10)]
    timeout: u64,

    /// Output filename, or `-` for stdout. Defaults to `screenshot_<addr>_<timestamp>.jpg`.
    #[arg(short, long)]
    file: Option<String>,
}

#[derive(Args)]
struct WaveformArgs {
    /// Connect over USB instead of TCP.
    #[arg(short = 'u', long)]
    usb: bool,

    /// Instrument IP address or hostname.
    #[arg(short, long, default_value = "127.0.0.1")]
    address: String,

    /// Raw SCPI-raw TCP port.
    #[arg(short, long, default_value_t = DEFAULT_RAW_PORT)]
    port: u16,

    /// Response timeout in seconds.
    #[arg(short, long, default_value_t = 10)]
    timeout: u64,

    /// Channel to read (1-4).
    #[arg(short, long, default_value_t = 1)]
    channel: u8,

    /// Read mode. `raw` reads full memory depth and needs a stopped scope.
    #[arg(short = 'm', long, value_enum, default_value_t = ModeArg::Normal)]
    mode: ModeArg,

    /// Output CSV filename, or `-` for stdout.
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
    /// Connect over USB instead of TCP.
    #[arg(short = 'u', long)]
    usb: bool,

    /// Instrument IP address or hostname.
    #[arg(short, long, default_value = "127.0.0.1")]
    address: String,

    /// Raw SCPI-raw TCP port.
    #[arg(short, long, default_value_t = DEFAULT_RAW_PORT)]
    port: u16,

    /// Response timeout in seconds.
    #[arg(short, long, default_value_t = 3)]
    timeout: u64,

    /// Number of requests to send.
    #[arg(short, long, default_value_t = 100)]
    count: usize,
}

#[derive(Args)]
struct DiscoverArgs {
    /// Host to probe.
    #[arg(short, long, default_value = "127.0.0.1")]
    address: String,

    /// Ports to probe.
    #[arg(short, long, default_values_t = vec![DEFAULT_RAW_PORT])]
    ports: Vec<u16>,

    /// Probe timeout in seconds.
    #[arg(short, long, default_value_t = 1)]
    timeout: u64,
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

fn connect(usb: bool, address: &str, port: u16, timeout: u64) -> Result<AnyTransport> {
    let timeout = Duration::from_secs(timeout);
    if usb {
        Ok(AnyTransport::Usb(UsbInstrument::connect(timeout)?))
    } else {
        Ok(AnyTransport::Tcp(Instrument::connect(
            address, port, timeout,
        )?))
    }
}

fn scpi_command(args: ScpiArgs) -> Result<()> {
    if args.interactive {
        return interactive(args.usb, &args.address, args.port, args.timeout, args.hex);
    }
    let command = args.command.as_deref().unwrap_or_default();
    let mut inst = connect(args.usb, &args.address, args.port, args.timeout)?;
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

fn interactive(usb: bool, address: &str, port: u16, timeout: u64, hex: bool) -> Result<()> {
    use std::io::{BufRead, Write};

    let mut inst = connect(usb, address, port, timeout)?;
    if usb {
        println!("Connected over USB");
    } else {
        println!("Connected to {address}:{port}");
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

fn screenshot_command(args: ScreenshotArgs) -> Result<()> {
    let mut inst = connect(args.usb, &args.address, args.port, args.timeout)?;
    let filename = match args.file.as_deref() {
        Some("-") => None,
        Some(f) => Some(f.to_string()),
        None => Some(default_filename(&args.address, "jpg")),
    };
    screenshot::save(&mut inst, filename.as_deref())?;
    if let Some(f) = &filename {
        println!("Saved screenshot image to {f}");
    }
    Ok(())
}

fn waveform_command(args: WaveformArgs) -> Result<()> {
    let mut inst = connect(args.usb, &args.address, args.port, args.timeout)?;
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

fn benchmark_command(args: BenchmarkArgs) -> Result<()> {
    let mut inst = connect(args.usb, &args.address, args.port, args.timeout)?;
    benchmark::run_with_progress(&mut inst, args.count)?;
    Ok(())
}

fn discover_command(args: DiscoverArgs) -> Result<()> {
    println!(
        "Probing {} for instruments - please wait...\n",
        args.address
    );
    let timeout = Duration::from_secs(args.timeout);
    let devices = discover::scan_ports(&args.address, &args.ports, timeout);
    if devices.is_empty() {
        println!("No devices found");
    } else {
        for d in &devices {
            println!("  Found \"{}\" on address {}", d.id.trim(), d.address);
        }
        println!(
            "Found {} device{}",
            devices.len(),
            if devices.len() > 1 { "s" } else { "" }
        );
    }
    Ok(())
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
    let result = match cli.command {
        Command::Scpi(args) => scpi_command(args),
        Command::Screenshot(args) => screenshot_command(args),
        Command::Waveform(args) => waveform_command(args),
        Command::Benchmark(args) => benchmark_command(args),
        Command::Discover(args) => discover_command(args),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
