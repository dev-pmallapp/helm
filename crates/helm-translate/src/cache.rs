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
