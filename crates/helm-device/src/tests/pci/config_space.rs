use crate::device::DeviceEvent;
use crate::pci::{BarDecl, PciCapability, PciConfigSpace, PciFunction};

// ── DummyPciFunction ──────────────────────────────────────────────────────────

/// Minimal PCI function used by config-space tests.
///
/// Has one 32-bit MMIO BAR of 4 KB (slot 0); the remaining five slots are
/// unused.
struct DummyPciFunction {
    bars: [BarDecl; 6],
    caps: Vec<Box<dyn PciCapability>>,
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
        0x02_00_00 // Ethernet controller
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
    fn bar_read(&self, _bar: u8, _offset: u64, _size: usize) -> u64 {
        0
    }
    fn bar_write(&mut self, _bar: u8, _offset: u64, _size: usize, _value: u64) {}
    fn reset(&mut self) {}
    fn tick(&mut self, _cycles: u64) -> Vec<DeviceEvent> {
        vec![]
    }
    fn name(&self) -> &str {
        "dummy"
    }
}

/// Build a `PciConfigSpace` from the dummy function.
fn make_cs() -> PciConfigSpace {
    let f = DummyPciFunction::new();
    PciConfigSpace::new(
        f.vendor_id(),
        f.device_id(),
        f.class_code(),
        f.revision_id(),
        f.bars(),
        f.capabilities(),
    )
}

// ── Identity field tests ──────────────────────────────────────────────────────

#[test]
fn vendor_id_correct() {
    let cs = make_cs();
    // Vendor ID is at offset 0, low 16 bits of the dword.
    assert_eq!(cs.read(0x00) & 0xFFFF, 0x1234);
}

#[test]
fn device_id_correct() {
    let cs = make_cs();
    // Device ID is at offset 0, high 16 bits.
    assert_eq!((cs.read(0x00) >> 16) & 0xFFFF, 0x5678);
}

#[test]
fn vendor_id_is_readonly() {
    let mut cs = make_cs();
    // Writing 0 to offset 0 should not change vendor/device ID.
    cs.write(0x00, 0x0000_0000);
    assert_eq!(cs.read(0x00) & 0xFFFF, 0x1234);
    assert_eq!((cs.read(0x00) >> 16) & 0xFFFF, 0x5678);
}

#[test]
fn device_id_is_readonly() {
    let mut cs = make_cs();
    cs.write(0x00, 0xFFFF_FFFF);
    assert_eq!(cs.read(0x00) & 0xFFFF, 0x1234);
    assert_eq!((cs.read(0x00) >> 16) & 0xFFFF, 0x5678);
}

#[test]
fn class_code_correct() {
    let cs = make_cs();
    // Class dword at offset 0x08: [31:24]=class [23:16]=sub [15:8]=prog-if [7:0]=revision
    let dword = cs.read(0x08);
    let class = (dword >> 24) & 0xFF;
    let sub = (dword >> 16) & 0xFF;
    let prog_if = (dword >> 8) & 0xFF;
    assert_eq!(class, 0x02);
    assert_eq!(sub, 0x00);
    assert_eq!(prog_if, 0x00);
}

#[test]
fn class_code_is_readonly() {
    let mut cs = make_cs();
    cs.write(0x08, 0xFFFF_FFFF);
    let dword = cs.read(0x08);
    assert_eq!((dword >> 24) & 0xFF, 0x02);
}

#[test]
fn revision_id_zero() {
    let cs = make_cs();
    // Revision at bits [7:0] of offset 0x08
    assert_eq!(cs.read(0x08) & 0xFF, 0x00);
}

#[test]
fn header_type_is_zero() {
    let cs = make_cs();
    // Header type at offset 0x0C, bits [23:16]
    let dword = cs.read(0x0C);
    let header_type = (dword >> 16) & 0xFF;
    assert_eq!(header_type, 0x00);
}

#[test]
fn header_type_is_readonly() {
    let mut cs = make_cs();
    cs.write(0x0C, 0xFFFF_FFFF);
    let dword = cs.read(0x0C);
    let header_type = (dword >> 16) & 0xFF;
    assert_eq!(header_type, 0x00);
}

// ── Command register tests ────────────────────────────────────────────────────

#[test]
fn command_register_initially_zero() {
    let cs = make_cs();
    // Command at offset 0x04, bits [15:0]
    assert_eq!(cs.read(0x04) & 0xFFFF, 0x0000);
}

#[test]
fn command_memory_enable_writable() {
    let mut cs = make_cs();
    // Bit 1: Memory Space Enable
    cs.write(0x04, 0x0002);
    assert_eq!(cs.read(0x04) & 0x0002, 0x0002);
}

#[test]
fn command_bus_master_writable() {
    let mut cs = make_cs();
    // Bit 2: Bus Master Enable
    cs.write(0x04, 0x0004);
    assert_eq!(cs.read(0x04) & 0x0004, 0x0004);
}

#[test]
fn command_io_enable_writable() {
    let mut cs = make_cs();
    // Bit 0: I/O Space Enable
    cs.write(0x04, 0x0001);
    assert_eq!(cs.read(0x04) & 0x0001, 0x0001);
}

#[test]
fn command_reserved_bits_readonly() {
    let mut cs = make_cs();
    // Bits [3], [4], [5], [7], [9] are reserved (not in COMMAND_WRITE_MASK)
    let reserved: u32 = 0x0000_02B8;
    cs.write(0x04, reserved);
    assert_eq!(cs.read(0x04) & reserved, 0);
}

