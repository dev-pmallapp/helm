//! helm-system-aarch64 — AArch64 full-system simulator.
//!
//! Like `qemu-system-aarch64`: boots kernels on emulated ARM platforms
//! with full device models, interrupt controllers, and peripheral buses.
//!
//! Usage:
//!     helm-system-aarch64 -M virt --kernel Image
//!     helm-system-aarch64 -M virt --kernel Image --monitor
//!     helm-system-aarch64 examples/fs/aarch64/boot_virt.py
//!
//! The machine type (-M) selects a pre-built platform with the correct
//! memory map, buses, and device set. Additional devices can be added
//! via -device and -drive options.

use anyhow::{Context, Result};
use clap::Parser;
use helm_device::backend::{BufferCharBackend, NullCharBackend, StdioCharBackend};
use helm_device::loader::DynamicDeviceLoader;
use helm_device::platform::Platform;

// Re-export the PyO3 module init function so append_to_inittab! can find it.
use _helm_core::_helm_core;

// Used only by helm-isa for CPU — the dependency comes via helm-engine
extern crate helm_core;

#[derive(Parser)]
#[command(
    name = "helm-system-aarch64",
    about = "HELM AArch64 full-system simulator",
    long_about = "Full-system AArch64 simulator with platform device models.\n\
                  Boots kernels on RealView-PB, RPi-3, or QEMU virt machines.\n\
                  Supports embedded Python scripts for programmatic control."
)]
struct Cli {
    /// Machine type: realview-pb, rpi3, virt.
    #[arg(short = 'M', long = "machine", default_value = "virt")]
    machine: String,

    /// Kernel image to boot (or .py config script).
    #[arg(short, long)]
    kernel: Option<String>,

    /// Python platform configuration script.
    /// If provided, overrides -M and all device options.
    #[arg()]
    script: Option<String>,

    /// Device tree blob.
    #[arg(long)]
    dtb: Option<String>,

    /// SD card / disk image.
    #[arg(long = "sd")]
    sd_image: Option<String>,

    /// Drive specification (file=path,format=raw,if=none,id=drive0).
    #[arg(long = "drive", value_name = "SPEC")]
    drives: Vec<String>,

    /// Add a device (type,key=val,...).
    #[arg(long = "device", value_name = "SPEC")]
    devices: Vec<String>,

    /// Add a driver (alias for -device, compatible with QEMU conventions).
    #[arg(long = "driver", value_name = "SPEC")]
    drivers: Vec<String>,

    /// Serial port backend: stdio, null, file:path.
    #[arg(long = "serial", default_value = "stdio")]
    serial: String,

    /// Number of CPUs.
    #[arg(long = "smp", default_value_t = 1)]
    smp: u32,

    /// RAM size (e.g. 256M, 1G).
    #[arg(short = 'm', long = "memory", default_value = "256M")]
    memory_size: String,

    /// Kernel command line (bootargs).
    #[arg(long = "append", value_name = "CMDLINE")]
    append: Option<String>,

    /// Initramfs / initrd image.
    #[arg(long = "initrd", value_name = "FILE")]
    initrd: Option<String>,

    /// BIOS / firmware image (e.g. EDK2 for UEFI boot).
    /// When present, implies firmware-driven boot — no DTB is generated.
    #[arg(long = "bios", value_name = "FILE")]
    bios: Option<String>,

    /// Dump the generated DTB to a file and exit.
    #[arg(long = "dump-dtb", value_name = "FILE")]
    dump_dtb: Option<String>,

    /// Timing model: fe, ape, cae.
    #[arg(long = "timing", default_value = "fe")]
    timing: String,

    /// Execution backend: jit, tcg, interp.
    #[arg(long = "backend", default_value = "jit")]
    backend: String,

    /// Maximum instructions to execute (0 = unlimited).
    /// Maximum instructions to execute (0 = unlimited).
    #[arg(short = 'n', long = "max-insns", default_value_t = 0)]
    max_insns: u64,

    /// Enable a plugin (repeatable).
    #[arg(long = "plugin", value_name = "NAME")]
    plugins: Vec<String>,

    /// Log device accesses.
    #[arg(long = "trace-devices", default_value_t = false)]
    trace_devices: bool,

