use crate::tlb::*;

#[test]
fn lookup_miss_returns_none() {
    let tlb = Tlb::new(16);
    assert!(tlb.lookup(0x1000).is_none());
}

#[test]
fn insert_then_lookup_returns_translated_addr() {
    let mut tlb = Tlb::new(16);
    tlb.insert(0x0000_1000, 0x0080_1000);
    let pa = tlb.lookup(0x0000_1ABC);
    // Same page, offset 0xABC should be preserved.
    assert_eq!(pa, Some(0x0080_1ABC));
}

#[test]
fn eviction_occurs_at_capacity() {
    let mut tlb = Tlb::new(2);
    tlb.insert(0x1000, 0xA000);
    tlb.insert(0x2000, 0xB000);
    tlb.insert(0x3000, 0xC000); // should evict one
                                // At least the newest should be present.
    assert!(tlb.lookup(0x3000).is_some());
}

#[test]
fn lookup_preserves_page_offset_zero() {
    let mut tlb = Tlb::new(4);
    tlb.insert(0x0000_5000, 0x0010_5000);
    let pa = tlb.lookup(0x0000_5000); // offset 0
    assert_eq!(pa, Some(0x0010_5000));
}

#[test]
fn lookup_preserves_page_offset_near_end() {
    let mut tlb = Tlb::new(4);
    tlb.insert(0x0000_A000, 0x0020_A000);
    let pa = tlb.lookup(0x0000_AFFF); // offset 0xFFF
    assert_eq!(pa, Some(0x0020_AFFF));
}

#[test]
fn insert_same_vpn_overwrites() {
    let mut tlb = Tlb::new(4);
    tlb.insert(0x1000, 0xAAAA_0000);
    tlb.insert(0x1000, 0xBBBB_0000); // overwrite
    let pa = tlb.lookup(0x1000);
    assert_eq!(pa, Some(0xBBBB_0000));
}

#[test]
fn capacity_one_evicts_on_second_insert() {
    let mut tlb = Tlb::new(1);
    tlb.insert(0x1000, 0xA000);
    tlb.insert(0x2000, 0xB000); // evicts 0x1000
    assert!(tlb.lookup(0x2000).is_some()); // newest present
}
