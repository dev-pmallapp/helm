//! helm-system-aarch64 — AArch64 full-system simulator (thin launcher).
//!
//! This binary is a thin Python launcher.  When given a `.py` script as
//! the first positional argument, it sets `sys.argv` and executes the
//! script with the embedded Python interpreter.  When no `.py` is given,
//! it runs the embedded default `virt.py` script instead.
//!
//! Usage:
//!     helm-system-aarch64 examples/fs/virt.py --kernel Image
//!     helm-system-aarch64 --kernel Image          # uses embedded virt.py
//!     helm-system-aarch64 --list-machines

use anyhow::{Context, Result};

// Re-export the PyO3 module init function so append_to_inittab! can find it.
use _helm_core::_helm_core;

// Keep the link to helm_core for CPU / IrqSignal
extern crate helm_core;

/// Embedded default script — used when no `.py` is given on the command line.
const DEFAULT_SCRIPT: &str = include_str!("../../../../examples/fs/virt.py");

fn main() -> Result<()> {
    env_logger::init();
    let raw_args: Vec<String> = std::env::args().collect();

    // Quick info queries that don't need Python
    if raw_args.iter().any(|a| a == "--list-machines") {
        println!("Available machines:");
        println!("  virt         QEMU-style ARM virt machine (2 UARTs, VirtIO MMIO slots)");
        println!("  realview-pb  ARM RealView Platform Baseboard for Cortex-A8");
        println!("  rpi3         Raspberry Pi 3 Model B (BCM2837, 4-core Cortex-A53)");
        return Ok(());
    }
    if raw_args.iter().any(|a| a == "--list-devices") {
        use helm_device::loader::DynamicDeviceLoader;
        let mut loader = DynamicDeviceLoader::new();
        loader.register_arm_builtins();
        println!("Built-in device types:");
        let mut devs = loader.available_devices();
        devs.sort();
        for d in devs {
            println!("  {d}");
        }
        println!("\nBus protocols: apb, ahb, pci, usb, i2c, spi, axi");
        println!("\nVirtIO devices: virtio-blk, virtio-net, virtio-console, virtio-rng, ...");
        return Ok(());
    }

    // Determine if the first positional arg (not a flag) is a .py script.
    let (script_path, script_args) = detect_script(&raw_args);

    run_python(&script_path, &script_args)
}

/// Scan raw argv to find a `.py` script.
///
/// Returns `(script_file_or_None, args_to_forward)`.
/// When no `.py` is found, returns `(None, all_args_after_binary)`.
fn detect_script(raw: &[String]) -> (Option<String>, Vec<String>) {
    // Skip argv[0] (the binary itself)
    let args = &raw[1..];

    // Find the first positional arg that ends in `.py`
    for (i, a) in args.iter().enumerate() {
        if a.ends_with(".py") {
            // Everything after the script path is forwarded
            let rest = args[i + 1..].to_vec();
            return (Some(a.clone()), rest);
        }
    }

    // No .py found — all args forwarded to embedded default script
    (None, args.to_vec())
}

/// Execute a Python script (from file or embedded string) with sys.argv set.
fn run_python(script_path: &Option<String>, script_args: &[String]) -> Result<()> {
    pyo3::append_to_inittab!(_helm_core);
    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;
        use pyo3::types::PyList;

        // Add python/ and script directory to sys.path
        let cwd = std::env::current_dir().unwrap_or_default();
        let python_dir = cwd.join("python");

        let sys = py
            .import("sys")
            .map_err(|e| anyhow::anyhow!("failed to import sys: {e}"))?;
        let path = sys
            .getattr("path")
            .map_err(|e| anyhow::anyhow!("failed to get sys.path: {e}"))?;
        path.call_method1("insert", (0, python_dir.to_string_lossy().as_ref()))
            .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;

        // Build sys.argv: [script_name, ...script_args]
        let (code, argv0) = match script_path {
            Some(ref p) => {
                let script_dir = std::path::Path::new(p.as_str())
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_string_lossy()
                    .into_owned();
                path.call_method1("insert", (0, script_dir.as_str()))
                    .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;

                let code =
                    std::fs::read_to_string(p).with_context(|| format!("failed to read {p}"))?;
                (code, p.clone())
            }
            None => {
                eprintln!("HELM: using embedded virt.py platform script");
                (DEFAULT_SCRIPT.to_string(), "<embedded-virt>".to_string())
            }
        };

        let mut argv_items: Vec<String> = vec![argv0];
        argv_items.extend_from_slice(script_args);
        let argv_list = PyList::new(py, &argv_items)
            .map_err(|e| anyhow::anyhow!("failed to build sys.argv: {e}"))?;
        sys.setattr("argv", argv_list)
            .map_err(|e| anyhow::anyhow!("failed to set sys.argv: {e}"))?;

        py.run(&std::ffi::CString::new(code).unwrap(), None, None)
            .map_err(|e| {
                e.print(py);
                anyhow::anyhow!("Python script failed")
            })?;

        Ok(())
    })
}