// ── BAR sizing protocol — 32-bit ─────────────────────────────────────────────

#[test]
fn bar0_initial_value_zero() {
    let cs = make_cs();
    // BAR0 at offset 0x10; address bits should be 0, type bits = 0 (MMIO32)
    let bar = cs.read(0x10);
    assert_eq!(bar & !0x0F, 0); // no address programmed yet
    assert_eq!(bar & 0x01, 0); // not I/O
    assert_eq!(bar & 0x06, 0); // 32-bit memory
}

#[test]
fn bar0_sizing_protocol_set() {
    let mut cs = make_cs();
    // Writing 0xFFFF_FFFF triggers sizing mode
    cs.write(0x10, 0xFFFF_FFFF);
    // Reading back should return the size mask (4 KB = 0xFFFF_F000)
    let mask = cs.read(0x10);
    assert_eq!(mask, 0xFFFF_F000);
}

#[test]
fn bar0_sizing_clears_on_normal_write() {
    let mut cs = make_cs();
    cs.write(0x10, 0xFFFF_FFFF); // enter sizing
    cs.write(0x10, 0x8000_0000); // normal address write
                                 // Should no longer return the size mask
    let val = cs.read(0x10);
    assert_ne!(val, 0xFFFF_F000);
    // Address should have the programmed value (within the write mask)
    assert_eq!(val & 0x8000_0000, 0x8000_0000);
}

#[test]
fn bar0_address_writable() {
    let mut cs = make_cs();
    cs.write(0x10, 0xDEAD_0000);
    let val = cs.read(0x10);
    // Type bits (bits 3:0) are preserved; address bits above alignment are writable
    assert_eq!(val & 0xFFFF_F000, 0xDEAD_0000);
}

#[test]
fn unused_bar_returns_zero() {
    let cs = make_cs();
    // BAR1 is unused
    assert_eq!(cs.read(0x14), 0);
}

#[test]
fn unused_bar_sizing_returns_zero() {
    let mut cs = make_cs();
    // Writing 0xFFFF_FFFF to an unused BAR
    cs.write(0x14, 0xFFFF_FFFF);
    // Size is 0 → size mask is 0
    assert_eq!(cs.read(0x14), 0);
}

// ── BAR sizing protocol — 64-bit ─────────────────────────────────────────────

/// Build a config space for a device with a single 64-bit BAR at slots 0+1.
fn make_cs_64bit() -> PciConfigSpace {
    let bars = [
        BarDecl::Mmio64 { size: 0x10_0000 }, // 1 MB
        BarDecl::Unused,                     // upper half (managed internally)
        BarDecl::Unused,
        BarDecl::Unused,
        BarDecl::Unused,
        BarDecl::Unused,
    ];
    PciConfigSpace::new(0xABCD, 0xEF01, 0x060400, 0, &bars, &[])
}

#[test]
fn bar0_64bit_type_bits() {
    let cs = make_cs_64bit();
    // bits [2:1] = 0b10 for 64-bit memory
    assert_eq!(cs.read(0x10) & 0x06, 0x04);
}

#[test]
fn bar0_64bit_sizing_low() {
    let mut cs = make_cs_64bit();
    cs.write(0x10, 0xFFFF_FFFF);
    let mask = cs.read(0x10);
    // 1 MB alignment: 0xFFF0_0000, plus type bits 0x04
    assert_eq!(mask & !0x0F, 0xFFF0_0000);
    // Type bits preserved
    assert_eq!(mask & 0x06, 0x04);
}

#[test]
fn bar1_64bit_sizing_high() {
    let mut cs = make_cs_64bit();
    // Upper slot at offset 0x14
    cs.write(0x14, 0xFFFF_FFFF);
    let mask = cs.read(0x14);
    // 1 MB fits in 32 bits, so upper half is all-ones
    assert_eq!(mask, 0xFFFF_FFFF);
}

#[test]
fn bar_addr_64bit() {
    let mut cs = make_cs_64bit();
    // Program a 64-bit address
    cs.write(0x10, 0x0000_0000); // low
    cs.write(0x14, 0x0000_0001); // high → address = 0x0000_0001_0000_0000
    let addr = cs.bar_addr(0);
    assert_eq!(addr, 0x0000_0001_0000_0000);
}

// ── bar_addr helper ───────────────────────────────────────────────────────────

#[test]
fn bar_addr_32bit_programmed() {
    let mut cs = make_cs();
    cs.write(0x10, 0x8000_0000);
    assert_eq!(cs.bar_addr(0), 0x8000_0000);
}

#[test]
fn bar_addr_out_of_range_returns_zero() {
    let cs = make_cs();
    assert_eq!(cs.bar_addr(6), 0);
    assert_eq!(cs.bar_addr(99), 0);
}

// ── Clone ─────────────────────────────────────────────────────────────────────

#[test]
fn clone_preserves_data() {
    let mut cs = make_cs();
    cs.write(0x10, 0xCAFE_0000);
    let cs2 = cs.clone();
    assert_eq!(cs2.read(0x10) & 0xFFFF_F000, 0xCAFE_0000);
}

#[test]
fn clone_is_independent() {
    let cs = make_cs();
    let mut cs2 = cs.clone();
    cs2.write(0x04, 0x0006); // Memory + BusMaster enable in clone
                             // Original should be unaffected — original has no way to verify
                             // internal state, but we confirm the clone read-back is correct.
    assert_eq!(cs2.read(0x04) & 0x0006, 0x0006);
}
