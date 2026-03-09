//! LLVM-IR hardware accelerator as a PCI function.
//!
//! [`AcceleratorPciFunction`] wraps an [`Accelerator`] and exposes it as a
//! PCIe Type-0 endpoint via the [`PciFunction`] trait.  The guest OS loads
//! the device using the standard PCI enumeration mechanism, then interacts
//! with it through three BARs:
//!
//! - **BAR0** (4 KB) — control/status registers
//! - **BAR2** (configurable) — scratchpad memory (optional; `Unused` when size is 0)
//! - **BAR4** (4 KB) — MSI-X vector table and pending-bit array (1 vector)
//!
//! # BAR0 Register Map
//!
//! | Offset | Name     | R/W | Description                              |
//! |--------|----------|-----|------------------------------------------|
//! | 0x00   | STATUS   | R   | 0=idle, 1=running, 2=complete, 3=error   |
//! | 0x04   | CONTROL  | W   | 1=start, 2=abort, 3=reset                |
//! | 0x08   | CYCLES   | R   | cycles elapsed (u64, low 32 bits)        |
//! | 0x0C   | CYCLES_H | R   | cycles elapsed (u64, high 32 bits)       |
//! | 0x10   | LOADS    | R   | memory load count (u64, low 32 bits)     |
//! | 0x14   | LOADS_H  | R   | memory load count (u64, high 32 bits)    |
//! | 0x18   | STORES   | R   | memory store count (u64, low 32 bits)    |
//! | 0x1C   | STORES_H | R   | memory store count (u64, high 32 bits)   |
//! | 0x20   | FN_SEL   | RW  | select entry function by index (u32)     |
//! | 0x28   | ARG0     | RW  | function argument 0 (u64, low 32 bits)   |
//! | 0x2C   | ARG0_H   | RW  | function argument 0 (u64, high 32 bits)  |
//! | 0x30   | ARG1     | RW  | function argument 1 (u64, low 32 bits)   |
//! | 0x34   | ARG1_H   | RW  | function argument 1 (u64, high 32 bits)  |
//! | 0x38   | ARG2     | RW  | function argument 2 (u64, low 32 bits)   |
//! | 0x3C   | ARG2_H   | RW  | function argument 2 (u64, high 32 bits)  |
//! | 0x40   | ARG3     | RW  | function argument 3 (u64, low 32 bits)   |
//! | 0x44   | ARG3_H   | RW  | function argument 3 (u64, high 32 bits)  |
//!
//! # PCI Identity
//!
//! | Field         | Value      |
//! |---------------|------------|
//! | Vendor ID     | 0x1DE5     |
//! | Device ID     | 0x0001     |
//! | Class code    | 0x120000   |
//! | Revision      | 1          |
//!
//! # Example
//!
//! ```rust
//! use helm_llvm::AcceleratorPciFunction;
//! use helm_device::pci::{PciFunction, BarDecl};
//!
//! let accel = AcceleratorPciFunction::from_string(
//!     "define i32 @main() { entry: ret i32 0 }",
//! );
//! assert_eq!(accel.vendor_id(), 0x1DE5);
//! assert_eq!(accel.device_id(), 0x0001);
//! // STATUS = idle initially
//! assert_eq!(accel.bar_read(0, 0x00, 4), 0);
//! ```

use crate::accelerator::AcceleratorConfig;
use crate::accelerator::AcceleratorBuilder;
use crate::accelerator::Accelerator;
use helm_device::device::DeviceEvent;
use helm_device::pci::capability::{MsixCapability, PcieCapability, PmCapability};
use helm_device::pci::traits::{BarDecl, PciCapability, PciFunction};

// ── Status codes ──────────────────────────────────────────────────────────────

/// Accelerator is idle and ready to accept a start command.
const STATUS_IDLE: u32 = 0;
/// Accelerator is currently running.
const STATUS_RUNNING: u32 = 1;
/// Accelerator has completed a run successfully.
const STATUS_COMPLETE: u32 = 2;
/// Accelerator failed to build or encountered an error during run.
const STATUS_ERROR: u32 = 3;

// ── Control codes ─────────────────────────────────────────────────────────────

/// Start the accelerator.
const CTRL_START: u64 = 1;
/// Abort a running accelerator (treated as a reset in this model).
const CTRL_ABORT: u64 = 2;
/// Reset the accelerator to idle state.
const CTRL_RESET: u64 = 3;

