//! Tests for [`PciBus`] and [`PciHostBridge`].

use crate::device::Device;
use crate::device::DeviceEvent;
use crate::pci::{BarDecl, Bdf, PciBus, PciCapability, PciFunction, PciHostBridge};
use crate::transaction::Transaction;

// ── DummyPciFunction ──────────────────────────────────────────────────────────

/// Minimal PCI function used by bus/host tests.
///
/// Has one 32-bit MMIO BAR of 4 KB (slot 0); the remaining five slots are
/// unused. BAR reads/writes go to an internal `registers` array.
struct DummyPciFunction {
    bars: [BarDecl; 6],
    caps: Vec<Box<dyn PciCapability>>,
    registers: [u64; 4],
}

impl DummyPciFunction {
    fn new() -> Self {
        Self {
            bars: [
                BarDecl::Mmio32 { size: 0x1000 },
                BarDecl::Unused,
                BarDecl::Unused,
                BarDecl::Unused,
                BarDecl::Unused,
                BarDecl::Unused,
            ],
            caps: Vec::new(),
            registers: [0u64; 4],
        }
    }
}

impl PciFunction for DummyPciFunction {
    fn vendor_id(&self) -> u16 {
        0x1234
    }
    fn device_id(&self) -> u16 {
        0x5678
    }
    fn class_code(&self) -> u32 {
        0x02_00_00
    }
    fn bars(&self) -> &[BarDecl; 6] {
        &self.bars
    }
    fn capabilities(&self) -> &[Box<dyn PciCapability>] {
        &self.caps
    }
    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>> {
        &mut self.caps
    }
    fn bar_read(&self, _bar: u8, offset: u64, _size: usize) -> u64 {
        let idx = (offset / 8) as usize;
        if idx < self.registers.len() {
            self.registers[idx]
        } else {
            0
        }
    }
    fn bar_write(&mut self, _bar: u8, offset: u64, _size: usize, value: u64) {
        let idx = (offset / 8) as usize;
        if idx < self.registers.len() {
            self.registers[idx] = value;
        }
    }
    fn reset(&mut self) {
        self.registers = [0u64; 4];
    }
    fn tick(&mut self, _cycles: u64) -> Vec<DeviceEvent> {
        vec![]
    }
    fn name(&self) -> &str {
        "dummy"
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_bdf(device: u8) -> Bdf {
    Bdf::new(0, device, 0)
}

// ── PciBus tests ─────────────────────────────────────────────────────────────

#[test]
fn bus_new_enumerate_empty() {
    let bus = PciBus::new(0);
    assert!(bus.enumerate().is_empty());
}

#[test]
fn bus_attach_enumerate_one() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let bdfs = bus.enumerate();
    assert_eq!(bdfs.len(), 1);
    assert_eq!(bdfs[0], make_bdf(1));
}

#[test]
fn bus_enumerate_sorted() {
    let mut bus = PciBus::new(0);
    bus.attach(5, 0, Box::new(DummyPciFunction::new()));
    bus.attach(2, 0, Box::new(DummyPciFunction::new()));
    bus.attach(3, 0, Box::new(DummyPciFunction::new()));
    let bdfs = bus.enumerate();
    assert_eq!(bdfs.len(), 3);
    assert_eq!(bdfs[0].device, 2);
    assert_eq!(bdfs[1].device, 3);
    assert_eq!(bdfs[2].device, 5);
}

#[test]
fn bus_config_read_vendor_id() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let bdf = make_bdf(1);
    // Offset 0x00: vendor_id (low 16) | device_id (high 16)
    let dword = bus.config_read(bdf, 0x00);
    assert_eq!(dword & 0xFFFF, 0x1234, "vendor_id");
    assert_eq!((dword >> 16) & 0xFFFF, 0x5678, "device_id");
}

#[test]
fn bus_config_read_empty_slot_returns_all_ones() {
    let bus = PciBus::new(0);
    let bdf = make_bdf(7);
    assert_eq!(bus.config_read(bdf, 0x00), 0xFFFF_FFFF);
}

