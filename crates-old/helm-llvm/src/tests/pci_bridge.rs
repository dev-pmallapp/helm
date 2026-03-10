//! Tests for [`AcceleratorPciFunction`].
//!
//! NOTE: The helm-llvm parser is a simplified parser that skips unknown IR
//! constructs.  Empty IR `""` is the only input that reliably produces a
//! working accelerator (empty module → immediate completion with 0 cycles).
//! Tests that require STATUS_ERROR use a non-existent file path, which
//! causes `LLVMModule::from_file` to fail with an IO error.

use crate::accelerator::AcceleratorConfig;
use crate::pci_bridge::{AcceleratorPciFunction, IrSource};
use helm_device::pci::{BarDecl, PciFunction};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a working accelerator using empty IR.
///
/// The parser handles empty input gracefully — it produces an empty module.
/// When started, the accelerator completes immediately with 0 cycles (no
/// entry function found → treated as immediate completion).
fn make_accel() -> AcceleratorPciFunction {
    // Empty IR: the parser returns an empty module, build() succeeds.
    AcceleratorPciFunction::from_string("")
}

fn make_accel_with_scratch(size: u64) -> AcceleratorPciFunction {
    AcceleratorPciFunction::new_with_config(
        IrSource::Str(String::new()),
        size,
        AcceleratorConfig::default(),
    )
}

/// Build an error accelerator by pointing at a nonexistent file.
///
/// `from_file` uses `std::fs::read_to_string`; a missing file produces an
/// `IoError` from `build()`, so `accel` is set to `None` and STATUS = 3.
fn make_error_accel() -> AcceleratorPciFunction {
    AcceleratorPciFunction::from_file("/nonexistent/__helm_test_file_that_must_not_exist__.ll")
}

// ── PCI identity ──────────────────────────────────────────────────────────────

#[test]
fn vendor_id_is_helm() {
    assert_eq!(make_accel().vendor_id(), 0x1DE5);
}

#[test]
fn device_id_is_0001() {
    assert_eq!(make_accel().device_id(), 0x0001);
}

#[test]
fn class_code_is_processing_accelerator() {
    // Processing Accelerator: base=0x12, sub=0x00, prog-if=0x00
    assert_eq!(make_accel().class_code(), 0x12_00_00);
}

#[test]
fn revision_id_is_1() {
    assert_eq!(make_accel().revision_id(), 1);
}

#[test]
fn name_is_helm_accelerator() {
    assert_eq!(PciFunction::name(&make_accel()), "helm-accelerator");
}

// ── BAR layout ───────────────────────────────────────────────────────────────

#[test]
fn bar0_is_mmio32_4k() {
    let a = make_accel();
    assert_eq!(a.bars()[0], BarDecl::Mmio32 { size: 0x1000 });
}

#[test]
fn bar1_is_unused() {
    assert_eq!(make_accel().bars()[1], BarDecl::Unused);
}

#[test]
fn bar2_unused_when_no_scratchpad() {
    assert_eq!(make_accel().bars()[2], BarDecl::Unused);
}

#[test]
fn bar2_mmio32_when_scratchpad_configured() {
    let a = make_accel_with_scratch(0x10000);
    assert_eq!(a.bars()[2], BarDecl::Mmio32 { size: 0x10000 });
}

#[test]
fn bar3_is_unused() {
    assert_eq!(make_accel().bars()[3], BarDecl::Unused);
}

#[test]
fn bar4_is_mmio32_4k_for_msix() {
    assert_eq!(make_accel().bars()[4], BarDecl::Mmio32 { size: 0x1000 });
}

#[test]
fn bar5_is_unused() {
    assert_eq!(make_accel().bars()[5], BarDecl::Unused);
}

// ── Capabilities ─────────────────────────────────────────────────────────────

#[test]
fn has_pm_capability() {
    let a = make_accel();
    let has_pm = a.capabilities().iter().any(|c| c.cap_id() == 0x01);
    assert!(has_pm, "must have PM capability (0x01)");
}

#[test]
fn has_pcie_capability() {
    let a = make_accel();
    let has_pcie = a.capabilities().iter().any(|c| c.cap_id() == 0x10);
    assert!(has_pcie, "must have PCIe endpoint capability (0x10)");
}

#[test]
fn has_msix_capability() {
    let a = make_accel();
    let has_msix = a.capabilities().iter().any(|c| c.cap_id() == 0x11);
    assert!(has_msix, "must have MSI-X capability (0x11)");
}

// ── STATUS register ───────────────────────────────────────────────────────────