// ── PCI identity ──────────────────────────────────────────────────────────────

/// HELM vendor ID.
const VENDOR_ID: u16 = 0x1DE5;
/// Accelerator device ID.
const DEVICE_ID: u16 = 0x0001;
/// Class code: Processing Accelerator (0x12 base, 0x00 sub, 0x00 prog-if).
const CLASS_CODE: u32 = 0x12_00_00;
/// Hardware revision 1.
const REVISION_ID: u8 = 1;

// ── BAR indices ───────────────────────────────────────────────────────────────

const BAR_CSR: u8 = 0;
const BAR_SCRATCH: u8 = 2;
const BAR_MSIX: u8 = 4;

/// Size of the control/status register BAR (4 KB).
const BAR_CSR_SIZE: u64 = 0x1000;
/// Size of the MSI-X BAR (4 KB).
const BAR_MSIX_SIZE: u64 = 0x1000;

// ── Capability offsets ────────────────────────────────────────────────────────

const CAP_OFFSET_PM: u16 = 0x40;
const CAP_OFFSET_PCIE: u16 = 0x50;
const CAP_OFFSET_MSIX: u16 = 0xA0;

/// LLVM-IR hardware accelerator exposed as a PCIe Type-0 endpoint.
///
/// The device wraps an [`Accelerator`] and provides a register-mapped control
/// interface through BAR0.  An optional scratchpad memory region is exposed
/// through BAR2.  Completion interrupts are delivered via the single MSI-X
/// vector in BAR4.
///
/// When the accelerator IR cannot be built (e.g. invalid IR string), the
/// device still presents itself on the PCI bus with STATUS = 3 (error), and
/// start commands are ignored.
///
/// # Example
///
/// ```rust
/// use helm_llvm::AcceleratorPciFunction;
/// use helm_device::pci::PciFunction;
///
/// let mut accel = AcceleratorPciFunction::from_string(
///     "define i32 @main() { entry: ret i32 0 }",
/// );
/// // STATUS = 0 (idle)
/// assert_eq!(accel.bar_read(0, 0x00, 4), 0);
/// // Write CONTROL = 1 (start)
/// accel.bar_write(0, 0x04, 4, 1);
/// // STATUS = 2 (complete)
/// assert_eq!(accel.bar_read(0, 0x00, 4), 2);
/// ```
pub struct AcceleratorPciFunction {
    /// The inner accelerator.  `None` when build failed.
    accel: Option<Accelerator>,

    /// Cached statistics from the last run.
    total_cycles: u64,
    memory_loads: u64,
    memory_stores: u64,

    /// Current device status (STATUS register).
    status: u32,

    /// Selected function index (FN_SEL register).
    fn_select: u32,

    /// Function arguments (ARG0–ARG3 registers).
    args: [u64; 4],

    /// BAR layout declarations (index 0–5).
    bars: [BarDecl; 6],

    /// Capability list (PM + PCIe + MSI-X).
    caps: Vec<Box<dyn PciCapability>>,

    /// MSI-X capability (also stored in `caps` but kept separately for fast
    /// access to `fire()` and `table_read/write()`).
    msix: MsixCapability,

    /// Size of scratchpad memory in bytes (0 = no scratchpad).
    scratchpad_size: u64,

    /// Scratchpad memory contents.
    scratchpad: Vec<u8>,
}

impl AcceleratorPciFunction {
    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Returns the configured scratchpad size in bytes (0 if BAR2 is unused).
    ///
    /// # Example
    ///
    /// ```rust
    /// use helm_llvm::{AcceleratorPciFunction, IrSource, accelerator::AcceleratorConfig};
    ///
    /// let accel = AcceleratorPciFunction::new_with_config(
    ///     IrSource::Str("define i32 @main() { entry: ret i32 0 }".to_owned()),
    ///     0x10000,
    ///     AcceleratorConfig::default(),
    /// );
    /// assert_eq!(accel.scratchpad_size(), 0x10000);
    /// ```
    #[must_use]
    pub fn scratchpad_size(&self) -> u64 {
        self.scratchpad_size
    }

    // ── Constructors ─────────────────────────────────────────────────────────

