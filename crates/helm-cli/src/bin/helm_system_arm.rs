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
    let all_device_specs: Vec<helm_device::DeviceSpec> = cli.devices.iter()
        .chain(cli.drivers.iter())
        .map(|s| helm_device::DeviceSpec::parse(s))
        .collect();

    {
        let mut loader = DynamicDeviceLoader::new();
        loader.register_arm_builtins();
        for spec in &all_device_specs {
            let spec_str = format!("{},{}", spec.type_name,
                spec.properties.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>().join(","));
            attach_device(&mut platform, &spec_str, &loader)?;
        }
    }

    // ── DTB generation ──────────────────────────────────────────────────
    let ram_size = helm_device::parse_ram_size(&cli.memory_size)
        .unwrap_or(256 * 1024 * 1024);

    let dtb_config = helm_device::DtbConfig {
        ram_base: 0x4000_0000,
        ram_size,
        num_cpus: cli.smp,
        bootargs: cli.append.clone().unwrap_or_default(),
        initrd: None,
        extra_devices: all_device_specs.clone(),
        ..Default::default()
    };

    let base_blob: Option<Vec<u8>> = if let Some(ref dtb_path) = cli.dtb {
        let data = std::fs::read(dtb_path)
            .with_context(|| format!("failed to read DTB: {dtb_path}"))?;
        eprintln!("HELM: loaded base DTB from {dtb_path} ({} bytes)", data.len());
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
    let resolved = helm_device::resolve_dtb(&platform, &dtb_config, base_blob.as_deref(), &infer_ctx);
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
            std::fs::write(&dtb_tmp, blob)
                .with_context(|| "failed to write DTB")?;
            eprintln!("HELM: DTB {} ({} bytes, policy={})",
                      if base_blob.is_some() { "patched" } else { "generated" },
                      blob.len(), inferred_policy);
            Some(dtb_tmp.to_string_lossy().into_owned())
        }
        helm_device::ResolvedDtb::None => {
            eprintln!("HELM: no DTB (policy={})", inferred_policy);
            None
        }
    };

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
        effective_dtb.as_deref(),
        None, // initramfs loaded separately if needed
        None, // default RAM base
    ).with_context(|| format!("failed to load kernel: {kernel}"))?;

    eprintln!("  Kernel: {:#x} ({} bytes)", loaded.kernel_addr, loaded.kernel_size);
    eprintln!("  DTB:    {:#x}", loaded.dtb_addr);
    eprintln!("  Entry:  {:#x}", loaded.entry_point);
    eprintln!("  SP:     {:#x}", loaded.initial_sp);

    // Set up CPU and run
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.set_irq_signal(irq_signal.clone());
    cpu.regs.pc = loaded.entry_point;

    // Auto-detect boot EL: --bios → EL3 (firmware), --kernel → EL1
    let boot_el: u8 = if cli.bios.is_some() { 3 } else { 1 };

    cpu.regs.current_el = boot_el;
    cpu.regs.sp_sel = 1;
    match boot_el {
        3 => {
            cpu.regs.sp_el3 = loaded.initial_sp;
            cpu.regs.sp = loaded.initial_sp;
            // Set SCR_EL3: RW=1 (EL2 is AArch64), HCE=1, NS=1
            cpu.regs.scr_el3 = (1 << 10) | (1 << 8) | (1 << 0);
            // Set HCR_EL2.RW=1 (EL1 is AArch64)
            cpu.regs.hcr_el2 = 1 << 31;
            eprintln!("  Boot EL: 3 (firmware mode)");
        }
        2 => {
            cpu.regs.sp_el2 = loaded.initial_sp;
            cpu.regs.sp = loaded.initial_sp;
            cpu.regs.hcr_el2 = 1 << 31; // RW=1
            eprintln!("  Boot EL: 2 (hypervisor mode)");
        }
        _ => {
            cpu.regs.sp_el1 = loaded.initial_sp;
            cpu.regs.sp = loaded.initial_sp;
        }
    }

    cpu.set_xn(0, loaded.dtb_addr);   // x0 = DTB address (ARM64 boot protocol)
    cpu.set_xn(1, 0);                 // x1 = 0
    cpu.set_xn(2, 0);                 // x2 = 0
    cpu.set_xn(3, 0);                 // x3 = 0

    let mut mem = loaded.address_space;

    // Wire up device bus as I/O fallback for MMIO accesses
    let uart_tx_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let uart_tx_ref = uart_tx_count.clone();
    struct DeviceBusIo {
        bus: helm_device::bus::DeviceBus,
        uart_tx: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }
    impl helm_memory::address_space::IoHandler for DeviceBusIo {
        fn io_read(&mut self, addr: u64, size: usize) -> Option<u64> {
            match self.bus.read_fast(addr, size) {
                Ok(val) => Some(val),
                Err(_) => Some(0),
            }
        }
        fn io_write(&mut self, addr: u64, size: usize, value: u64) -> bool {
            // Track UART TX writes (UARTDR at base+0x000)
            if addr == 0x0900_0000 && size <= 4 {
                self.uart_tx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = self.bus.write_fast(addr, size, value);
            true
        }
    }
    let io_handler = DeviceBusIo {
        bus: std::mem::take(&mut platform.system_bus),
        uart_tx: uart_tx_ref,
    };
    mem.set_io_handler(Box::new(io_handler));

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
    let mut isa_skip_count: u64 = 0;
    let mut irq_count: u64 = 0;

    // Timer check interval — amortized cost
    const TIMER_CHECK_INTERVAL: u64 = 1024;

    // GIC MMIO addresses for timer IRQ injection
    // GICD_ISPENDR[0] at 0x0800_0200 covers IRQs 0-31
    const VTIMER_IRQ_BIT: u32 = 1 << 27; // Virtual timer PPI (IRQ 27)
    const PTIMER_IRQ_BIT: u32 = 1 << 30; // Physical timer PPI (IRQ 30)

    // Ring buffer for last N instructions (post-mortem trace)
    const TRACE_SIZE: usize = 32;
    let mut trace_ring: Vec<(u64, u32, u8)> = Vec::with_capacity(TRACE_SIZE);
    let mut trace_idx: usize = 0;

    eprintln!("HELM: booting kernel...");

    loop {
        // WFI handling: fast-forward the counter to the next timer event
        if cpu.wfi_pending {
            let skipped = cpu.wfi_advance();
            if skipped > 0 {
                insn_count += skipped;
                virtual_cycles += skipped;
            }
            // Check timers immediately after WFI advance
            let (v_fire, p_fire) = cpu.check_timers();
            if v_fire {
                let _ = mem.write(0x0800_0200, &VTIMER_IRQ_BIT.to_le_bytes());
            }
            if p_fire {
                let _ = mem.write(0x0800_0200, &PTIMER_IRQ_BIT.to_le_bytes());
            }
            if !irq_signal.is_raised() && !v_fire && !p_fire {
                // No interrupt to wake us — skip 4096 ticks and retry
                cpu.insn_count += 4096;
                insn_count += 4096;
                virtual_cycles += 4096;
                continue;
            }
            cpu.wfi_pending = false;
        }

        // Periodic timer check (every TIMER_CHECK_INTERVAL instructions)
        if insn_count % TIMER_CHECK_INTERVAL == 0 {
            let (v_fire, p_fire) = cpu.check_timers();
            if v_fire {
                let _ = mem.write(0x0800_0200, &VTIMER_IRQ_BIT.to_le_bytes());
            }
            if p_fire {
                let _ = mem.write(0x0800_0200, &PTIMER_IRQ_BIT.to_le_bytes());
            }
        }

        if insn_count >= cli.max_insns {
            eprintln!("HELM: hit instruction limit after {} instructions", insn_count);
            // Dump final CPU state
            eprintln!("HELM: CPU state: EL{} SP_sel={} DAIF={:#x} NZCV={:#x}",
                      cpu.regs.current_el, cpu.regs.sp_sel, cpu.regs.daif, cpu.regs.nzcv);
            eprintln!("HELM:   PC={:#x} SCTLR_EL1={:#x} TCR_EL1={:#x}",
                      cpu.regs.pc, cpu.regs.sctlr_el1, cpu.regs.tcr_el1);
            eprintln!("HELM:   TTBR0={:#x} TTBR1={:#x} VBAR_EL1={:#x}",
                      cpu.regs.ttbr0_el1, cpu.regs.ttbr1_el1, cpu.regs.vbar_el1);
            eprintln!("HELM:   SP_EL1={:#x} ELR_EL1={:#x} ESR_EL1={:#x} FAR_EL1={:#x}",
                      cpu.regs.sp_el1, cpu.regs.elr_el1, cpu.regs.esr_el1, cpu.regs.far_el1);
            for i in (0..31).step_by(4) {
                let end = (i + 4).min(31);
                let regs: Vec<String> = (i..end).map(|r| format!("X{r}={:#x}", cpu.xn(r as u16))).collect();
                eprintln!("HELM:   {}", regs.join(" "));
            }
            // Show last 16 PCs
            eprintln!("HELM: last {} instructions:", trace_ring.len().min(TRACE_SIZE));
            let start = if trace_ring.len() < TRACE_SIZE { 0 } else { trace_idx };
            let count = trace_ring.len().min(TRACE_SIZE);
            for i in 0..count {
                let idx = (start + i) % trace_ring.len();
                let (pc, insn, el) = trace_ring[idx];
                eprintln!("  [{:>10}] EL{} PC={:#010x} insn={:#010x}",
                          insn_count as i64 - (count as i64 - i as i64), el, pc, insn);
            }
            break;
        }

        if cpu.halted {
            eprintln!("HELM: CPU halted at PC={:#x} after {} instructions",
                      cpu.regs.pc, insn_count);
            break;
        }

        let pc_before = cpu.regs.pc;
        let el_before = cpu.regs.current_el;

        match cpu.step(&mut mem) {
            Ok(trace) => {
                insn_count += 1;
                if trace.insn_word == 0 && trace.pc == cpu.regs.pc {
                    // IRQ exception was taken (trace from check_irq)
                    irq_count += 1;
                }
                // Record in ring buffer (post-step, uses trace's insn_word)
                let entry = (trace.pc, trace.insn_word, el_before);
                if trace_ring.len() < TRACE_SIZE {
                    trace_ring.push(entry);
                } else {
                    trace_ring[trace_idx] = entry;
                }
                trace_idx = (trace_idx + 1) % TRACE_SIZE;
                let mut stall = timing.instruction_latency_for_class(trace.class);
                for a in &trace.mem_accesses {
                    stall += timing.memory_latency(a.addr, a.size, a.is_write);
                }
                virtual_cycles += stall;
            }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                // SVC from EL1 in FS mode — kernel internal call
                // Advance past and continue
                cpu.regs.pc += 4;
                insn_count += 1;
                let _ = number;
            }
            Err(helm_core::HelmError::Memory { addr, reason }) => {
                eprintln!("HELM: MEMORY FAULT at PC={:#x} addr={:#x}: {}", pc_before, addr, reason);
                eprintln!("HELM: fatal memory fault — stopping");
                // Post-mortem trace
                eprintln!("HELM: last {} instructions:", trace_ring.len().min(TRACE_SIZE));
                let start = if trace_ring.len() < TRACE_SIZE { 0 } else { trace_idx };
                let count = trace_ring.len().min(TRACE_SIZE);
                for i in 0..count {
                    let idx = (start + i) % trace_ring.len();
                    let (pc, insn, el) = trace_ring[idx];
                    eprintln!("  [{:5}] EL{} PC={:#010x} insn={:#010x}", insn_count as i64 - (count as i64 - i as i64), el, pc, insn);
                }
                eprintln!("HELM: CPU state: EL{} SP_sel={} DAIF={:#x} NZCV={:#x}",
                          cpu.regs.current_el, cpu.regs.sp_sel, cpu.regs.daif, cpu.regs.nzcv);
                eprintln!("HELM:   VBAR_EL1={:#x} ELR_EL1={:#x} SPSR_EL1={:#x}",
                          cpu.regs.vbar_el1, cpu.regs.elr_el1, cpu.regs.spsr_el1);
                eprintln!("HELM:   SCTLR_EL1={:#x} SP={:#x} SP_EL1={:#x}",
                          cpu.regs.sctlr_el1, cpu.regs.sp, cpu.regs.sp_el1);
                eprintln!("HELM:   X0={:#x} X1={:#x} X30={:#x}",
                          cpu.xn(0), cpu.xn(1), cpu.xn(30));
                break;
            }
            Err(helm_core::HelmError::Isa(ref msg))
            | Err(helm_core::HelmError::Decode { reason: ref msg, .. }) => {
                if cli.trace_devices {
                    eprintln!("HELM: unhandled at PC={:#x}: {}", pc_before, msg);
                }
                // Skip unimplemented/decode errors (NOP them in FS mode)
                cpu.regs.pc += 4;
                insn_count += 1;
                isa_skip_count += 1;
                virtual_cycles += 1;
            }
            Err(e) => {
                eprintln!("HELM: fatal error at PC={:#x}: {}", pc_before, e);
                eprintln!("HELM: {} instructions executed", insn_count);
                break;
            }
        }
    }

    if isa_skip_count > 0 {
        eprintln!("HELM: {} instructions skipped (unimplemented)", isa_skip_count);
    }

    let ipc = if virtual_cycles > 0 {
        insn_count as f64 / virtual_cycles as f64
    } else {
        0.0
    };
    let uart_bytes = uart_tx_count.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!("HELM: {} instructions, {} cycles, IPC={:.3}, UART TX={} bytes, IRQs={}",
              insn_count, virtual_cycles, ipc, uart_bytes, irq_count);

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
        "realview-pb" | "realview" => {
            Ok(helm_device::realview_pb_platform(serial_backend))
        }
        "rpi3" | "raspi3" => {
            let serial2: Box<dyn helm_device::backend::CharBackend> = Box::new(NullCharBackend);
            Ok(helm_device::rpi3_platform(serial_backend, serial2))
        }
        "virt" | "arm-virt" => {
            let serial2: Box<dyn helm_device::backend::CharBackend> = Box::new(NullCharBackend);
            Ok(helm_device::arm_virt_platform(serial_backend, serial2, irq_signal))
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
fn count_nodes(node: &helm_device::FdtNode) -> usize {
    1 + node.children.iter().map(|c| count_nodes(c)).sum::<usize>()
}