    /// List available machines and exit.
    #[arg(long = "list-machines", default_value_t = false)]
    list_machines: bool,

    /// List available device types and exit.
    #[arg(long = "list-devices", default_value_t = false)]
    list_devices: bool,

    /// Dump platform configuration as JSON and exit.
    #[arg(long = "dump-config", default_value_t = false)]
    dump_config: bool,

    /// Do not boot — just validate configuration.
    #[arg(long = "dry-run", default_value_t = false)]
    dry_run: bool,

    /// Enable interactive monitor (QEMU-like debugging console).
    #[arg(long = "monitor", default_value_t = false)]
    monitor: bool,

    /// Path to System.map for symbol resolution.
    #[arg(long = "sysmap", value_name = "FILE")]
    sysmap: Option<String>,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    // List machines
    if cli.list_machines {
        println!("Available machines:");
        println!("  virt         QEMU-style ARM virt machine (2 UARTs, VirtIO MMIO slots)");
        println!("  realview-pb  ARM RealView Platform Baseboard for Cortex-A8");
        println!("  rpi3         Raspberry Pi 3 Model B (BCM2837, 4-core Cortex-A53)");
        return Ok(());
    }

    // List devices
    if cli.list_devices {
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

    // Python config script mode
    if let Some(ref script) = cli.script {
        if script.ends_with(".py") {
            return run_from_python(script, &cli);
        }
    }

    // Build platform
    let kernel = cli
        .kernel
        .as_deref()
        .or(cli.script.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!("no kernel specified (use -kernel or provide a .py script)")
        })?;

    // Create shared IRQ signal for GIC → CPU communication
    let irq_signal = helm_core::IrqSignal::new();
    let mut platform = build_platform(&cli, Some(irq_signal.clone()))?;

    // Attach SD card as virtio-blk if provided
    if let Some(ref sd) = cli.sd_image {
        attach_sd_card(&mut platform, sd);
    }

    // Parse -drive options
    for drive_spec in &cli.drives {
        attach_drive(&mut platform, drive_spec)?;
    }

    // Parse -device options
    // Merge -device and -driver into a single spec list
    let all_device_specs: Vec<helm_device::DeviceSpec> = cli
        .devices
        .iter()
        .chain(cli.drivers.iter())
        .map(|s| helm_device::DeviceSpec::parse(s))
        .collect();