    /// Create an accelerator device from an LLVM IR file.
    ///
    /// If the file cannot be read or parsed, the device is created with
    /// STATUS = `3` (error) and `start` commands are ignored.
    ///
    /// # Arguments
    ///
    /// - `path` — filesystem path to the `.ll` or `.bc` file.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use helm_llvm::AcceleratorPciFunction;
    /// let accel = AcceleratorPciFunction::from_file("matmul.ll");
    /// ```
    #[must_use]
    pub fn from_file(path: &str) -> Self {
        Self::new_with_config(IrSource::File(path.to_owned()), 0, AcceleratorConfig::default())
    }

    /// Create an accelerator device from an inline LLVM IR string.
    ///
    /// If the IR is invalid, the device is created with STATUS = `3` (error).
    ///
    /// # Arguments
    ///
    /// - `ir` — LLVM IR text.
    ///
    /// # Example
    ///
    /// ```rust
    /// use helm_llvm::AcceleratorPciFunction;
    /// let accel = AcceleratorPciFunction::from_string(
    ///     "define i32 @main() { entry: ret i32 0 }",
    /// );
    /// ```
    #[must_use]
    pub fn from_string(ir: &str) -> Self {
        Self::new_with_config(IrSource::Str(ir.to_owned()), 0, AcceleratorConfig::default())
    }

