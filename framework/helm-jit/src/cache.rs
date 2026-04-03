//! Direct-mapped JIT block cache with tiered compilation support.

#![allow(missing_docs)]

use crate::block::CompiledBlock;
use std::sync::Arc;

/// Number of entries in the direct-mapped cache (must be a power of 2).
const CACHE_SIZE: usize = 4096;
const CACHE_MASK: u64 = (CACHE_SIZE as u64) - 1;

/// Execution count at which a stencil block is promoted to dynasm.
pub const PROMOTE_THRESHOLD: u32 = 64;

/// Which JIT tier compiled a cached block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitTier {
    /// Fast-compile baseline (stencil copy-and-patch).
    Stencil,
    /// Optimized hot-tier (dynasm runtime code generation).
    Dynasm,
}

/// Cached entry: stores the full guest PC for tag comparison.
struct CacheEntry {
    /// Full guest PC of the cached block.
    guest_pc: u64,
    /// The compiled block (shared ownership so the cache can be cleared
    /// without invalidating in-flight block references).
    block: Arc<CompiledBlock>,
    /// Number of times this block has been executed.
    exec_count: u32,
    /// Which tier compiled this block.
    tier: JitTier,
}

/// Result of a cache lookup with heat tracking.
pub struct CacheLookup {
    /// The compiled block.
    pub block: Arc<CompiledBlock>,
    /// Number of executions (after incrementing).
    pub exec_count: u32,
    /// Which tier compiled this block.
    pub tier: JitTier,
}

/// A 4096-entry direct-mapped cache for compiled translation blocks.
///
/// Keyed by `(pc >> 2) & 0xFFF` (AArch64 instructions are 4-byte aligned).
/// On collision, the old entry is silently evicted.
///
/// Tracks execution counts per entry for tiered JIT promotion: stencil-compiled
/// blocks that exceed `PROMOTE_THRESHOLD` executions can be recompiled with
/// dynasm for better code quality.
pub struct JitCache {
    entries: Vec<Option<CacheEntry>>,
    /// Number of blocks currently cached.
    count: usize,
    /// Number of blocks promoted from stencil to dynasm.
    promotions: u64,
    /// Number of cache entries evicted due to index collisions.
    evictions: u64,
}

impl JitCache {
    /// Create an empty JIT cache.
    pub fn new() -> Self {
        let mut entries = Vec::with_capacity(CACHE_SIZE);
        entries.resize_with(CACHE_SIZE, || None);
        Self {
            entries,
            count: 0,
            promotions: 0,
            evictions: 0,
        }
    }

    /// Compute the cache index for a given guest PC.
    #[inline]
    fn index(pc: u64) -> usize {
        ((pc >> 2) & CACHE_MASK) as usize
    }

    /// Look up a compiled block by guest PC (immutable, no heat tracking).
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

    /// Look up a compiled block and increment its execution count.
    ///
    /// Returns the block, updated execution count, and compilation tier.
    pub fn lookup_hot(&mut self, pc: u64) -> Option<CacheLookup> {
        let idx = Self::index(pc);
        self.entries[idx].as_mut().and_then(|e| {
            if e.guest_pc == pc {
                e.exec_count = e.exec_count.saturating_add(1);
                Some(CacheLookup {
                    block: Arc::clone(&e.block),
                    exec_count: e.exec_count,
                    tier: e.tier,
                })
            } else {
                None
            }
        })
    }

    /// Insert a compiled block into the cache.
    ///
    /// On collision, the previous entry at this index is silently evicted.
    pub fn insert(&mut self, block: CompiledBlock) {
        self.insert_with_tier(block, JitTier::Stencil);
    }

    /// Insert a compiled block with an explicit tier tag.
    pub fn insert_with_tier(&mut self, block: CompiledBlock, tier: JitTier) {
        let pc = block.guest_pc;
        let idx = Self::index(pc);
        if let Some(existing) = &self.entries[idx] {
            if existing.guest_pc != pc {
                self.evictions += 1;
            }
        } else {
            self.count += 1;
        }
        self.entries[idx] = Some(CacheEntry {
            guest_pc: pc,
            block: Arc::new(block),
            exec_count: 0,
            tier,
        });
    }

    /// Replace a cached block with a promoted version (e.g. dynasm).
    /// Preserves the execution count. Returns true if replaced.
    pub fn promote(&mut self, pc: u64, block: CompiledBlock, tier: JitTier) -> bool {
        let idx = Self::index(pc);
        if let Some(entry) = &mut self.entries[idx] {
            if entry.guest_pc == pc {
                let old_count = entry.exec_count;
                entry.block = Arc::new(block);
                entry.tier = tier;
                entry.exec_count = old_count;
                self.promotions += 1;
                return true;
            }
        }
        false
    }

    /// Number of blocks currently in the cache.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Number of blocks promoted from one tier to another.
    pub fn promotions(&self) -> u64 {
        self.promotions
    }

    /// Number of cache entries evicted due to index collisions.
    pub fn evictions(&self) -> u64 {
        self.evictions
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
#[cfg(feature = "backend-dynasm")]
mod tests {
    use super::*;
    use crate::block::JitBlockFn;
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
            let entry: JitBlockFn = std::mem::transmute(buf.ptr(dynasmrt::AssemblyOffset(0)));
            CompiledBlock::new(buf, entry, pc, 1)
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

    #[test]
    fn heat_tracking() {
        let mut cache = JitCache::new();
        cache.insert(make_test_block(0x1000));

        for i in 1..=PROMOTE_THRESHOLD {
            let hit = cache.lookup_hot(0x1000).unwrap();
            assert_eq!(hit.exec_count, i);
            assert_eq!(hit.tier, JitTier::Stencil);
        }
    }

    #[test]
    fn promote_replaces_block() {
        let mut cache = JitCache::new();
        cache.insert(make_test_block(0x1000));

        // Simulate some executions
        for _ in 0..10 {
            cache.lookup_hot(0x1000);
        }

        // Promote
        let new_block = make_test_block(0x1000);
        assert!(cache.promote(0x1000, new_block, JitTier::Dynasm));
        assert_eq!(cache.promotions(), 1);

        // After promotion, tier is Dynasm and count is preserved
        let hit = cache.lookup_hot(0x1000).unwrap();
        assert_eq!(hit.tier, JitTier::Dynasm);
        assert_eq!(hit.exec_count, 11); // 10 + 1 from this lookup
    }
}