    {
        let mut loader = DynamicDeviceLoader::new();
        loader.register_arm_builtins();
        for spec in &all_device_specs {
            let spec_str = format!(
                "{},{}",
                spec.type_name,
                spec.properties
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            attach_device(&mut platform, &spec_str, &loader)?;
        }
    }

    // ── DTB generation ──────────────────────────────────────────────────
    let ram_size = helm_device::parse_ram_size(&cli.memory_size).unwrap_or(256 * 1024 * 1024);

    let dtb_config = helm_device::DtbConfig {
        ram_base: 0x4000_0000,
        ram_size,
        num_cpus: cli.smp,
        bootargs: cli.append.clone().unwrap_or_default(),
        initrd: cli.initrd.as_ref().map(|_| {
            // Initrd address will be set by the loader at DEFAULT_INITRD_ADDR
            let initrd_base = 0x4000_0000u64 + 0x0400_0000; // RAM + 64MB
            let initrd_size = cli
                .initrd
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            (initrd_base, initrd_base + initrd_size)
        }),
        extra_devices: all_device_specs.clone(),
        ..Default::default()
    };

    let base_blob: Option<Vec<u8>> = if let Some(ref dtb_path) = cli.dtb {
        let data =
            std::fs::read(dtb_path).with_context(|| format!("failed to read DTB: {dtb_path}"))?;
        eprintln!(
            "HELM: loaded base DTB from {dtb_path} ({} bytes)",
            data.len()
        );
        Some(data)
    } else {
        None
    };

    let infer_ctx = helm_device::InferCtx::from_platform(
        &platform,
        cli.kernel.is_some(),
        cli.bios.is_some(),
        !cli.drives.is_empty() || cli.sd_image.is_some(),
        cli.dtb.is_some(),
        !all_device_specs.is_empty(),
    );
    let resolved =
        helm_device::resolve_dtb(&platform, &dtb_config, base_blob.as_deref(), &infer_ctx);
    let inferred_policy = helm_device::DtbPolicy::infer(&infer_ctx);

    if let Some(ref dump_path) = cli.dump_dtb {
        match &resolved {
            helm_device::ResolvedDtb::Blob(blob) => {
                std::fs::write(dump_path, blob)
                    .with_context(|| format!("failed to write DTB to {dump_path}"))?;
                eprintln!("HELM: wrote DTB ({} bytes) to {dump_path}", blob.len());
            }
            helm_device::ResolvedDtb::None => {
                eprintln!("HELM: DTB policy is 'none' — no DTB to dump");
            }
        }
        return Ok(());
    }

    let effective_dtb: Option<String> = match &resolved {
        helm_device::ResolvedDtb::Blob(blob) => {
            let dtb_tmp = std::env::temp_dir().join("helm-virt.dtb");
            std::fs::write(&dtb_tmp, blob).with_context(|| "failed to write DTB")?;
            eprintln!(
                "HELM: DTB {} ({} bytes, policy={})",
                if base_blob.is_some() {
                    "patched"
                } else {
                    "generated"
                },
                blob.len(),
                inferred_policy
            );
            if let Some(root) = helm_device::parse_dtb(blob) {
                eprintln!("HELM: DTB contains {} node(s)", count_nodes(&root));
            }
            Some(dtb_tmp.to_string_lossy().into_owned())
        }
        helm_device::ResolvedDtb::None => {
            eprintln!("HELM: no DTB (policy={})", inferred_policy);
            None
        }
    };

    // Build plugin registry if plugins were requested
    if !cli.plugins.is_empty() {
        let (_plugin_reg, _adapters) = build_plugin_registry(&cli.plugins)?;
        eprintln!("HELM: loaded {} plugin(s)", cli.plugins.len());
    }

    if cli.dump_config {
        dump_platform_config(&platform);
        return Ok(());
    }

    if cli.dry_run {
        eprintln!("HELM: dry run — configuration valid, not booting.");
        return Ok(());
    }

    // Decide whether to use traced (per-instruction trace) or fast (JIT/interp) path.
    // Traced path is needed for: --monitor, --timing ape/cae, --trace-devices
    let needs_trace = cli.monitor || cli.trace_devices
        || matches!(cli.timing.as_str(), "ape" | "cae");
    let effective_backend = if needs_trace { "interp" } else { &cli.backend };

    eprintln!(
        "HELM system-arm: machine={} kernel={}",
        platform.name, kernel
    );
    eprintln!(
        "  CPUs: {} | RAM: {} | Timing: {} | Backend: {}",
        cli.smp, cli.memory_size, cli.timing, effective_backend
    );
    eprintln!("  Devices: {}", platform.device_map().len());
    for (name, base) in platform.device_map() {
        eprintln!("    {name} @ {base:#010x}");
    }

    let opts = helm_engine::FsOpts {
        machine: cli.machine.clone(),
        append: cli.append.clone().unwrap_or_default(),
        memory_size: cli.memory_size.clone(),
        dtb: effective_dtb.clone(),
        initrd: cli.initrd.clone(),
        sysmap: cli.sysmap.clone(),
        serial: cli.serial.clone(),
        timing: cli.timing.clone(),
        backend: effective_backend.to_string(),
        max_insns: cli.max_insns,
    };

    let mut session = helm_engine::FsSession::new(kernel, &opts)
        .with_context(|| "failed to create FS session")?;

    if cli.monitor {
        let mut monitor = helm_engine::Monitor::new();
        monitor.run_interactive(&mut session);
    } else {
        let limit = if cli.max_insns == 0 { u64::MAX } else { cli.max_insns };
        eprintln!("HELM: booting kernel...");
        let result = session.run(limit);
        let stats = session.stats();
        eprintln!(
            "HELM: {} instructions, {} cycles, IPC={:.3}, IRQs={}, result={:?}",
            stats.insn_count, stats.virtual_cycles,
            if stats.virtual_cycles > 0 {
                stats.insn_count as f64 / stats.virtual_cycles as f64
            } else {
                0.0
            },
            stats.irq_count,
            result,
        );
    }

    Ok(())
}

// ── Platform construction ───────────────────────────────────────────────────

fn build_platform(cli: &Cli, irq_signal: Option<helm_core::IrqSignal>) -> Result<Platform> {
    let serial_backend: Box<dyn helm_device::backend::CharBackend> = match cli.serial.as_str() {
        "null" => Box::new(NullCharBackend),
        "stdio" => Box::new(StdioCharBackend),
        s if s.starts_with("file:") => {
            // For file backend, use buffer for now (real file backend would need std::fs)
            Box::new(BufferCharBackend::new())
        }
        _ => Box::new(StdioCharBackend),
    };

    match cli.machine.as_str() {
        "realview-pb" | "realview" => Ok(helm_device::realview_pb_platform(serial_backend)),
        "rpi3" | "raspi3" => {
            let serial2: Box<dyn helm_device::backend::CharBackend> = Box::new(NullCharBackend);
            Ok(helm_device::rpi3_platform(serial_backend, serial2))
        }
        "virt" | "arm-virt" => {
            let serial2: Box<dyn helm_device::backend::CharBackend> = Box::new(NullCharBackend);
            Ok(helm_device::arm_virt_platform(
                serial_backend,
                serial2,
                irq_signal,
            ))
        }
        other => {
            anyhow::bail!(
                "unknown machine type '{}'. Use --list-machines to see available options.",
                other
            );
        }
    }
}

fn attach_sd_card(platform: &mut Platform, path: &str) {
    use helm_device::virtio::blk::VirtioBlk;
    use helm_device::virtio::transport::VirtioMmioTransport;

    // VirtIO block device at standard slot
    let blk = VirtioBlk::new(0); // 0-byte disk (path would be loaded in real impl)
    let transport = VirtioMmioTransport::new(Box::new(blk));
    platform.add_device("virtio-blk-sd", 0x0A00_0000, Box::new(transport));
    eprintln!("HELM: attached SD card image: {path}");
}

fn attach_drive(platform: &mut Platform, spec: &str) -> Result<()> {
    let mut file_path = String::new();
    let mut drive_id = String::from("drive0");

    for part in spec.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "file" => file_path = v.to_string(),
                "id" => drive_id = v.to_string(),
                _ => {}
            }
        }
    }

    if file_path.is_empty() {
        anyhow::bail!("drive spec missing file= parameter");
    }

    use helm_device::virtio::blk::VirtioBlk;
    use helm_device::virtio::transport::VirtioMmioTransport;

    let blk = VirtioBlk::new(0);
    let transport = VirtioMmioTransport::new(Box::new(blk));
    let base = 0x0A00_0000 + (platform.device_map().len() as u64 * 0x200);
    platform.add_device(&drive_id, base, Box::new(transport));
    eprintln!("HELM: attached drive {drive_id}: {file_path}");
    Ok(())
}