#[test]
fn bus_config_write_then_read_command_reg() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let bdf = make_bdf(1);
    // Write Memory Space Enable (bit 1) to command register at offset 0x04
    bus.config_write(bdf, 0x04, 0x0002);
    let val = bus.config_read(bdf, 0x04);
    assert_eq!(
        val & 0x0002,
        0x0002,
        "memory space enable bit should be set"
    );
}

#[test]
fn bus_bar_read_write_via_set_bar_mapping() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let bdf = make_bdf(1);

    // Map BAR0 at a synthetic platform address.
    bus.set_bar_mapping(bdf, 0, 0x1000_0000, 0x1000);

    // Write a value into register 0 (offset 0x00 within the BAR).
    assert!(bus.bar_write(0x1000_0000, 8, 0xDEAD_BEEF_CAFE_BABE));

    // Read it back.
    let val = bus.bar_read(0x1000_0000, 8);
    assert_eq!(val, Some(0xDEAD_BEEF_CAFE_BABE));
}

#[test]
fn bus_bar_read_unmapped_returns_none() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    // No mapping set.
    assert!(bus.bar_read(0x1000_0000, 4).is_none());
}

#[test]
fn bus_bar_address_after_set_mapping() {
    let mut bus = PciBus::new(0);
    bus.attach(2, 0, Box::new(DummyPciFunction::new()));
    let bdf = make_bdf(2);
    bus.set_bar_mapping(bdf, 0, 0x2000_0000, 0x1000);
    assert_eq!(bus.bar_address(bdf, 0), Some(0x2000_0000));
}

#[test]
fn bus_bar_address_unknown_returns_none() {
    let bus = PciBus::new(0);
    let bdf = make_bdf(3);
    assert!(bus.bar_address(bdf, 0).is_none());
}

#[test]
fn bus_reset_all_clears_bar_mappings() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let bdf = make_bdf(1);
    bus.set_bar_mapping(bdf, 0, 0x1000_0000, 0x1000);
    assert!(bus.bar_address(bdf, 0).is_some());

    bus.reset_all();

    // Mapping should be gone after reset.
    assert!(bus.bar_address(bdf, 0).is_none());
    // Reads to cleared mapping should return None.
    assert!(bus.bar_read(0x1000_0000, 4).is_none());
}

#[test]
fn bus_tick_all_returns_empty_for_dummy() {
    let mut bus = PciBus::new(0);
    bus.attach(1, 0, Box::new(DummyPciFunction::new()));
    let events = bus.tick_all(1000);
    assert!(events.is_empty());
}

// ── PciHostBridge tests ───────────────────────────────────────────────────────

/// Helper: build a bridge with small pools.
fn make_bridge() -> PciHostBridge {
    PciHostBridge::new(
        0x3000_0000, // ECAM base
        0x0100_0000, // ECAM size (1 MB)
        0x1000_0000, // MMIO32 base
        0x0800_0000, // MMIO32 size (128 MB)
    )
}

#[test]
fn bridge_name() {
    let bridge = make_bridge();
    assert_eq!(bridge.name(), "pci-host-bridge");
}

#[test]
fn bridge_regions_single_combined() {
    let bridge = make_bridge();
    let regions = bridge.regions();
    assert_eq!(regions.len(), 1);
    // Combined size = ECAM + MMIO32.
    assert_eq!(regions[0].size, 0x0100_0000 + 0x0800_0000);
}

#[test]
fn bridge_ecam_read_vendor_id() {
    let mut bridge = make_bridge();
    bridge.attach(1, 0, Box::new(DummyPciFunction::new()));

    // Compute ECAM offset for bus=0, dev=1, fn=0, reg=0x00.
    let bdf = Bdf::new(0, 1, 0);
    let ecam_off = bdf.ecam_offset(0x00);

    let mut txn = Transaction::read(ecam_off, 4);
    txn.offset = ecam_off;
    bridge.transact(&mut txn).unwrap();

    let vendor_id = txn.data_u32() & 0xFFFF;
    assert_eq!(vendor_id, 0x1234, "vendor ID via ECAM transact");
}

