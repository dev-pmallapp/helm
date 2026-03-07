//! helm-system-arm — AArch64 full-system simulator.
//!
//! Like `qemu-system-aarch64`: boots kernels on emulated ARM platforms
//! with full device models, interrupt controllers, and peripheral buses.
//!
//! Usage:
//!     helm-system-arm -M realview-pb -kernel zImage
//!     helm-system-arm -M rpi3 -kernel kernel8.img -sd rootfs.img
//!     helm-system-arm -M virt -kernel Image -drive file=disk.img
//!     helm-system-arm -M virt -kernel Image -serial stdio -device virtio-net
//!     helm-system-arm config.py          # Python platform script
//!
//! The machine type (-M) selects a pre-built platform with the correct
//! memory map, buses, and device set. Additional devices can be added
//! via -device and -drive options.

use anyhow::{Context, Result};
use clap::Parser;
use helm_device::backend::{BufferCharBackend, NullCharBackend, StdioCharBackend};
use helm_device::loader::DynamicDeviceLoader;
use helm_device::platform::Platform;
use std::process::Command;

// Used only by helm-isa for CPU — the dependency comes via helm-engine
extern crate helm_core;

#[derive(Parser)]
#[command(
    name = "helm-system-arm",
    about = "HELM AArch64 full-system simulator",
    long_about = "Full-system ARM simulator with platform device models.\n\
                  Boots kernels on RealView-PB, RPi-3, or QEMU virt machines.",
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

    /// Serial port backend: stdio, null, file:path.
    #[arg(long = "serial", default_value = "stdio")]
    serial: String,

    /// Number of CPUs.
    #[arg(long = "smp", default_value_t = 1)]
    smp: u32,

    /// RAM size (e.g. 256M, 1G).
    #[arg(short = 'm', long = "memory", default_value = "256M")]
    memory_size: String,

    /// Timing model: fe, ape, cae.
    #[arg(long = "timing", default_value = "fe")]
    timing: String,

    /// Execution backend: interp, tcg.
    #[arg(long = "backend", default_value = "interp")]
    backend: String,

    /// Maximum instructions to execute (0 = unlimited).
    #[arg(short = 'n', long = "max-insns", default_value_t = 100_000_000)]
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
    let kernel = cli.kernel.as_deref()
        .or(cli.script.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no kernel specified (use -kernel or provide a .py script)"))?;

    let mut platform = build_platform(&cli)?;

    // Attach SD card as virtio-blk if provided
    if let Some(ref sd) = cli.sd_image {
        attach_sd_card(&mut platform, sd);
    }

    // Parse -drive options
    for drive_spec in &cli.drives {
        attach_drive(&mut platform, drive_spec)?;
    }

    // Parse -device options
    let mut loader = DynamicDeviceLoader::new();
    loader.register_arm_builtins();
    for dev_spec in &cli.devices {
        attach_device(&mut platform, dev_spec, &loader)?;
    }

    if cli.dump_config {
        dump_platform_config(&platform);
        return Ok(());
    }

    eprintln!("HELM system-arm: machine={} kernel={}", platform.name, kernel);
    eprintln!("  CPUs: {} | RAM: {} | Timing: {} | Backend: {}",
              cli.smp, cli.memory_size, cli.timing, cli.backend);
    eprintln!("  Devices: {}", platform.device_map().len());
    for (name, base) in platform.device_map() {
        eprintln!("    {name} @ {base:#010x}");
    }

    if cli.dry_run {
        eprintln!("HELM: dry run — configuration valid, not booting.");
        return Ok(());
    }

    // Load the kernel image (ARM64 Image format)
    let loaded = helm_engine::loader::load_arm64_image(
        kernel,
        cli.dtb.as_deref(),
        None, // initramfs loaded separately if needed
        None, // default RAM base
    ).with_context(|| format!("failed to load kernel: {kernel}"))?;

    eprintln!("  Kernel: {:#x} ({} bytes)", loaded.kernel_addr, loaded.kernel_size);
    eprintln!("  DTB:    {:#x}", loaded.dtb_addr);
    eprintln!("  Entry:  {:#x}", loaded.entry_point);
    eprintln!("  SP:     {:#x}", loaded.initial_sp);

    // Set up CPU and run
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    cpu.set_xn(0, loaded.dtb_addr);   // x0 = DTB address (ARM64 boot protocol)
    cpu.set_xn(1, 0);                 // x1 = 0
    cpu.set_xn(2, 0);                 // x2 = 0
    cpu.set_xn(3, 0);                 // x3 = 0

    let mut mem = loaded.address_space;

    // Build timing model
    let mut timing: Box<dyn helm_timing::TimingModel> = match cli.timing.as_str() {
        "ape" => Box::new(helm_timing::model::ApeModelDetailed::default()),
        "cae" => Box::new(helm_timing::model::ApeModelDetailed {
            branch_penalty: 14,
            ..Default::default()
        }),
        _ => Box::new(helm_timing::model::FeModel),
    };

    // Run: fetch-decode-execute loop with device bus
    let mut insn_count: u64 = 0;
    let mut virtual_cycles: u64 = 0;

    eprintln!("HELM: booting kernel...");

    loop {
        if insn_count >= cli.max_insns {
            eprintln!("HELM: hit instruction limit after {} instructions", insn_count);
            break;
        }

        let pc_before = cpu.regs.pc;
        match cpu.step(&mut mem) {
            Ok(trace) => {
                insn_count += 1;
                let mut stall = timing.instruction_latency_for_class(trace.class);
                for a in &trace.mem_accesses {
                    stall += timing.memory_latency(a.addr, a.size, a.is_write);
                    // Route device accesses through the platform bus
                    if platform.system_bus.contains(a.addr) {
                        if a.is_write {
                            let _ = platform.system_bus.bus_write(a.addr, a.size, 0);
                        } else {
                            let _ = platform.system_bus.bus_read(a.addr, a.size);
                        }
                    }
                }
                virtual_cycles += stall;

                // Tick devices every 1024 instructions
                if insn_count % 1024 == 0 {
                    let _ = platform.tick(1024);
                }
            }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                // In FS mode, SVC goes to the kernel's exception handler.
                // For now, if we hit an SVC with no handler, log and advance.
                if number == 0 {
                    // HLT / WFI — halt
                    eprintln!("HELM: CPU halted (WFI/HLT) at PC={:#x} after {} instructions",
                              pc_before, insn_count);
                    break;
                }
                // Otherwise advance past the SVC
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(helm_core::HelmError::Memory { addr, reason }) => {
                eprintln!("HELM: MEMORY FAULT at PC={:#x} addr={:#x}: {}", pc_before, addr, reason);
                // Try to route to device bus
                if platform.system_bus.contains(addr) {
                    // Device access — handle it
                    cpu.regs.pc += 4;
                    insn_count += 1;
                } else {
                    eprintln!("HELM: fatal memory fault — stopping");
                    break;
                }
            }
            Err(helm_core::HelmError::Isa(ref msg)) if msg.contains("unhandled") => {
                let mut insn_buf = [0u8; 4];
                let _ = mem.read(pc_before, &mut insn_buf);
                let insn_word = u32::from_le_bytes(insn_buf);
                eprintln!("HELM: unhandled instruction at PC={:#x}: {:#010x} ({})",
                          pc_before, insn_word, msg);
                eprintln!("HELM: {} instructions executed, {} virtual cycles", insn_count, virtual_cycles);
                break;
            }
            Err(e) => {
                eprintln!("HELM: error at PC={:#x}: {}", pc_before, e);
                break;
            }
        }
    }

    let ipc = if virtual_cycles > 0 {
        insn_count as f64 / virtual_cycles as f64
    } else {
        0.0
    };
    eprintln!("HELM: {} instructions, {} cycles, IPC={:.3}", insn_count, virtual_cycles, ipc);

    Ok(())
}

