use crate::block::TranslatedBlock;
use crate::cache::*;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;

fn dummy_block(pc: Addr) -> TranslatedBlock {
    TranslatedBlock {
        start_pc: pc,
        guest_size: 4,
        uops: vec![MicroOp {
            guest_pc: pc,
            opcode: Opcode::Nop,
            sources: vec![],
            dest: None,
            immediate: None,
            flags: MicroOpFlags::default(),
        }],
    }
}

#[test]
fn lookup_miss() {
    let cache = TranslationCache::new();
    assert!(cache.lookup(0x1000).is_none());
}

#[test]
fn insert_then_lookup() {
    let mut cache = TranslationCache::new();
    cache.insert(dummy_block(0x2000));
    let block = cache.lookup(0x2000);
    assert!(block.is_some());
    assert_eq!(block.unwrap().start_pc, 0x2000);
}

#[test]
fn invalidate_removes_block() {
    let mut cache = TranslationCache::new();
    cache.insert(dummy_block(0x3000));
    cache.invalidate(0x3000);
    assert!(cache.lookup(0x3000).is_none());
}

#[test]
fn flush_clears_all() {
    let mut cache = TranslationCache::new();
    cache.insert(dummy_block(0x1000));
    cache.insert(dummy_block(0x2000));
    cache.flush();
    assert!(cache.lookup(0x1000).is_none());
    assert!(cache.lookup(0x2000).is_none());
}

#[test]
fn invalidate_nonexistent_does_not_panic() {
    let mut cache = TranslationCache::new();
    cache.invalidate(0xDEAD_0000); // not in cache — must not crash
}

#[test]
fn multiple_blocks_coexist() {
    let mut cache = TranslationCache::new();
    for i in 0u64..8 {
        cache.insert(dummy_block(0x1000 + i * 4));
    }
    for i in 0u64..8 {
        assert!(cache.lookup(0x1000 + i * 4).is_some());
    }
}

#[test]
fn lookup_adjacent_pc_misses() {
    let mut cache = TranslationCache::new();
    cache.insert(dummy_block(0x4000));
    // 0x4004 is not in cache
    assert!(cache.lookup(0x4004).is_none());
}

#[test]
fn block_fields_preserved_in_cache() {
    let mut cache = TranslationCache::new();
    cache.insert(dummy_block(0x8000));
    let b = cache.lookup(0x8000).unwrap();
    assert_eq!(b.start_pc, 0x8000);
    assert_eq!(b.guest_size, 4);
    assert_eq!(b.uops.len(), 1);
}
