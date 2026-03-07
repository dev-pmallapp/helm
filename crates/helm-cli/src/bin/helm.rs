//! HELM command-line entry point.

use anyhow::Result;
use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(
    name = "helm",
    about = "HELM: Hybrid Emulation Layer for Microarchitecture"
)]
struct Cli {
    /// Path to the guest binary to execute.
    #[arg(short, long)]
    binary: String,

    /// Target ISA.
    #[arg(short, long, value_enum, default_value_t = Isa::Arm64)]
    isa: Isa,

    /// Execution mode.
    #[arg(short, long, value_enum, default_value_t = Mode::Se)]
    mode: Mode,

    /// Maximum instructions to execute.
    #[arg(long, default_value_t = 100_000_000)]
    max_insns: u64,

    /// Arguments passed to the guest binary (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    guest_args: Vec<String>,
}

#[derive(Clone, ValueEnum)]
enum Isa {
    X86_64,
    RiscV64,
    Arm64,
}

#[derive(Clone, ValueEnum)]
enum Mode {
    Se,
    Cae,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match (&cli.isa, &cli.mode) {
        (Isa::Arm64, Mode::Se) => run_aarch64_se(&cli),
        _ => {
            eprintln!("Only --isa arm64 --mode se is currently implemented.");
            std::process::exit(1);
        }
    }
}

fn run_aarch64_se(cli: &Cli) -> Result<()> {
    // Build argv: binary path + guest args
    let mut argv_strings = vec![cli.binary.clone()];
    argv_strings.extend(cli.guest_args.iter().cloned());
    let argv: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    // Default environment
    let envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C"];

    eprintln!("HELM SE mode: binary={} argv={:?}", cli.binary, argv);

    let result = helm_engine::run_aarch64_se(&cli.binary, &argv, &envp, cli.max_insns)?;

    eprintln!(
        "Exited with code {} after {} instructions",
        result.exit_code, result.instructions_executed
    );
    std::process::exit(result.exit_code as i32);
}
