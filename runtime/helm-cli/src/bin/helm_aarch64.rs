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

use anyhow::{Context, Result};

// Re-export the PyO3 module init function so append_to_inittab! can find it.
// The helm-python crate sets [lib] name = "_helm_ng", so the crate name is _helm_ng.
use _helm_ng::_helm_ng;

/// The default SE script embedded at compile time.
/// Users can override by passing their own `.py` file on the command line.
const DEFAULT_SCRIPT: &str = include_str!("../../../../examples/se/run_binary.py");

fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_default_env()
        .init();

    let raw_args: Vec<String> = std::env::args().collect();
    let (script_path, script_args) = detect_script(&raw_args);
    run_python(script_path, &script_args)
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

/// Boot the embedded Python interpreter and execute the config script.
fn run_python(script_path: Option<String>, script_args: &[String]) -> Result<()> {
    // Register _helm_ng before Python starts so `import _helm_ng` always works.
    // The module init fn is named after the #[pymodule] fn in helm-python.
    pyo3::append_to_inittab!(_helm_ng);
    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;
        use pyo3::types::PyList;

        // -- sys.path setup ------------------------------------------------
        #[allow(deprecated)]
        let sys = py.import_bound("sys")
            .map_err(|e| anyhow::anyhow!("import sys failed: {e}"))?;
        let path = sys.getattr("path")
            .map_err(|e| anyhow::anyhow!("sys.path failed: {e}"))?;

        // Prepend ./python/ so `import helm_ng` works out of the box.
        let cwd        = std::env::current_dir().unwrap_or_default();
        let python_dir = cwd.join("python");
        path.call_method1("insert", (0i32, python_dir.to_string_lossy().as_ref()))
            .map_err(|e| anyhow::anyhow!("sys.path insert failed: {e}"))?;

        // -- Script selection ----------------------------------------------
        let (code, argv0): (String, String) = match &script_path {
            Some(p) => {
                // Also add the script's own directory to sys.path.
                let script_dir = std::path::Path::new(p.as_str())
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_string_lossy()
                    .into_owned();
                path.call_method1("insert", (0i32, script_dir.as_str()))
                    .map_err(|e| anyhow::anyhow!("sys.path insert failed: {e}"))?;

                let code = std::fs::read_to_string(p)
                    .with_context(|| format!("cannot read script {p}"))?;
                (code, p.clone())
            }
            None => {
                log::info!("helm-aarch64: using embedded run_binary.py");
                (DEFAULT_SCRIPT.to_string(), "<embedded:run_binary.py>".to_string())
            }
        };

        // -- sys.argv ------------------------------------------------------
        let mut argv_items = vec![argv0];
        argv_items.extend_from_slice(script_args);
        #[allow(deprecated)]
        let argv_list = PyList::new_bound(py, &argv_items);
        sys.setattr("argv", &argv_list)
            .map_err(|e| anyhow::anyhow!("sys.argv failed: {e}"))?;

        // -- Execute -------------------------------------------------------
        #[allow(deprecated)]
        py.run_bound(&code, None, None).map_err(|e: pyo3::PyErr| {
            e.print(py);
            anyhow::anyhow!("Python script exited with an error")
        })?;

        Ok(())
    })
}