#[test]
fn status_idle_initially() {
    let a = make_accel();
    assert_eq!(a.bar_read(0, 0x00, 4), 0, "STATUS should be 0 (idle)");
}

#[test]
fn status_reads_correct_size_1() {
    let a = make_accel();
    assert_eq!(a.bar_read(0, 0x00, 1), 0);
}

#[test]
fn status_reads_correct_size_2() {
    let a = make_accel();
    assert_eq!(a.bar_read(0, 0x00, 2), 0);
}

// ── CONTROL write — start ─────────────────────────────────────────────────────

#[test]
fn control_start_sets_status_complete() {
    let mut a = make_accel();
    a.bar_write(0, 0x04, 4, 1); // CTRL_START
    let status = a.bar_read(0, 0x00, 4);
    // Empty IR → immediate completion (STATUS_COMPLETE = 2).
    assert_eq!(status, 2, "STATUS should be 2 (complete) after start");
}

#[test]
fn cycles_after_run_are_readable() {
    // With empty IR, cycles = 0 (no instructions scheduled).
    // We only verify the read does not panic and returns a consistent value.
    let mut a = make_accel();
    a.bar_write(0, 0x04, 4, 1); // run
    let lo = a.bar_read(0, 0x08, 4);
    let hi = a.bar_read(0, 0x0C, 4);
    // Verify we can reassemble the 64-bit value without panic.
    let _cycles = lo | (hi << 32);
    // With empty IR, the scheduler runs 0 instructions → 0 cycles.
    assert_eq!(_cycles, 0);
}

// ── CONTROL write — reset ─────────────────────────────────────────────────────

#[test]
fn control_reset_returns_to_idle() {
    let mut a = make_accel();
    a.bar_write(0, 0x04, 4, 1); // start
    assert_eq!(a.bar_read(0, 0x00, 4), 2);
    a.bar_write(0, 0x04, 4, 3); // reset
    assert_eq!(
        a.bar_read(0, 0x00, 4),
        0,
        "STATUS should be 0 (idle) after reset"
    );
}

#[test]
fn control_abort_returns_to_idle() {
    let mut a = make_accel();
    a.bar_write(0, 0x04, 4, 1); // start
    a.bar_write(0, 0x04, 4, 2); // abort
    assert_eq!(
        a.bar_read(0, 0x00, 4),
        0,
        "STATUS should be 0 (idle) after abort"
    );
}

#[test]
fn reset_clears_cycles() {
    let mut a = make_accel();
    a.bar_write(0, 0x04, 4, 1); // run
    a.bar_write(0, 0x04, 4, 3); // reset
    let cycles = a.bar_read(0, 0x08, 4) | (a.bar_read(0, 0x0C, 4) << 32);
    assert_eq!(cycles, 0, "cycles should be cleared after reset");
}

// ── FN_SEL register ───────────────────────────────────────────────────────────

#[test]
fn fn_sel_default_zero() {
    assert_eq!(make_accel().bar_read(0, 0x20, 4), 0);
}

#[test]
fn fn_sel_write_read_roundtrip() {
    let mut a = make_accel();
    a.bar_write(0, 0x20, 4, 7);
    assert_eq!(a.bar_read(0, 0x20, 4), 7);
}

// ── ARG registers ─────────────────────────────────────────────────────────────

#[test]
fn arg0_default_zero() {
    let a = make_accel();
    let lo = a.bar_read(0, 0x28, 4);
    let hi = a.bar_read(0, 0x2C, 4);
    assert_eq!(lo | (hi << 32), 0);
}

#[test]
fn arg0_write_read_low() {
    let mut a = make_accel();
    a.bar_write(0, 0x28, 4, 0xDEAD_BEEF);
    assert_eq!(a.bar_read(0, 0x28, 4), 0xDEAD_BEEF);
}

#[test]
fn arg0_write_read_high() {
    let mut a = make_accel();
    a.bar_write(0, 0x2C, 4, 0xCAFE_BABE);
    assert_eq!(a.bar_read(0, 0x2C, 4), 0xCAFE_BABE);
}

#[test]
fn arg1_write_read_roundtrip() {
    let mut a = make_accel();
    a.bar_write(0, 0x30, 4, 0x1234_5678);
    assert_eq!(a.bar_read(0, 0x30, 4), 0x1234_5678);
}

#[test]
fn arg2_write_read_roundtrip() {
    let mut a = make_accel();
    a.bar_write(0, 0x38, 4, 0xABCD_EF01);
    assert_eq!(a.bar_read(0, 0x38, 4), 0xABCD_EF01);
}

#[test]
fn arg3_write_read_roundtrip() {
    let mut a = make_accel();
    a.bar_write(0, 0x40, 4, 0x9999_1111);
    assert_eq!(a.bar_read(0, 0x40, 4), 0x9999_1111);
}