fn attach_device(platform: &mut Platform, spec: &str, loader: &DynamicDeviceLoader) -> Result<()> {
    let parts: Vec<&str> = spec.split(',').collect();
    let dev_type = parts[0];
    let mut params = serde_json::Map::new();
    let mut base_addr: u64 = 0x0B00_0000 + (platform.device_map().len() as u64 * 0x1000);

    for part in &parts[1..] {
        if let Some((k, v)) = part.split_once('=') {
            if k == "base" || k == "addr" {
                base_addr =
                    u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(base_addr);
            } else {
                params.insert(k.to_string(), serde_json::Value::String(v.to_string()));
            }
        }
    }

    let config = serde_json::Value::Object(params);

    if let Ok(device) = loader.create_device(dev_type, &config) {
        platform.add_device(dev_type, base_addr, device);
        eprintln!("HELM: attached device {dev_type} @ {base_addr:#010x}");
    } else {
        // Try VirtIO devices
        match dev_type {
            "virtio-net" => {
                use helm_device::virtio::net::VirtioNet;
                use helm_device::virtio::transport::VirtioMmioTransport;
                let transport = VirtioMmioTransport::new(Box::new(VirtioNet::new()));
                platform.add_device("virtio-net", base_addr, Box::new(transport));
                eprintln!("HELM: attached virtio-net @ {base_addr:#010x}");
            }
            "virtio-rng" => {
                use helm_device::virtio::rng::VirtioRng;
                use helm_device::virtio::transport::VirtioMmioTransport;
                let transport = VirtioMmioTransport::new(Box::new(VirtioRng::new()));
                platform.add_device("virtio-rng", base_addr, Box::new(transport));
                eprintln!("HELM: attached virtio-rng @ {base_addr:#010x}");
            }
            "virtio-console" => {
                use helm_device::virtio::console::VirtioConsole;
                use helm_device::virtio::transport::VirtioMmioTransport;
                let transport = VirtioMmioTransport::new(Box::new(VirtioConsole::new()));
                platform.add_device("virtio-console", base_addr, Box::new(transport));
                eprintln!("HELM: attached virtio-console @ {base_addr:#010x}");
            }
            _ => {
                eprintln!("HELM: warning: unknown device type '{dev_type}'");
            }
        }
    }

    Ok(())
}

