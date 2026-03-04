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
