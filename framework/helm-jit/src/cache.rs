//! Direct-mapped JIT block cache.

#![allow(missing_docs)]

use std::sync::Arc;
use crate::block::CompiledBlock;

/// Number of entries in the direct-mapped cache (must be a power of 2).
const CACHE_SIZE: usize = 4096;
const CACHE_MASK: u64 = (CACHE_SIZE as u64) - 1;

/// Cached entry: stores the full guest PC for tag comparison.
struct CacheEntry {
    /// Full guest PC of the cached block.
    guest_pc: u64,
    /// The compiled block (shared ownership so the cache can be cleared
    /// without invalidating in-flight block references).
    block: Arc<CompiledBlock>,
}

/// A 4096-entry direct-mapped cache for compiled translation blocks.
///
/// Keyed by `(pc >> 2) & 0xFFF` (AArch64 instructions are 4-byte aligned).
/// On collision, the old entry is silently evicted.
pub struct JitCache {
    entries: Vec<Option<CacheEntry>>,
    /// Number of blocks currently cached.
    count: usize,
}

impl JitCache {
    /// Create an empty JIT cache.
    pub fn new() -> Self {
        let mut entries = Vec::with_capacity(CACHE_SIZE);
        entries.resize_with(CACHE_SIZE, || None);
        Self { entries, count: 0 }
    }

    /// Compute the cache index for a given guest PC.
    #[inline]
    fn index(pc: u64) -> usize {
        ((pc >> 2) & CACHE_MASK) as usize
    }

    /// Look up a compiled block by guest PC.
    ///
    /// Returns `None` on a miss or tag mismatch (collision evicted the entry).
    pub fn lookup(&self, pc: u64) -> Option<Arc<CompiledBlock>> {
        let idx = Self::index(pc);
        self.entries[idx].as_ref().and_then(|e| {
            if e.guest_pc == pc {
                Some(Arc::clone(&e.block))
            } else {
                None
            }
        })
    }

    /// Insert a compiled block into the cache.
    ///
    /// On collision, the previous entry at this index is silently evicted.
    pub fn insert(&mut self, block: CompiledBlock) {
        let pc = block.guest_pc;
        let idx = Self::index(pc);
        if self.entries[idx].is_none() {
            self.count += 1;
        }
        self.entries[idx] = Some(CacheEntry {
            guest_pc: pc,
            block: Arc::new(block),
        });
    }

    /// Number of blocks currently in the cache.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Flush all entries from the cache.
    pub fn flush(&mut self) {
        for entry in &mut self.entries {
            *entry = None;
        }
        self.count = 0;
    }
}

impl Default for JitCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dynasm::dynasm;
    use dynasmrt::DynasmApi;

    /// Create a minimal compiled block (just a `ret`) for testing.
    fn make_test_block(pc: u64) -> CompiledBlock {
        let mut ops = dynasmrt::x64::Assembler::new().unwrap();
        dynasm!(ops
            ; xor rax, rax
            ; ret
        );
        let buf = ops.finalize().unwrap();
        #[allow(unsafe_code)]
        unsafe {
            CompiledBlock::new(buf, pc, 1)
        }
    }

    #[test]
    fn insert_and_lookup() {
        let mut cache = JitCache::new();
        assert!(cache.is_empty());

        cache.insert(make_test_block(0x4000_0000));
        assert_eq!(cache.len(), 1);

        let hit = cache.lookup(0x4000_0000);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().guest_pc, 0x4000_0000);

        // Miss for different PC
        assert!(cache.lookup(0x4000_0004).is_none());
    }

    #[test]
    fn collision_evicts() {
        let mut cache = JitCache::new();
        // Two PCs that map to the same index: differ by 4096*4 = 0x4000
        let pc_a = 0x4000_0000;
        let pc_b = pc_a + (CACHE_SIZE as u64) * 4;

        cache.insert(make_test_block(pc_a));
        cache.insert(make_test_block(pc_b));

        // pc_a should be evicted
        assert!(cache.lookup(pc_a).is_none());
        assert!(cache.lookup(pc_b).is_some());
    }

    #[test]
    fn flush_clears_all() {
        let mut cache = JitCache::new();
        cache.insert(make_test_block(0x1000));
        cache.insert(make_test_block(0x2000));
        assert_eq!(cache.len(), 2);

        cache.flush();
        assert!(cache.is_empty());
        assert!(cache.lookup(0x1000).is_none());
    }
}
