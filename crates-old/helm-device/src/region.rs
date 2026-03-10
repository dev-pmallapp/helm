//! Memory region tree — QEMU-inspired address space management.
//!
//! Regions have priorities; overlapping regions are resolved by priority
//! (higher wins). The tree is flattened into a `FlatView` for O(log n)
//! address lookup.

use helm_core::types::Addr;
use std::sync::{Arc, RwLock};

/// Unique identifier for a region within the tree.
pub type RegionId = u32;

/// A named region of address space with type and priority.
#[derive(Debug, Clone)]
pub struct MemRegion {
    pub name: String,
    pub base: Addr,
    pub size: u64,
    pub kind: RegionKind,
    /// Higher priority wins when regions overlap.
    pub priority: i32,
}

/// What a region does when accessed.
#[derive(Debug, Clone)]
pub enum RegionKind {
    /// MMIO — dispatches to a Device's `transact()`.
    Io,
    /// RAM — direct access to backing memory.
    Ram { backing: Arc<RwLock<Vec<u8>>> },
    /// Container — groups sub-regions, no default handler.
    Container,
    /// Alias — window into another region at a different offset.
    Alias { target: RegionId, offset: Addr },
}

/// A single entry in the flattened address map.
#[derive(Debug, Clone)]
pub struct FlatEntry {
    /// Start address (inclusive).
    pub start: Addr,
    /// End address (exclusive).
    pub end: Addr,
    /// Index into the region list.
    pub region_idx: usize,
    /// Offset within the region where `start` maps to.
    pub offset_in_region: Addr,
}

/// Manages a tree of memory regions and provides fast address lookup
/// via a flattened, sorted, non-overlapping view.
pub struct MemRegionTree {
    regions: Vec<MemRegion>,
    flat_view: Vec<FlatEntry>,
}

impl MemRegionTree {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            flat_view: Vec::new(),
        }
    }

    /// Add a region and rebuild the flat view.
    pub fn add(&mut self, region: MemRegion) -> usize {
        let idx = self.regions.len();
        self.regions.push(region);
        self.rebuild_flat_view();
        idx
    }

    /// Remove a region by index and rebuild.
    pub fn remove(&mut self, idx: usize) {
        if idx < self.regions.len() {
            self.regions.remove(idx);
            self.rebuild_flat_view();
        }
    }

    /// Get a region by index.
    pub fn get(&self, idx: usize) -> Option<&MemRegion> {
        self.regions.get(idx)
    }

    /// All regions.
    pub fn regions(&self) -> &[MemRegion] {
        &self.regions
    }

    /// The flattened view for address lookup.
    pub fn flat_view(&self) -> &[FlatEntry] {
        &self.flat_view
    }

    /// Rebuild the flat view from the region list.
    ///
    /// Algorithm: collect all region intervals, sort by priority (descending),
    /// then greedily assign non-overlapping segments. Higher priority regions
    /// mask lower ones.
    pub fn rebuild_flat_view(&mut self) {
        if self.regions.is_empty() {
            self.flat_view.clear();
            return;
        }

        // Collect (start, end, region_idx, priority) sorted by priority desc, then start asc
        let mut intervals: Vec<(Addr, Addr, usize, i32)> = self
            .regions
            .iter()
            .enumerate()
            .map(|(i, r)| (r.base, r.base.saturating_add(r.size), i, r.priority))
            .collect();
        intervals.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)));

        // Build flat view by inserting intervals in priority order.
        // Each interval masks anything already placed at lower priority.
        let mut entries: Vec<FlatEntry> = Vec::new();

        for (start, end, region_idx, _prio) in &intervals {
            self.insert_interval(&mut entries, *start, *end, *region_idx);
        }

        // Sort by start address
        entries.sort_by_key(|e| e.start);
        self.flat_view = entries;
    }

    /// Insert a region interval into the flat view, splitting existing
    /// entries as needed when a higher-priority region overlaps.
    fn insert_interval(
        &self,
        entries: &mut Vec<FlatEntry>,
        start: Addr,
        end: Addr,
        region_idx: usize,
    ) {
        let region = &self.regions[region_idx];

        // Check for overlap with existing entries
        let mut has_overlap = false;
        for e in entries.iter() {
            if e.start < end && start < e.end {
                has_overlap = true;
                break;
            }
        }

        if !has_overlap {
            // No overlap — insert directly
            entries.push(FlatEntry {
                start,
                end,
                region_idx,
                offset_in_region: 0,
            });
            return;
        }

        // There's overlap. Since we process in priority order, the new
        // interval should NOT override existing higher-priority entries.
        // Instead, fill in the gaps.
        let mut covered: Vec<(Addr, Addr)> = Vec::new();
        for e in entries.iter() {
            if e.start < end && start < e.end {
                covered.push((e.start.max(start), e.end.min(end)));
            }
        }
        covered.sort_by_key(|c| c.0);

        // Merge overlapping covered ranges
        let mut merged: Vec<(Addr, Addr)> = Vec::new();
        for c in covered {
            if let Some(last) = merged.last_mut() {
                if c.0 <= last.1 {
                    last.1 = last.1.max(c.1);
                    continue;
                }
            }
            merged.push(c);
        }

        // Fill gaps between covered ranges
        let mut cursor = start;
        for (cov_start, cov_end) in &merged {
            if cursor < *cov_start {
                entries.push(FlatEntry {
                    start: cursor,
                    end: *cov_start,
                    region_idx,
                    offset_in_region: cursor - region.base,
                });
            }
            cursor = *cov_end;
        }
        if cursor < end {
            entries.push(FlatEntry {
                start: cursor,
                end,
                region_idx,
                offset_in_region: cursor - region.base,
            });
        }
    }

    /// Look up an address in the flat view. Returns the matching entry.
    ///
    /// Uses binary search for O(log n) lookup.
    pub fn lookup(&self, addr: Addr) -> Option<&FlatEntry> {
        let idx = self
            .flat_view
            .binary_search_by(|entry| {
                if addr < entry.start {
                    std::cmp::Ordering::Greater
                } else if addr >= entry.end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.flat_view[idx])
    }

    /// Check if an address is mapped.
    pub fn contains(&self, addr: Addr) -> bool {
        self.lookup(addr).is_some()
    }
}

impl Default for MemRegionTree {
    fn default() -> Self {
        Self::new()
    }
}
