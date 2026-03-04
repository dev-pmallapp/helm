//! Translation cache — stores previously translated blocks for reuse.

use super::block::TranslatedBlock;
use helm_core::types::Addr;
use std::collections::HashMap;

pub struct TranslationCache {
    blocks: HashMap<Addr, TranslatedBlock>,
}

impl Default for TranslationCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TranslationCache {
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
        }
    }

    pub fn lookup(&self, pc: Addr) -> Option<&TranslatedBlock> {
        self.blocks.get(&pc)
    }

    pub fn insert(&mut self, block: TranslatedBlock) {
        self.blocks.insert(block.start_pc, block);
    }

    pub fn invalidate(&mut self, pc: Addr) {
        self.blocks.remove(&pc);
    }

    pub fn flush(&mut self) {
        self.blocks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

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
}