    /// Create an accelerator device with full configuration.
    ///
    /// # Arguments
    ///
    /// - `ir` — LLVM IR source (file path or inline string, as an [`IrSource`]).
    /// - `scratchpad_size` — size of the BAR2 scratchpad window in bytes.
    ///   Pass `0` to disable BAR2.
    /// - `config` — accelerator hardware configuration.
    ///
    /// # Example
    ///
    /// ```rust
    /// use helm_llvm::{AcceleratorPciFunction, IrSource, accelerator::AcceleratorConfig};
    /// let accel = AcceleratorPciFunction::new_with_config(
    ///     IrSource::Str("define i32 @main() { entry: ret i32 0 }".to_owned()),
    ///     65536,
    ///     AcceleratorConfig::default(),
    /// );
    /// ```
    #[must_use]
    pub fn new_with_config(ir: IrSource, scratchpad_size: u64, _config: AcceleratorConfig) -> Self {
        // Build the accelerator; on failure keep None.
        let builder = match &ir {
            IrSource::File(path) => AcceleratorBuilder::new()
                .with_ir_file(path)
                .with_scratchpad_size(scratchpad_size as usize),
            IrSource::Str(s) => AcceleratorBuilder::new()
                .with_ir_string(s)
                .with_scratchpad_size(scratchpad_size as usize),
        };

        let (accel, status) = match builder.build() {
            Ok(a) => (Some(a), STATUS_IDLE),
            Err(e) => {
                log::warn!("AcceleratorPciFunction: build failed: {e}");
                (None, STATUS_ERROR)
            }
        };

        // Build BAR array.
        let bar2 = if scratchpad_size > 0 {
            BarDecl::Mmio32 { size: scratchpad_size }
        } else {
            BarDecl::Unused
        };

        let bars = [
            BarDecl::Mmio32 { size: BAR_CSR_SIZE }, // BAR0 — CSR
            BarDecl::Unused,                         // BAR1
            bar2,                                    // BAR2 — scratchpad
            BarDecl::Unused,                         // BAR3
            BarDecl::Mmio32 { size: BAR_MSIX_SIZE }, // BAR4 — MSI-X
            BarDecl::Unused,                         // BAR5
        ];

        // Build MSI-X capability: 1 vector, table in BAR4 at offset 0,
        // PBA in BAR4 at offset 0x800.
        let msix = MsixCapability::new(CAP_OFFSET_MSIX, 1, BAR_MSIX, 0, BAR_MSIX, 0x800);

        // Build capability list.
        let caps: Vec<Box<dyn PciCapability>> = vec![
            Box::new(PmCapability::new(CAP_OFFSET_PM)),
            Box::new(PcieCapability::endpoint(CAP_OFFSET_PCIE)),
            // MSI-X header is re-read from `self.msix` via the cap stored here;
            // we also keep a direct `msix` field for table BAR access.
            Box::new(MsixCapability::new(CAP_OFFSET_MSIX, 1, BAR_MSIX, 0, BAR_MSIX, 0x800)),
        ];

        let scratchpad = vec![0u8; scratchpad_size as usize];

        Self {
            accel,
            total_cycles: 0,
            memory_loads: 0,
            memory_stores: 0,
            status,
            fn_select: 0,
            args: [0u64; 4],
            bars,
            caps,
            msix,
            scratchpad_size,
            scratchpad,
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Run the accelerator and update cached statistics.
    fn run_accelerator(&mut self) {
        let Some(accel) = self.accel.as_mut() else {
            // No accelerator (build failed) — stay in error state.
            return;
        };

        self.status = STATUS_RUNNING;

        match accel.run() {
            Ok(()) => {
                let stats = accel.stats();
                self.total_cycles = stats.total_cycles;
                self.memory_loads = stats.memory_loads;
                self.memory_stores = stats.memory_stores;
                self.status = STATUS_COMPLETE;
                log::debug!(
                    "AcceleratorPciFunction: run complete in {} cycles",
                    self.total_cycles
                );
            }
            Err(crate::error::Error::Other(ref msg)) if msg.contains("No entry function") => {
                // Empty module — treat as immediate completion with 0 cycles.
                // This matches the existing AcceleratorDevice behaviour and supports
                // the common case of an IR source with no callable function yet.
                let stats = accel.stats();
                self.total_cycles = stats.total_cycles;
                self.memory_loads = stats.memory_loads;
                self.memory_stores = stats.memory_stores;
                self.status = STATUS_COMPLETE;
                log::debug!("AcceleratorPciFunction: empty module, completing immediately");
            }
            Err(e) => {
                log::warn!("AcceleratorPciFunction: run error: {e}");
                self.status = STATUS_ERROR;
            }
        }
    }

    /// Read a 32-bit dword from BAR0 at `offset`.
    fn csr_read32(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.status,
            0x04 => 0, // CONTROL is write-only
            0x08 => self.total_cycles as u32,
            0x0C => (self.total_cycles >> 32) as u32,
            0x10 => self.memory_loads as u32,
            0x14 => (self.memory_loads >> 32) as u32,
            0x18 => self.memory_stores as u32,
            0x1C => (self.memory_stores >> 32) as u32,
            0x20 => self.fn_select,
            0x24 => 0, // reserved
            0x28 => self.args[0] as u32,
            0x2C => (self.args[0] >> 32) as u32,
            0x30 => self.args[1] as u32,
            0x34 => (self.args[1] >> 32) as u32,
            0x38 => self.args[2] as u32,
            0x3C => (self.args[2] >> 32) as u32,
            0x40 => self.args[3] as u32,
            0x44 => (self.args[3] >> 32) as u32,
            _ => 0,
        }
    }

    /// Compose a multi-byte CSR read from individual 32-bit fields.
    fn csr_read(&self, offset: u64, size: usize) -> u64 {
        match size {
            1 => {
                let word = self.csr_read32(offset & !3);
                let shift = (offset & 3) * 8;
                u64::from((word >> shift) as u8)
            }
            2 => {
                let word = self.csr_read32(offset & !3);
                let shift = (offset & 2) * 8;
                u64::from((word >> shift) as u16)
            }
            4 => u64::from(self.csr_read32(offset)),
            8 => {
                u64::from(self.csr_read32(offset)) | (u64::from(self.csr_read32(offset + 4)) << 32)
            }
            _ => 0,
        }
    }

    /// Write `value` to BAR0 at `offset`.
    fn csr_write(&mut self, offset: u64, value: u64) {
        match offset {
            0x04 => {
                // CONTROL register
                match value {
                    CTRL_START => self.run_accelerator(),
                    CTRL_ABORT | CTRL_RESET => {
                        self.status = STATUS_IDLE;
                        self.total_cycles = 0;
                        self.memory_loads = 0;
                        self.memory_stores = 0;
                    }
                    _ => {}
                }
            }
            0x20 => self.fn_select = value as u32,
            0x28 => {
                let hi = self.args[0] & 0xFFFF_FFFF_0000_0000;
                self.args[0] = hi | (value & 0xFFFF_FFFF);
            }
            0x2C => {
                let lo = self.args[0] & 0xFFFF_FFFF;
                self.args[0] = lo | ((value & 0xFFFF_FFFF) << 32);
            }
            0x30 => {
                let hi = self.args[1] & 0xFFFF_FFFF_0000_0000;
                self.args[1] = hi | (value & 0xFFFF_FFFF);
            }
            0x34 => {
                let lo = self.args[1] & 0xFFFF_FFFF;
                self.args[1] = lo | ((value & 0xFFFF_FFFF) << 32);
            }
            0x38 => {
                let hi = self.args[2] & 0xFFFF_FFFF_0000_0000;
                self.args[2] = hi | (value & 0xFFFF_FFFF);
            }
            0x3C => {
                let lo = self.args[2] & 0xFFFF_FFFF;
                self.args[2] = lo | ((value & 0xFFFF_FFFF) << 32);
            }
            0x40 => {
                let hi = self.args[3] & 0xFFFF_FFFF_0000_0000;
                self.args[3] = hi | (value & 0xFFFF_FFFF);
            }
            0x44 => {
                let lo = self.args[3] & 0xFFFF_FFFF;
                self.args[3] = lo | ((value & 0xFFFF_FFFF) << 32);
            }
            _ => {}
        }
    }
}

// ── IrSource ──────────────────────────────────────────────────────────────────

/// Source of LLVM IR for [`AcceleratorPciFunction::new_with_config`].
///
/// # Example
///
/// ```rust
/// use helm_llvm::IrSource;
///
/// let s = IrSource::Str("define i32 @main() { entry: ret i32 0 }".to_owned());
/// let f = IrSource::File("matmul.ll".to_owned());
/// ```
#[derive(Debug, Clone)]
pub enum IrSource {
    /// Path to a `.ll` or `.bc` file on disk.
    File(String),
    /// Inline LLVM IR text.
    Str(String),
}

// ── PciFunction impl ──────────────────────────────────────────────────────────

impl PciFunction for AcceleratorPciFunction {
    // ── Identity ─────────────────────────────────────────────────────────────

