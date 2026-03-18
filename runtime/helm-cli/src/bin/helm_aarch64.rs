//! `helm-aarch64` — AArch64 SE mode launcher with embedded Python interpreter.
//!
//! Follows the gem5 pattern: Python describes and drives the simulation,
//! Rust executes it.  The `_helm_ng` native module is registered before
//! Python starts and is always available as `import _helm_ng`.
//!
//! # Invocation modes
//!
//! ```text
//! # Run a Python config script (full control)
//! helm-aarch64 configs/se.py --binary ./hello --max-insns 100000000
//!
//! # Run a binary directly (uses the embedded run_binary.py script)
//! helm-aarch64 ./hello
//! helm-aarch64 ./hello arg1 arg2
//! ```
//!
//! # Python path
//!
//! The launcher prepends the following directories to `sys.path`:
//!
//! 1. `./python/`  — project-local Python layer (helm_ng package lives here)
//! 2. Script directory (when a `.py` file is given)
//!
//! This mirrors gem5's `<prefix>/lib/python/` convention.

use anyhow::Result;
use helm_cli::run_python;

/// The default SE script embedded at compile time.
/// Users can override by passing their own `.py` file on the command line.
const DEFAULT_SCRIPT: &str = include_str!("../../../../examples/se/run_binary.py");

fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_default_env()
        .init();

    let raw_args: Vec<String> = std::env::args().collect();
    // Extract --sim-trace=URI before handing args to the script
    let sim_trace_uri: Option<String> = raw_args.iter()
        .find(|a| a.starts_with("--sim-trace="))
        .map(|a| a["--sim-trace=".len()..].to_string());
    let filtered_args: Vec<String> = raw_args.iter()
        .filter(|a| !a.starts_with("--sim-trace="))
        .cloned()
        .collect();
    let (script_path, script_args) = detect_script(&filtered_args);
    run_python(
        script_path,
        &script_args,
        DEFAULT_SCRIPT,
        "examples/se/run_binary.py",
        "helm-aarch64",
        sim_trace_uri.as_deref(),
    )
}

/// Scan argv for a `.py` script.
///
/// Returns `(Some(path), rest_of_args)` when a `.py` is found,
/// or `(None, forwarded_args)` when running a binary directly.
///
/// In the binary-direct case:
/// - If the first non-flag argument looks like a file path (no leading `-`),
///   prepend `--binary` so run_binary.py can parse it.
/// - Otherwise, forward all args as-is (user already used `--binary`).
fn detect_script(raw: &[String]) -> (Option<String>, Vec<String>) {
    let args = &raw[1..]; // skip argv[0]

    for (i, a) in args.iter().enumerate() {
        if a.ends_with(".py") {
            return (Some(a.clone()), args[i + 1..].to_vec());
        }
    }

    // No .py found — forward args to the embedded script.
    // If the first arg doesn't start with '-', assume it's the binary path
    // and prepend --binary for the argparser.
    if let Some(first) = args.first() {
        if !first.starts_with('-') {
            let mut forwarded = vec!["--binary".to_string(), first.clone()];
            forwarded.extend_from_slice(&args[1..]);
            return (None, forwarded);
        }
    }

    // Already has flags (e.g. --binary ./hello), forward as-is.
    (None, args.to_vec())
}
