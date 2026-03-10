use crate::address_space::AddressSpace;
use crate::flat::FlatMemoryAccess;
use helm_core::mem::MemoryAccess;

fn make_space() -> AddressSpace {
    let mut space = AddressSpace::new();
    space.map(0x0, 0x10000, (true, true, true));
    space
}

#[test]
fn flat_read_write_u64() {
    let mut space = make_space();
    let mut mem = FlatMemoryAccess { space: &mut space };
    mem.write(0x100, 8, 0xDEAD_BEEF_CAFE_BABE).unwrap();
    assert_eq!(mem.read(0x100, 8).unwrap(), 0xDEAD_BEEF_CAFE_BABE);
}

#[test]
fn flat_read_write_u32() {
    let mut space = make_space();
    let mut mem = FlatMemoryAccess { space: &mut space };
    mem.write(0x200, 4, 0x1234_5678).unwrap();
    assert_eq!(mem.read(0x200, 4).unwrap(), 0x1234_5678);
}

#[test]
fn flat_read_write_u16() {
    let mut space = make_space();
    let mut mem = FlatMemoryAccess { space: &mut space };
    mem.write(0x300, 2, 0xABCD).unwrap();
    assert_eq!(mem.read(0x300, 2).unwrap(), 0xABCD);
}

#[test]
fn flat_read_write_u8() {
    let mut space = make_space();
    let mut mem = FlatMemoryAccess { space: &mut space };
    mem.write(0x400, 1, 0xFF).unwrap();
    assert_eq!(mem.read(0x400, 1).unwrap(), 0xFF);
}

#[test]
fn flat_fetch() {
    let mut space = make_space();
    {
        let mut mem = FlatMemoryAccess { space: &mut space };
        mem.write(0x500, 4, 0x11223344).unwrap();
    }
    let mut mem = FlatMemoryAccess { space: &mut space };
    let mut buf = [0u8; 4];
    mem.fetch(0x500, &mut buf).unwrap();
    assert_eq!(buf, [0x44, 0x33, 0x22, 0x11]);
}

#[test]
fn flat_wide_read_write() {
    let mut space = make_space();
    let mut mem = FlatMemoryAccess { space: &mut space };
    let data: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    mem.write_wide(0x600, &data).unwrap();
    let mut out = [0u8; 16];
    mem.read_wide(0x600, &mut out).unwrap();
    assert_eq!(out, data);
}

#[test]
fn flat_unmapped_read_returns_fault() {
    let mut space = AddressSpace::new();
    // No regions mapped
    let mut mem = FlatMemoryAccess { space: &mut space };
    assert!(mem.read(0xFFFF_0000, 4).is_err());
}

#[test]
fn flat_unmapped_write_returns_fault() {
    let mut space = AddressSpace::new();
    let mut mem = FlatMemoryAccess { space: &mut space };
    assert!(mem.write(0xFFFF_0000, 4, 42).is_err());
}