// ── Platform construction ───────────────────────────────────────────────────

fn build_platform(cli: &Cli) -> Result<Platform> {
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
        "realview-pb" | "realview" => {
            Ok(helm_device::realview_pb_platform(serial_backend))
        }
        "rpi3" | "raspi3" => {
            let serial2: Box<dyn helm_device::backend::CharBackend> = Box::new(NullCharBackend);
            Ok(helm_device::rpi3_platform(serial_backend, serial2))
        }
        "virt" | "arm-virt" => {
            let serial2: Box<dyn helm_device::backend::CharBackend> = Box::new(NullCharBackend);
            Ok(helm_device::arm_virt_platform(serial_backend, serial2))
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

fn attach_device(
    platform: &mut Platform,
    spec: &str,
    loader: &DynamicDeviceLoader,
) -> Result<()> {
    let parts: Vec<&str> = spec.split(',').collect();
    let dev_type = parts[0];
    let mut params = serde_json::Map::new();
    let mut base_addr: u64 = 0x0B00_0000 + (platform.device_map().len() as u64 * 0x1000);

    for part in &parts[1..] {
        if let Some((k, v)) = part.split_once('=') {
            if k == "base" || k == "addr" {
                base_addr = u64::from_str_radix(v.trim_start_matches("0x"), 16)
                    .unwrap_or(base_addr);
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
        let comma = if i + 1 < platform.device_map().len() { "," } else { "" };
        println!("    {{ \"name\": \"{name}\", \"base\": \"0x{base:08x}\" }}{comma}");
    }
    println!("  ]");
    println!("}}");
}

// ── Python config script ────────────────────────────────────────────────────

fn run_from_python(script: &str, cli: &Cli) -> Result<()> {
    eprintln!("HELM: loading platform from {script}");

    let output = Command::new("python3")
        .arg(script)
        .env("PYTHONPATH", {
            let cwd = std::env::current_dir().unwrap_or_default();
            format!("{}", cwd.join("python").display())
        })
        .output()
        .with_context(|| format!("failed to run {script}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{script} failed:\n{stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!("HELM: Python config output:\n{stdout}");

    // For now, the Python script outputs JSON which we'd parse to build the platform.
    // Full integration requires the embedded Python interpreter (PyO3).
    eprintln!("HELM: full embedded Python integration requires PyO3 build. \
               Use `helm-arm <script.py>` for SE mode or configure via CLI flags.");

    Ok(())
}

// ── Plugin registry ─────────────────────────────────────────────────────────

fn build_plugin_registry(
    names: &[String],
) -> Result<(helm_plugin::PluginRegistry, Vec<helm_plugin::runtime::PluginComponentAdapter>)> {
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
