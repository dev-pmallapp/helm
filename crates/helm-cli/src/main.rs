//! HELM command-line entry point.

use anyhow::Result;
use clap::{Parser, ValueEnum};
use helm_core::config::*;
use helm_core::types::{ExecMode, IsaKind};
use helm_engine::Simulation;

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
    #[arg(short, long, value_enum, default_value_t = Isa::RiscV64)]
    isa: Isa,

    /// Execution mode.
    #[arg(short, long, value_enum, default_value_t = Mode::Se)]
    mode: Mode,

    /// Path to a JSON platform configuration file (optional).
    #[arg(short, long)]
    config: Option<String>,

    /// Maximum simulation cycles (microarch mode).
    #[arg(long, default_value_t = 1_000_000)]
    max_cycles: u64,
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
    Microarch,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    let platform = if let Some(config_path) = &cli.config {
        let json = std::fs::read_to_string(config_path)?;
        serde_json::from_str::<PlatformConfig>(&json)?
    } else {
        default_platform(
            match cli.isa {
                Isa::X86_64 => IsaKind::X86_64,
                Isa::RiscV64 => IsaKind::RiscV64,
                Isa::Arm64 => IsaKind::Arm64,
            },
            match cli.mode {
                Mode::Se => ExecMode::SyscallEmulation,
                Mode::Microarch => ExecMode::Microarchitectural,
            },
        )
    };

    let mut sim = Simulation::new(platform, cli.binary);
    let results = sim.run(cli.max_cycles)?;
    println!("{}", results.to_json());
    Ok(())
}

/// Build a sensible default platform when no config file is given.
fn default_platform(isa: IsaKind, mode: ExecMode) -> PlatformConfig {
    PlatformConfig {
        name: "default".into(),
        isa,
        exec_mode: mode,
        cores: vec![CoreConfig {
            name: "core0".into(),
            width: 4,
            rob_size: 128,
            iq_size: 64,
            lq_size: 32,
            sq_size: 32,
            branch_predictor: BranchPredictorConfig::TAGE { history_length: 64 },
        }],
        memory: MemoryConfig {
            l1i: Some(CacheConfig {
                size: "32KB".into(),
                associativity: 8,
                latency_cycles: 1,
                line_size: 64,
            }),
            l1d: Some(CacheConfig {
                size: "32KB".into(),
                associativity: 8,
                latency_cycles: 1,
                line_size: 64,
            }),
            l2: Some(CacheConfig {
                size: "256KB".into(),
                associativity: 4,
                latency_cycles: 10,
                line_size: 64,
            }),
            l3: Some(CacheConfig {
                size: "8MB".into(),
                associativity: 16,
                latency_cycles: 30,
                line_size: 64,
            }),
            dram_latency_cycles: 100,
        },
    }
}
