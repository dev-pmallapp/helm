//! ARM virt platform — QEMU-compatible address map and device wiring.
//!
//! This platform creates a SystemMem with:
//! - PL011 UART at 0x0900_0000
//! - GICv2 Distributor at 0x0800_0000
//! - GICv2 CPU Interface at 0x0801_0000
//! - RAM at 0x4000_0000 (1 GiB default)

use helm_arch::Aarch64ArchState;
use helm_devices::{CharBackend, NullCharBackend};
use helm_hw_char::Pl011;
use helm_hw_intc::{Gicv2CpuInterface, Gicv2Distributor};

use crate::fs::FsState;
use crate::loader::{load_arm64_kernel, LoadedKernel};
use crate::system_mem::SystemMem;
use crate::FlatMem;

// Address constants (QEMU virt compatible).
/// GIC Distributor base address.
pub const GICD_BASE: u64 = 0x0800_0000;
/// GIC CPU Interface base address.
pub const GICC_BASE: u64 = 0x0801_0000;
/// PL011 UART base address.
pub const UART_BASE: u64 = 0x0900_0000;
/// RAM base address.
pub const RAM_BASE: u64 = 0x4000_0000;

/// Device indices in SystemMem.
pub struct ArmVirtDevices {
    /// Index of GIC Distributor in SystemMem.devices.
    pub gicd_idx: usize,
    /// Index of GIC CPU Interface.
    pub gicc_idx: usize,
    /// Index of PL011 UART.
    pub uart_idx: usize,
}

/// Build the ARM virt platform.
///
/// Creates SystemMem with all devices mapped and wired.
/// Returns the SystemMem, device indices, and FsState.
pub fn build_arm_virt(
    mem_mib: usize,
    uart_backend: Box<dyn CharBackend>,
) -> (SystemMem, ArmVirtDevices, FsState) {
    let ram = FlatMem::new(RAM_BASE, mem_mib * 1024 * 1024);
    let mut sys_mem = SystemMem::new(ram);

    // Create GIC distributor (128 SPI lines)
    let gicd = Gicv2Distributor::new(128);
    let gicd_idx = sys_mem.add_device(GICD_BASE, Box::new(gicd));

    // Create GIC CPU interface
    let gicc = Gicv2CpuInterface::new();
    let gicc_idx = sys_mem.add_device(GICC_BASE, Box::new(gicc));

    // Create PL011 UART with provided backend
    let uart = Pl011::new(uart_backend);
    let uart_idx = sys_mem.add_device(UART_BASE, Box::new(uart));

    let devs = ArmVirtDevices {
        gicd_idx,
        gicc_idx,
        uart_idx,
    };

    let fs = FsState::new();

    (sys_mem, devs, fs)
}

/// Load a kernel and set up the AArch64 state for FS boot.
///
/// Returns a fully configured (ArchState, SystemMem, FsState, ArmVirtDevices).
pub fn setup_arm_virt_boot(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    mem_mib: usize,
    uart_backend: Box<dyn CharBackend>,
) -> Result<(Aarch64ArchState, SystemMem, FsState, ArmVirtDevices), String> {
    let (mut sys_mem, devs, fs) = build_arm_virt(mem_mib, uart_backend);

    // Load kernel, DTB, initramfs into RAM
    let loaded = load_arm64_kernel(kernel_path, dtb_path, initrd_path, &mut sys_mem.ram, RAM_BASE)?;

    // Set up AArch64 state for kernel entry
    let mut a64 = Aarch64ArchState::new();
    a64.current_el = 1;
    a64.spsel = true;
    a64.pc = loaded.entry;
    a64.x[0] = loaded.dtb_addr; // x0 = DTB address (ARM64 boot protocol)
    a64.x[1] = 0; // Reserved
    a64.x[2] = 0; // Reserved
    a64.x[3] = 0; // Reserved

    // Set up initial SP_EL1 (top of RAM)
    a64.sp_el1 = RAM_BASE + (mem_mib as u64 * 1024 * 1024) - 0x1000;

    // MMU off initially (kernel enables it)
    a64.sctlr_el1 = 0x0000_0800; // RES1 bits only, MMU disabled

    // DAIF fully masked (kernel will unmask as needed)
    a64.daif = 0xF;

    Ok((a64, sys_mem, fs, devs))
}

/// Stdio character backend — writes to stdout, no input.
pub struct StdioCharBackend;

impl CharBackend for StdioCharBackend {
    fn write(&mut self, data: &[u8]) -> usize {
        use std::io::Write;
        let _ = std::io::stdout().write_all(data);
        let _ = std::io::stdout().flush();
        data.len()
    }

    fn read(&mut self) -> Option<u8> {
        None // No input support for now
    }

    fn can_write(&self) -> bool {
        true
    }

    fn can_read(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_devices::NullCharBackend;

    #[test]
    fn build_arm_virt_creates_devices() {
        let (sys_mem, devs, _fs) = build_arm_virt(256, Box::new(NullCharBackend));
        assert_eq!(devs.gicd_idx, 0);
        assert_eq!(devs.gicc_idx, 1);
        assert_eq!(devs.uart_idx, 2);
        assert_eq!(sys_mem.devices.len(), 3);
    }

    #[test]
    fn uart_at_correct_address() {
        let (mut sys_mem, _devs, _fs) = build_arm_virt(256, Box::new(NullCharBackend));
        // Read PL011 UARTFR (flag register) at offset 0x18
        use helm_core::{AccessType, MemInterface};
        let val = sys_mem.read(UART_BASE + 0x18, 4, AccessType::Load).unwrap();
        // UARTFR should have TXFE (TX FIFO empty) set (bit 7) and RXFE (RX FIFO empty, bit 4)
        assert_ne!(val & 0x90, 0); // At least TXFE should be set
    }
}
