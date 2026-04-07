#![allow(missing_docs)]

use helm_engine::{ExecMode, HelmEngine, Isa, StopReason};
use helm_timing::VirtualTiming;

const DEFAULT_FISH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/aarch64/binaries/fish"
);
const DEFAULT_INFLATE_TEST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/aarch64/binaries/inflate_test"
);

#[cfg(feature = "jit-dynasm")]
#[test]
#[ignore = "developer utility: run manually to inspect unsupported opcode histogram"]
fn print_se_jit_unsupported_histogram() {
    let binary =
        std::env::var("HELM_HIST_BINARY").unwrap_or_else(|_| DEFAULT_INFLATE_TEST.to_string());
    let max_insns = std::env::var("HELM_HIST_MAX_INSNS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(100_000);
    let argv_env = std::env::var("HELM_HIST_ARGV").unwrap_or_default();
    let argv_storage: Vec<String> = if argv_env.trim().is_empty() {
        if binary == DEFAULT_FISH {
            vec!["fish".into(), "-c".into(), "echo hello".into()]
        } else {
            vec![std::path::Path::new(&binary)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("binary")
                .to_string()]
        }
    } else {
        argv_env.split('\n').map(str::to_string).collect()
    };
    let argv_refs: Vec<&str> = argv_storage.iter().map(String::as_str).collect();

    let mut engine = HelmEngine::<VirtualTiming>::new(
        Isa::AArch64,
        ExecMode::Syscall,
        VirtualTiming::default(),
        0,
        256 * 1024 * 1024,
    );
    engine
        .load_aarch64_elf(
            &binary,
            &argv_refs,
            &[
                "HOME=/tmp/home/pmallapp",
                "TERM=dumb",
                "PATH=/usr/bin:/bin",
                "LANG=C",
                "USER=helm",
            ],
        )
        .expect("load AArch64 ELF");
    engine.set_jit(true);

    let mut remaining = max_insns;
    while remaining > 0 {
        let chunk = remaining.min(50_000);
        let stop = engine.run_jit(chunk);
        remaining = remaining.saturating_sub(chunk);
        if !matches!(stop, StopReason::Quantum) {
            break;
        }
    }

    let stats = engine.jit_perf_stats();
    let mut unsupported: Vec<_> = stats.unsupported_opcodes.into_iter().collect();
    unsupported.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    eprintln!("binary={binary}");
    eprintln!("insns_retired={}", engine.insns_retired);
    eprintln!("fallback_count={}", stats.fallback_count);
    eprintln!(
        "unsupported_block_starts={}",
        stats.unsupported_block_starts
    );
    eprintln!("unsupported_opcodes:");
    for (opcode, count) in unsupported {
        eprintln!("  {opcode:<24} {count}");
    }
}
