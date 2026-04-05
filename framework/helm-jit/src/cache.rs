//! Set-associative JIT block cache with tiered compilation support.

#![allow(missing_docs)]

use crate::block::CompiledBlock;
use std::sync::Arc;

/// Total number of cache entries (must be a power-of-two multiple of `CACHE_WAYS`).
const CACHE_SIZE: usize = 4096;
/// Number of ways per set.
const CACHE_WAYS: usize = 2;
/// Number of cache sets.
const CACHE_SETS: usize = CACHE_SIZE / CACHE_WAYS;
const CACHE_SET_MASK: u64 = (CACHE_SETS as u64) - 1;

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
    /// Monotonic touch sequence used for LRU victim selection.
    last_touch: u64,
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

/// A 4096-entry 2-way set-associative cache for compiled translation blocks.
///
/// Keyed by `(pc >> 2) & (CACHE_SETS - 1)` (AArch64 instructions are 4-byte
/// aligned). Each set keeps two candidate blocks and evicts the least-recently
/// used entry only after both ways are occupied.
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
    /// Monotonic counter for LRU ordering.
    touch_clock: u64,
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
            touch_clock: 0,
        }
    }

    /// Compute the cache set index for a given guest PC.
    #[inline]
    fn set_index(pc: u64) -> usize {
        ((pc >> 2) & CACHE_SET_MASK) as usize
    }

    #[inline]
    fn set_range(set_idx: usize) -> std::ops::Range<usize> {
        let base = set_idx * CACHE_WAYS;
        base..(base + CACHE_WAYS)
    }

    #[inline]
    fn next_touch(&mut self) -> u64 {
        self.touch_clock = self.touch_clock.saturating_add(1);
        self.touch_clock
    }

    fn find_entry_index(&self, pc: u64) -> Option<usize> {
        let set_idx = Self::set_index(pc);
        self.entries[Self::set_range(set_idx)]
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|e| e.guest_pc == pc))
            .map(|pos| set_idx * CACHE_WAYS + pos)
    }

    /// Look up a compiled block by guest PC (immutable, no heat tracking).
    pub fn lookup(&self, pc: u64) -> Option<Arc<CompiledBlock>> {
        self.find_entry_index(pc)
            .and_then(|idx| self.entries[idx].as_ref().map(|e| Arc::clone(&e.block)))
    }

    /// Look up a compiled block and increment its execution count.
    ///
    /// Returns the block, updated execution count, and compilation tier.
    pub fn lookup_hot(&mut self, pc: u64) -> Option<CacheLookup> {
        let idx = self.find_entry_index(pc)?;
        let touch = self.next_touch();
        let entry = self.entries[idx].as_mut()?;
        entry.exec_count = entry.exec_count.saturating_add(1);
        entry.last_touch = touch;
        Some(CacheLookup {
            block: Arc::clone(&entry.block),
            exec_count: entry.exec_count,
            tier: entry.tier,
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
        let touch = self.next_touch();

        if let Some(idx) = self.find_entry_index(pc) {
            self.entries[idx] = Some(CacheEntry {
                guest_pc: pc,
                block: Arc::new(block),
                exec_count: 0,
                tier,
                last_touch: touch,
            });
            return;
        }

        let set_idx = Self::set_index(pc);
        let set_range = Self::set_range(set_idx);
        if let Some(idx) = self.entries[set_range.clone()]
            .iter()
            .position(Option::is_none)
            .map(|pos| set_range.start + pos)
        {
            self.count += 1;
            self.entries[idx] = Some(CacheEntry {
                guest_pc: pc,
                block: Arc::new(block),
                exec_count: 0,
                tier,
                last_touch: touch,
            });
            return;
        }

        let victim_idx = self.entries[set_range.clone()]
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.as_ref().map_or(u64::MAX, |e| e.last_touch))
            .map(|(pos, _)| set_range.start + pos)
            .expect("set must contain at least one victim");

        if let Some(victim_pc) = self.entries[victim_idx].as_ref().map(|e| e.guest_pc) {
            self.unlink_block(victim_pc);
            self.evictions += 1;
        }

        self.entries[victim_idx] = Some(CacheEntry {
            guest_pc: pc,
            block: Arc::new(block),
            exec_count: 0,
            tier,
            last_touch: touch,
        });
    }

    /// Replace a cached block with a promoted version (e.g. dynasm).
    /// Preserves the execution count. Returns true if replaced.
    pub fn promote(&mut self, pc: u64, block: CompiledBlock, tier: JitTier) -> bool {
        let Some(idx) = self.find_entry_index(pc) else {
            return false;
        };

        let touch = self.next_touch();
        if let Some(entry) = &mut self.entries[idx] {
            let old_count = entry.exec_count;
            entry.block = Arc::new(block);
            entry.tier = tier;
            entry.exec_count = old_count;
            entry.last_touch = touch;
            self.promotions += 1;
            return true;
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
        self.touch_clock = 0;
    }

    // ── Block chaining (Phase 2-B) ───────────────────────────────────────────

    /// After inserting the block at `new_block_pc`, scan all cached blocks for
    /// unlinked patch sites that target `new_block_pc` and patch them with
    /// `jmp rel32` pointing at the new block's entry.
    ///
    /// Called immediately after `insert` / `insert_with_tier` in `run_jit`.
    #[allow(unsafe_code)]
    pub fn link_waiters(&mut self, new_block_pc: u64) {
        use crate::arena::CodeArena;

        // Resolve the target entry pointer first.
        let target_rx: *const u8 = {
            let Some(idx) = self.find_entry_index(new_block_pc) else {
                return;
            };
            match self.entries[idx].as_ref() {
                Some(e) => e.block.entry as *const u8,
                None => return,
            }
        };

        // Patch all cached blocks that have an unlinked exit targeting new_block_pc.
        for entry in self.entries.iter_mut().flatten() {
            // Collect indices of patch sites that need linking.
            let sites_to_link: Vec<usize> = entry
                .block
                .patch_sites
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.linked && s.target_pc == new_block_pc)
                .map(|(i, _)| i)
                .collect();

            for site_idx in sites_to_link {
                let byte_offset = entry.block.patch_sites[site_idx].byte_offset;
                // Only patchable blocks participate in chaining.
                let block_mut = match Arc::get_mut(&mut entry.block) {
                    Some(b) => b,
                    None => continue, // block is shared — skip
                };
                if let Some((buf, entry_offset)) = block_mut.take_buffer() {
                    // Compute site_rx = buf_base + byte_offset.
                    // buf_base is the RX ptr of the first byte of the buffer.
                    // We use target_rx as a reference; site_rx is reconstructed
                    // from the entry pointer (which points to offset 0) + byte_offset.
                    let site_rx = unsafe { (block_mut.entry as *const u8).add(byte_offset) };
                    let patched = CodeArena::write_jmp_rel32(buf, byte_offset, site_rx, target_rx);
                    block_mut.restore_buffer(patched, entry_offset);
                    block_mut.patch_sites[site_idx].linked = true;
                }
            }
        }
    }

    /// Unlink all blocks that have a `jmp rel32` pointing at `evicted_pc`.
    ///
    /// Called before evicting a block to restore `ret+nop×4` at all callers.
    #[allow(unsafe_code)]
    pub fn unlink_block(&mut self, evicted_pc: u64) {
        use crate::arena::CodeArena;

        for entry in self.entries.iter_mut().flatten() {
            let sites_to_unlink: Vec<usize> = entry
                .block
                .patch_sites
                .iter()
                .enumerate()
                .filter(|(_, s)| s.linked && s.target_pc == evicted_pc)
                .map(|(i, _)| i)
                .collect();

            for site_idx in sites_to_unlink {
                let byte_offset = entry.block.patch_sites[site_idx].byte_offset;
                let block_mut = match Arc::get_mut(&mut entry.block) {
                    Some(b) => b,
                    None => continue,
                };
                if let Some((buf, entry_offset)) = block_mut.take_buffer() {
                    let restored = CodeArena::write_ret_nop4(buf, byte_offset);
                    block_mut.restore_buffer(restored, entry_offset);
                    block_mut.patch_sites[site_idx].linked = false;
                }
            }
        }
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
    fn two_way_set_keeps_two_colliding_entries() {
        let mut cache = JitCache::new();
        // Two PCs that map to the same set: differ by CACHE_SETS*4.
        let pc_a = 0x4000_0000;
        let pc_b = pc_a + (CACHE_SETS as u64) * 4;

        cache.insert(make_test_block(pc_a));
        cache.insert(make_test_block(pc_b));

        assert!(cache.lookup(pc_a).is_some());
        assert!(cache.lookup(pc_b).is_some());
        assert_eq!(cache.evictions(), 0);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn third_collision_evicts_lru_way() {
        let mut cache = JitCache::new();
        let pc_a = 0x4000_0000;
        let pc_b = pc_a + (CACHE_SETS as u64) * 4;
        let pc_c = pc_b + (CACHE_SETS as u64) * 4;

        cache.insert(make_test_block(pc_a));
        cache.insert(make_test_block(pc_b));
        cache.insert(make_test_block(pc_c));

        // `pc_a` was inserted least recently, so it should be the victim.
        assert!(cache.lookup(pc_a).is_none());
        assert!(cache.lookup(pc_b).is_some());
        assert!(cache.lookup(pc_c).is_some());
        assert_eq!(cache.evictions(), 1);
        assert_eq!(cache.len(), 2);
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