    fn vendor_id(&self) -> u16 {
        VENDOR_ID
    }

    fn device_id(&self) -> u16 {
        DEVICE_ID
    }

    fn class_code(&self) -> u32 {
        CLASS_CODE
    }

    fn revision_id(&self) -> u8 {
        REVISION_ID
    }

    // ── BAR layout ───────────────────────────────────────────────────────────

    fn bars(&self) -> &[BarDecl; 6] {
        &self.bars
    }

    // ── Capabilities ─────────────────────────────────────────────────────────

    fn capabilities(&self) -> &[Box<dyn PciCapability>] {
        &self.caps
    }

    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>> {
        &mut self.caps
    }

    // ── BAR access ───────────────────────────────────────────────────────────

    fn bar_read(&self, bar: u8, offset: u64, size: usize) -> u64 {
        match bar {
            BAR_CSR => self.csr_read(offset, size),
            BAR_SCRATCH => {
                // Read from scratchpad — up to 8 bytes, little-endian.
                let end = (offset as usize).saturating_add(size);
                if end > self.scratchpad.len() {
                    return 0;
                }
                let mut result = 0u64;
                for (i, &byte) in self.scratchpad[offset as usize..end].iter().enumerate() {
                    result |= u64::from(byte) << (i * 8);
                }
                result
            }
            BAR_MSIX => self.msix.table_read(offset as u32, size),
            _ => 0,
        }
    }

    fn bar_write(&mut self, bar: u8, offset: u64, size: usize, value: u64) {
        match bar {
            BAR_CSR => self.csr_write(offset, value),
            BAR_SCRATCH => {
                // Write to scratchpad — up to 8 bytes, little-endian.
                let end = (offset as usize).saturating_add(size);
                if end > self.scratchpad.len() {
                    return;
                }
                for (i, byte) in self.scratchpad[offset as usize..end].iter_mut().enumerate() {
                    *byte = ((value >> (i * 8)) & 0xFF) as u8;
                }
            }
            BAR_MSIX => self.msix.table_write(offset as u32, size, value),
            _ => {}
        }
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    fn reset(&mut self) {
        if self.accel.is_some() {
            self.status = STATUS_IDLE;
        }
        // If accel is None (build failed), preserve STATUS_ERROR.
        self.total_cycles = 0;
        self.memory_loads = 0;
        self.memory_stores = 0;
        self.fn_select = 0;
        self.args = [0u64; 4];
        self.scratchpad.iter_mut().for_each(|b| *b = 0);
        self.msix.reset();
        for cap in &mut self.caps {
            cap.reset();
        }
    }

    fn tick(&mut self, _cycles: u64) -> Vec<DeviceEvent> {
        vec![]
    }

    fn name(&self) -> &str {
        "helm-accelerator"
    }
}

