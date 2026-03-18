//! ARM virt platform — QEMU-compatible address map and device wiring.
//!
//! Devices:
//! - GICv2 Distributor at 0x0800_0000  (4 KiB)
//! - GICv2 CPU Interface at 0x0801_0000 (4 KiB)
//! - PL011 UART at 0x0900_0000         (4 KiB)
//! - RAM at 0x4000_0000
//!
//! The GIC distributor and CPU interface share state via `Arc<Mutex<>>` and
//! a single `Arc<AtomicBool>` IRQ line that the FS step loop polls each step.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use helm_arch::Aarch64ArchState;
use helm_devices::CharBackend;
#[cfg(test)] use helm_devices::NullCharBackend;
use helm_hw_char::Pl011;
use helm_hw_intc::build_gicv2;

use crate::fs::FsState;
use crate::loader::load_arm64_kernel;
use crate::system_mem::SystemMem;
use crate::FlatMem;

// ── Address map (QEMU virt compatible) ───────────────────────────────────────

/// GIC Distributor base address.
pub const GICD_BASE: u64 = 0x0800_0000;
/// GIC CPU Interface base address.
pub const GICC_BASE: u64 = 0x0801_0000;
/// PL011 UART base address.
pub const UART_BASE: u64 = 0x0900_0000;
/// RAM base address.
pub const RAM_BASE: u64 = 0x4000_0000;

// ── Device index bookkeeping ──────────────────────────────────────────────────

/// Device indices in SystemMem.devices.
pub struct ArmVirtDevices {
    pub gicd_idx: usize,
    pub gicc_idx: usize,
    pub uart_idx: usize,
}

// ── build_arm_virt ────────────────────────────────────────────────────────────

/// Build the ARM virt platform.
///
/// Returns `(sys_mem, device_indices, fs_state, irq_line)`.
/// The `irq_line` must be stored in `Aarch64FsMachine` and polled before each
/// FS step so the CPU receives interrupts from the GIC.
pub fn build_arm_virt(
    mem_mib: usize,
    uart_backend: Box<dyn CharBackend>,
) -> (SystemMem, ArmVirtDevices, FsState, Arc<AtomicBool>, Arc<std::sync::Mutex<helm_hw_intc::GicState>>) {
    let ram = FlatMem::new(RAM_BASE, mem_mib * 1024 * 1024);
    let mut sys_mem = SystemMem::new(ram);

    // GICv2: distributor + CPU interface share state; irq_line goes to the CPU,
    // gic_state allows the step loop to assert device/timer IRQs.
    let (gicd, gicc, irq_line, gic_state) = build_gicv2(128);
    let gicd_idx = sys_mem.add_device(GICD_BASE, Box::new(gicd));
    let gicc_idx = sys_mem.add_device(GICC_BASE, Box::new(gicc));

    // PL011 UART — wire irq_out to GIC SPI 1 (INTID 33) before boxing.
    //
    // The arm-virt DTB declares: interrupts = <0 1 4>
    //   type=0 (SPI), SPI number=1 -> INTID = 32 + 1 = 33.
    // Wiring happens here, at platform construction time, so the device is
    // fully connected before any MMIO access can trigger update_irq().
    let mut uart = Pl011::new(uart_backend);
    {
        use helm_hw_intc::GicSink;
        use helm_devices::WireId;
        let sink = std::sync::Arc::new(GicSink::new(Arc::clone(&gic_state), 33));
        uart.irq_out.wire(WireId::from(33u32), sink);
    }
    let uart_idx = sys_mem.add_device(UART_BASE, Box::new(uart));

    let devs = ArmVirtDevices { gicd_idx, gicc_idx, uart_idx };
    let fs = FsState::new();

    (sys_mem, devs, fs, irq_line, gic_state)
}

// ── setup_arm_virt_boot ───────────────────────────────────────────────────────

/// Load a kernel and set up AArch64 state for FS boot on the ARM virt platform.
///
/// Returns `(arch_state, sys_mem, fs_state, devices, irq_line)`.
pub fn setup_arm_virt_boot(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem_mib: usize,
    uart_backend: Box<dyn CharBackend>,
) -> Result<(Aarch64ArchState, SystemMem, FsState, ArmVirtDevices, Arc<AtomicBool>, Arc<std::sync::Mutex<helm_hw_intc::GicState>>), String> {
    let (mut sys_mem, devs, fs, irq_line, gic_state) = build_arm_virt(mem_mib, uart_backend);

    // Load kernel, DTB, initramfs into RAM; optionally override bootargs.
    let loaded = load_arm64_kernel(
        kernel_path, dtb_path, initrd_path, append, &mut sys_mem.ram, RAM_BASE,
    )?;

    // AArch64 boot-protocol register setup
    let mut a64 = Aarch64ArchState::new();
    a64.current_el = 1;
    a64.spsel = true;
    a64.pc = loaded.entry;
    a64.x[0] = loaded.dtb_addr; // x0 = DTB PA
    a64.x[1] = 0;
    a64.x[2] = 0;
    a64.x[3] = 0;
    a64.sp_el1 = RAM_BASE + (mem_mib as u64 * 1024 * 1024) - 0x1000;
    a64.sctlr_el1 = 0x0000_0800; // RES1 only — MMU off
    a64.daif = 0xF;              // all interrupts masked initially

    Ok((a64, sys_mem, fs, devs, irq_line, gic_state))
}

