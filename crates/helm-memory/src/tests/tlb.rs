use crate::mmu::Permissions;
use crate::tlb::*;

fn perms_rw() -> Permissions {
    Permissions { readable: true, writable: true, el1_executable: true, el0_executable: true }
}

#[test]
fn lookup_miss_returns_none() {
    let tlb = Tlb::new(16);
    assert!(tlb.lookup(0x1000, 0).is_none());
}

#[test]
fn insert_then_lookup_returns_translated_addr() {
    let mut tlb = Tlb::new(16);
    let entry = Tlb::make_entry(0x0000_1000, 0x0080_1000, 4096, perms_rw(), 0, 0, true);
    tlb.insert(entry);
    let (pa, _) = tlb.lookup(0x0000_1ABC, 0).unwrap();
    assert_eq!(pa, 0x0080_1ABC);
}

#[test]
fn eviction_occurs_at_capacity() {
    let mut tlb = Tlb::new(2);
    tlb.insert(Tlb::make_entry(0x1000, 0xA000, 4096, perms_rw(), 0, 0, true));
    tlb.insert(Tlb::make_entry(0x2000, 0xB000, 4096, perms_rw(), 0, 0, true));
    tlb.insert(Tlb::make_entry(0x3000, 0xC000, 4096, perms_rw(), 0, 0, true));
    assert!(tlb.lookup(0x3000, 0).is_some());
}

#[test]
fn lookup_preserves_page_offset_zero() {
    let mut tlb = Tlb::new(4);
    tlb.insert(Tlb::make_entry(0x0000_5000, 0x0010_5000, 4096, perms_rw(), 0, 0, true));
    let (pa, _) = tlb.lookup(0x0000_5000, 0).unwrap();
    assert_eq!(pa, 0x0010_5000);
}

#[test]
fn lookup_preserves_page_offset_near_end() {
    let mut tlb = Tlb::new(4);
    tlb.insert(Tlb::make_entry(0x0000_A000, 0x0020_A000, 4096, perms_rw(), 0, 0, true));
    let (pa, _) = tlb.lookup(0x0000_AFFF, 0).unwrap();
    assert_eq!(pa, 0x0020_AFFF);
}

#[test]
fn insert_same_vpn_overwrites() {
    let mut tlb = Tlb::new(4);
    tlb.insert(Tlb::make_entry(0x1000, 0xAAAA_0000, 4096, perms_rw(), 0, 0, true));
    tlb.insert(Tlb::make_entry(0x1000, 0xBBBB_0000, 4096, perms_rw(), 0, 0, true));
    let (pa, _) = tlb.lookup(0x1000, 0).unwrap();
    assert_eq!(pa, 0xBBBB_0000);
}

#[test]
fn capacity_one_evicts_on_second_insert() {
    let mut tlb = Tlb::new(1);
    tlb.insert(Tlb::make_entry(0x1000, 0xA000, 4096, perms_rw(), 0, 0, true));
    tlb.insert(Tlb::make_entry(0x2000, 0xB000, 4096, perms_rw(), 0, 0, true));
    assert!(tlb.lookup(0x2000, 0).is_some());
}

#[test]
fn flush_all_invalidates() {
    let mut tlb = Tlb::new(4);
    tlb.insert(Tlb::make_entry(0x1000, 0xA000, 4096, perms_rw(), 0, 0, true));
    tlb.insert(Tlb::make_entry(0x2000, 0xB000, 4096, perms_rw(), 0, 0, true));
    tlb.flush_all();
    assert!(tlb.lookup(0x1000, 0).is_none());
    assert!(tlb.lookup(0x2000, 0).is_none());
}

#[test]
fn asid_tagged_entries() {
    let mut tlb = Tlb::new(8);
    // Same VA, different ASIDs
    tlb.insert(Tlb::make_entry(0x1000, 0xA000, 4096, perms_rw(), 0, 1, false));
    tlb.insert(Tlb::make_entry(0x1000, 0xB000, 4096, perms_rw(), 0, 2, false));
    let (pa1, _) = tlb.lookup(0x1000, 1).unwrap();
    let (pa2, _) = tlb.lookup(0x1000, 2).unwrap();
    assert_eq!(pa1, 0xA000);
    assert_eq!(pa2, 0xB000);
    // Different ASID → miss
    assert!(tlb.lookup(0x1000, 3).is_none());
}

#[test]
fn global_matches_any_asid() {
    let mut tlb = Tlb::new(4);
    tlb.insert(Tlb::make_entry(0x1000, 0xA000, 4096, perms_rw(), 0, 0, true));
    assert!(tlb.lookup(0x1000, 0).is_some());
    assert!(tlb.lookup(0x1000, 99).is_some());
}

#[test]
fn flush_asid_only_removes_non_global() {
    let mut tlb = Tlb::new(8);
    tlb.insert(Tlb::make_entry(0x1000, 0xA000, 4096, perms_rw(), 0, 1, false)); // non-global
    tlb.insert(Tlb::make_entry(0x2000, 0xB000, 4096, perms_rw(), 0, 0, true));  // global
    tlb.flush_asid(1);
    assert!(tlb.lookup(0x1000, 1).is_none(), "non-global flushed");
    assert!(tlb.lookup(0x2000, 0).is_some(), "global retained");
}

#[test]
fn variable_page_size_2m() {
    let mut tlb = Tlb::new(4);
    let size_2m = 2 * 1024 * 1024;
    tlb.insert(Tlb::make_entry(0x0020_0000, 0x8020_0000, size_2m, perms_rw(), 0, 0, true));
    // Any offset within the 2M block should hit
    let (pa, _) = tlb.lookup(0x0020_1234, 0).unwrap();
    assert_eq!(pa, 0x8020_1234);
    let (pa, _) = tlb.lookup(0x003F_FFFF, 0).unwrap();
    assert_eq!(pa, 0x803F_FFFF);
    // Outside the block → miss
    assert!(tlb.lookup(0x0040_0000, 0).is_none());
}
