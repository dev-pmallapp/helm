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