fn dump_platform_config(platform: &Platform) {
    println!("{{");
    println!("  \"name\": \"{}\",", platform.name);
    println!("  \"devices\": [");
    for (i, (name, base)) in platform.device_map().iter().enumerate() {
        let comma = if i + 1 < platform.device_map().len() {
            ","
        } else {
            ""
        };
        println!("    {{ \"name\": \"{name}\", \"base\": \"0x{base:08x}\" }}{comma}");
    }
    println!("  ]");
    println!("}}");
}

// ── Python config script ────────────────────────────────────────────────────

fn run_from_python(script: &str, _cli: &Cli) -> Result<()> {
    eprintln!("HELM: running Python script with embedded interpreter: {script}");

    // Register the _helm_core module BEFORE Python is initialized.
    // This makes `import _helm_core` work inside the embedded interpreter,
    // giving the script direct access to FsSession, SeSession, etc.
    pyo3::append_to_inittab!(_helm_core);

    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;
        // Add the python/ directory to sys.path so `import helm` works
        let cwd = std::env::current_dir().unwrap_or_default();
        let python_dir = cwd.join("python");
        let script_dir = std::path::Path::new(script)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_string_lossy()
            .into_owned();

        let sys = py
            .import("sys")
            .map_err(|e| anyhow::anyhow!("failed to import sys: {e}"))?;
        let path = sys
            .getattr("path")
            .map_err(|e| anyhow::anyhow!("failed to get sys.path: {e}"))?;
        path.call_method1("insert", (0, python_dir.to_string_lossy().as_ref()))
            .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;
        path.call_method1("insert", (0, script_dir.as_str()))
            .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;

        // Read and execute the script
        let code =
            std::fs::read_to_string(script).with_context(|| format!("failed to read {script}"))?;

        py.run(&std::ffi::CString::new(code).unwrap(), None, None)
            .map_err(|e| {
                e.print(py);
                anyhow::anyhow!("Python script failed")
            })?;

        Ok(())
    })
}

// ── Plugin registry ─────────────────────────────────────────────────────────

fn build_plugin_registry(
    names: &[String],
) -> Result<(
    helm_plugin::PluginRegistry,
    Vec<helm_plugin::runtime::PluginComponentAdapter>,
)> {
    use helm_plugin::api::ComponentRegistry;
    use helm_plugin::runtime::{register_builtins, PluginComponentAdapter};
    use helm_plugin::{PluginArgs, PluginRegistry};

    let mut comp_reg = ComponentRegistry::new();
    register_builtins(&mut comp_reg);

    let mut plugin_reg = PluginRegistry::new();
    let mut adapters: Vec<PluginComponentAdapter> = Vec::new();

    for name in names {
        let fqn = match name.as_str() {
            "cache" => "plugin.memory.cache".to_string(),
            other => format!("plugin.trace.{other}"),
        };
        match comp_reg.create(&fqn) {
            Some(comp) => {
                let raw = Box::into_raw(comp);
                let mut adapter = unsafe { *Box::from_raw(raw as *mut PluginComponentAdapter) };
                adapter.install(&mut plugin_reg, &PluginArgs::new());
                adapters.push(adapter);
                eprintln!("HELM: enabled plugin {fqn}");
            }
            None => {
                eprintln!("HELM: unknown plugin '{name}', skipping");
            }
        }
    }

    Ok((plugin_reg, adapters))
}
fn count_nodes(node: &helm_device::FdtNode) -> usize {
    1 + node.children.iter().map(|c| count_nodes(c)).sum::<usize>()
}