// ── StdioCharBackend ──────────────────────────────────────────────────────────

/// Character backend that writes guest UART output to host stdout.
pub struct StdioCharBackend;

impl CharBackend for StdioCharBackend {
    fn write(&mut self, data: &[u8]) -> usize {
        use std::io::Write;
        let _ = std::io::stdout().write_all(data);
        let _ = std::io::stdout().flush();
        data.len()
    }
    fn read(&mut self) -> Option<u8> { None }
    fn can_write(&self) -> bool { true }
    fn can_read(&self) -> bool { false }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use helm_devices::NullCharBackend;

    #[test]
    fn build_arm_virt_creates_devices() {
        let (sys_mem, devs, _fs, _irq, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        assert_eq!(devs.gicd_idx, 0);
        assert_eq!(devs.gicc_idx, 1);
        assert_eq!(devs.uart_idx, 2);
        assert_eq!(sys_mem.devices.len(), 3);
    }

    #[test]
    fn uart_at_correct_address() {
        let (mut sys_mem, _devs, _fs, _irq, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        use helm_core::{AccessType, MemInterface};
        // UARTFR at offset 0x18 — TXFE and RXFE should be set on reset
        let val = sys_mem.read(UART_BASE + 0x18, 4, AccessType::Load).unwrap();
        assert_ne!(val & 0x90, 0);
    }

    #[test]
    fn uart_irq_out_is_wired_to_gic() {
        // Confirm that the PL011's irq_out is connected; a TX write must NOT
        // produce the "unconnected pin" warning. Instead it should call the
        // GicSink, which asserts/deasserts INTID 33 on the shared GicState.
        use helm_core::{AccessType, MemInterface};
        let (mut sys_mem, devs, _fs, irq, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        // Enable GICD + GICC + unmask UART SPI 1 (INTID 33)
        sys_mem.write(GICD_BASE, 4, 1, AccessType::Store).unwrap();
        sys_mem.write(GICD_BASE + 0x104, 4, 0x2, AccessType::Store).unwrap(); // ISENABLER1 bit1=INTID33
        sys_mem.write(GICC_BASE, 4, 1, AccessType::Store).unwrap();
        sys_mem.write(GICC_BASE + 0x4, 4, 0xFF, AccessType::Store).unwrap(); // PMR=all
        // Enable UART TX interrupt in IMSC
        sys_mem.write(UART_BASE + 0x3C, 4, 1 << 5, AccessType::Store).unwrap(); // UARTIMSC (0x03C) TX bit
        // Write a byte to UART DR — triggers ris|=INT_TX -> update_irq -> assert
        sys_mem.write(UART_BASE, 4, b'A' as u64, AccessType::Store).unwrap();
        // GIC irq_line should now be raised
        use std::sync::atomic::Ordering;
        assert!(irq.load(Ordering::Relaxed),
            "UART TX interrupt must propagate through GicSink -> GIC -> irq_line");
        let _ = devs.uart_idx;
    }

    #[test]
    fn gic_irq_line_not_raised_on_reset() {
        use std::sync::atomic::Ordering;
        let (_sys_mem, _devs, _fs, irq, _gic) = build_arm_virt(256, Box::new(NullCharBackend));
        assert!(!irq.load(Ordering::Relaxed), "IRQ line should be low on reset");
    }

    #[test]
    fn gic_irq_line_raised_when_enabled_and_pending() {
        use std::sync::atomic::Ordering;
        use helm_core::{AccessType, MemInterface};
        let (mut sys_mem, devs, _fs, irq, _gic) = build_arm_virt(256, Box::new(NullCharBackend));

        // Enable GICD (GICD_CTLR = 1)
        sys_mem.write(GICD_BASE, 4, 1, AccessType::Store).unwrap();
        // Enable IRQ 32 (GICD_ISENABLER1 bit 0)
        sys_mem.write(GICD_BASE + 0x104, 4, 0x1, AccessType::Store).unwrap();
        // Set pending IRQ 32 (GICD_ISPENDR1 bit 0)
        sys_mem.write(GICD_BASE + 0x204, 4, 0x1, AccessType::Store).unwrap();
        // Enable GICC (GICC_CTLR = 1)
        sys_mem.write(GICC_BASE, 4, 1, AccessType::Store).unwrap();
        // Set GICC PMR = 0xFF (allow all)
        sys_mem.write(GICC_BASE + 0x4, 4, 0xFF, AccessType::Store).unwrap();

        assert!(irq.load(Ordering::Relaxed), "IRQ line should be raised");

        // ACK via GICC_IAR
        let iar = sys_mem.read(GICC_BASE + 0xC, 4, AccessType::Load).unwrap();
        assert_eq!(iar, 32, "IAR should return IRQ 32");
        assert!(!irq.load(Ordering::Relaxed), "IRQ line should drop after ACK");

        // EOI
        sys_mem.write(GICC_BASE + 0x10, 4, 32, AccessType::Store).unwrap();
        let _ = devs.gicc_idx; // suppress unused warning
    }
}