// ── Scratchpad (BAR2) ─────────────────────────────────────────────────────────

#[test]
fn scratchpad_initial_zero() {
    let a = make_accel_with_scratch(64);
    assert_eq!(a.bar_read(2, 0, 4), 0);
}

#[test]
fn scratchpad_write_read_byte() {
    let mut a = make_accel_with_scratch(64);
    a.bar_write(2, 3, 1, 0xAB);
    assert_eq!(a.bar_read(2, 3, 1), 0xAB);
}

#[test]
fn scratchpad_write_read_dword() {
    let mut a = make_accel_with_scratch(64);
    a.bar_write(2, 0, 4, 0xDEAD_C0DE);
    assert_eq!(a.bar_read(2, 0, 4), 0xDEAD_C0DE);
}

#[test]
fn scratchpad_write_read_qword() {
    let mut a = make_accel_with_scratch(64);
    a.bar_write(2, 8, 8, 0x0102_0304_0506_0708);
    assert_eq!(a.bar_read(2, 8, 8), 0x0102_0304_0506_0708);
}

#[test]
fn scratchpad_out_of_range_reads_zero() {
    let a = make_accel_with_scratch(64);
    // Read beyond end of scratchpad — should return 0, not panic.
    assert_eq!(a.bar_read(2, 100, 4), 0);
}

#[test]
fn scratchpad_size_accessor() {
    let a = make_accel_with_scratch(0x4000);
    assert_eq!(a.scratchpad_size(), 0x4000);
}

// ── Error accelerator (nonexistent file) ───────────────────────────────────────

#[test]
fn nonexistent_file_status_is_error() {
    let a = make_error_accel();
    assert_eq!(
        a.bar_read(0, 0x00, 4),
        3,
        "STATUS should be 3 (error) for nonexistent file"
    );
}

#[test]
fn error_accel_start_is_noop() {
    let mut a = make_error_accel();
    a.bar_write(0, 0x04, 4, 1); // CTRL_START — should be no-op
                                // Status must remain error (3), not transition to running or complete.
    assert_eq!(a.bar_read(0, 0x00, 4), 3);
}

// ── PciFunction::reset ────────────────────────────────────────────────────────

#[test]
fn pci_reset_clears_fn_sel() {
    let mut a = make_accel();
    a.bar_write(0, 0x20, 4, 42);
    PciFunction::reset(&mut a);
    assert_eq!(a.bar_read(0, 0x20, 4), 0);
}

#[test]
fn pci_reset_clears_args() {
    let mut a = make_accel();
    a.bar_write(0, 0x28, 4, 0xFFFF_FFFF);
    PciFunction::reset(&mut a);
    assert_eq!(a.bar_read(0, 0x28, 4), 0);
}

#[test]
fn pci_reset_returns_to_idle() {
    let mut a = make_accel();
    a.bar_write(0, 0x04, 4, 1); // run → STATUS_COMPLETE
    assert_eq!(a.bar_read(0, 0x00, 4), 2);
    PciFunction::reset(&mut a);
    assert_eq!(
        a.bar_read(0, 0x00, 4),
        0,
        "STATUS should be 0 after PciFunction::reset"
    );
}

#[test]
fn pci_reset_clears_scratchpad() {
    let mut a = make_accel_with_scratch(64);
    a.bar_write(2, 0, 4, 0xDEAD_BEEF);
    PciFunction::reset(&mut a);
    assert_eq!(a.bar_read(2, 0, 4), 0);
}

#[test]
fn pci_reset_preserves_error_status_for_missing_file() {
    let mut a = make_error_accel();
    PciFunction::reset(&mut a);
    // Error state is preserved — the accelerator cannot be built.
    assert_eq!(a.bar_read(0, 0x00, 4), 3);
}

// ── MSI-X BAR (BAR4) ──────────────────────────────────────────────────────────

#[test]
fn msix_table_read_initial_zero() {
    let a = make_accel();
    // Vector 0 addr_lo
    assert_eq!(a.bar_read(4, 0, 4), 0);
}

#[test]
fn msix_table_write_read_addr() {
    let mut a = make_accel();
    a.bar_write(4, 0, 4, 0xFEE0_0000); // addr_lo
    assert_eq!(a.bar_read(4, 0, 4), 0xFEE0_0000);
}

#[test]
fn msix_table_write_read_data() {
    let mut a = make_accel();
    a.bar_write(4, 8, 4, 0x41); // data
    assert_eq!(a.bar_read(4, 8, 4), 0x41);
}