#[test]
fn bridge_ecam_empty_slot_all_ones() {
    let mut bridge = make_bridge();

    // No device at slot 5.
    let bdf = Bdf::new(0, 5, 0);
    let ecam_off = bdf.ecam_offset(0x00);

    let mut txn = Transaction::read(ecam_off, 4);
    txn.offset = ecam_off;
    bridge.transact(&mut txn).unwrap();

    assert_eq!(txn.data_u32(), 0xFFFF_FFFF);
}

#[test]
fn bridge_bar_auto_allocation_non_zero() {
    let mut bridge = make_bridge();
    bridge.attach(1, 0, Box::new(DummyPciFunction::new()));

    let bdf = Bdf::new(0, 1, 0);
    let addr = bridge.bar_address(bdf, 0);
    assert!(addr.is_some(), "BAR0 should be allocated");
    let a = addr.unwrap();
    // Must be within the MMIO32 pool.
    assert!(a >= 0x1000_0000);
    assert!(a < 0x1000_0000 + 0x0800_0000);
}

#[test]
fn bridge_bar_aligned_to_size() {
    let mut bridge = make_bridge();
    bridge.attach(1, 0, Box::new(DummyPciFunction::new()));

    let bdf = Bdf::new(0, 1, 0);
    let addr = bridge.bar_address(bdf, 0).unwrap();
    // BAR size is 0x1000 (4 KB) — address must be 4 KB aligned.
    assert_eq!(addr & 0xFFF, 0, "BAR address must be 4 KB aligned");
}

#[test]
fn bridge_two_devices_non_overlapping_bars() {
    let mut bridge = make_bridge();
    bridge.attach(1, 0, Box::new(DummyPciFunction::new()));
    bridge.attach(2, 0, Box::new(DummyPciFunction::new()));

    let bdf1 = Bdf::new(0, 1, 0);
    let bdf2 = Bdf::new(0, 2, 0);
    let a1 = bridge.bar_address(bdf1, 0).unwrap();
    let a2 = bridge.bar_address(bdf2, 0).unwrap();

    // BAR size is 0x1000; both addresses must be distinct and non-overlapping.
    assert_ne!(a1, a2, "two devices must not share the same BAR address");
    let end1 = a1 + 0x1000;
    let end2 = a2 + 0x1000;
    let no_overlap = end1 <= a2 || end2 <= a1;
    assert!(
        no_overlap,
        "BARs must not overlap: [{:#x}..{:#x}) and [{:#x}..{:#x})",
        a1, end1, a2, end2
    );
}

#[test]
fn bridge_mmio_read_write_via_transact() {
    let mut bridge = make_bridge();
    bridge.attach(1, 0, Box::new(DummyPciFunction::new()));

    let bdf = Bdf::new(0, 1, 0);
    let bar_phys = bridge.bar_address(bdf, 0).unwrap();

    // The MMIO offset within the bridge region = ecam_size + (bar_phys - mmio32_base).
    let ecam_size = 0x0100_0000u64;
    let mmio32_base = 0x1000_0000u64;
    let mmio_region_off = ecam_size + (bar_phys - mmio32_base);

    // Write register 0.
    let write_val: u64 = 0xCAFE_BABE_1234_5678;
    let mut wtxn = Transaction::write(mmio_region_off, 8, write_val);
    wtxn.offset = mmio_region_off;
    bridge.transact(&mut wtxn).unwrap();

    // Read it back.
    let mut rtxn = Transaction::read(mmio_region_off, 8);
    rtxn.offset = mmio_region_off;
    bridge.transact(&mut rtxn).unwrap();

    assert_eq!(
        rtxn.data_u64(),
        write_val,
        "BAR MMIO round-trip via transact"
    );
}

#[test]
fn bridge_transact_adds_stall_cycle() {
    let mut bridge = make_bridge();
    let bdf = Bdf::new(0, 9, 0);
    let ecam_off = bdf.ecam_offset(0x00);

    let mut txn = Transaction::read(ecam_off, 4);
    txn.offset = ecam_off;
    txn.stall_cycles = 0;
    bridge.transact(&mut txn).unwrap();

    assert!(
        txn.stall_cycles >= 1,
        "transact must add at least 1 stall cycle"
    );
}
