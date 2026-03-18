//! Address map -- O(log n) flat-view device dispatch with transactional mutations.
//!
//! The address map maintains a set of [`MappedRegion`]s (each associating a
//! `[base, base+size)` range with a [`DeviceId`] and priority) and produces a
//! non-overlapping [`FlatViewEntry`] vector sorted by base address for O(log n)
//! lookup via binary search.
//!
//! Mutations are transactional: [`map_region`](AddressMap::map_region) and
//! [`unmap_region`](AddressMap::unmap_region) queue changes that take effect
//! only when [`commit`](AddressMap::commit) is called. This enables atomic
//! batched updates -- e.g. remapping a PCI BAR involves unmapping the old range
//! and mapping the new range in a single commit without an intermediate state
//! where the device is unreachable.
//!
//! # Priority-based overlap resolution
//!
//! When two regions overlap, the region with the higher `priority` value wins.
//! This models hardware overlays such as ROM/RAM shadowing, MMIO windows that
//! mask underlying memory, or PCI BAR re-configuration over a default region.
//!
//! # Example
//!
//! ```rust,no_run
//! use helm_devices::framework::address_map::{AddressMap, DeviceId, MappedRegion};
//!
//! let mut map = AddressMap::new();
//!
//! // Map a UART at 0x0900_0000, 4 KiB, default priority.
//! let uart_id = DeviceId(1);
//! map.map_region(MappedRegion {
//!     device_id: uart_id,
//!     base: 0x0900_0000,
//!     size: 0x1000,
//!     priority: 0,
//! });
//! map.commit();
//!
//! let entry = map.lookup(0x0900_0100).unwrap();
//! assert_eq!(entry.device_id, uart_id);
//! assert_eq!(entry.offset_in_device, 0);
//! ```

// ---------------------------------------------------------------------------
// DeviceId
// ---------------------------------------------------------------------------

/// Opaque identifier for a device registered in the address map.
///
/// The ID is assigned externally (e.g. by `World` or `DeviceRegistry`) and
/// used here purely as a tag to associate flat-view entries with their owning
/// device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u64);

// ---------------------------------------------------------------------------
// MappedRegion
// ---------------------------------------------------------------------------

/// A region mapping that associates an address range with a device.
///
/// Multiple regions may be created for the same `device_id` (e.g. a device
/// with separate config and data windows). Overlapping regions from different
/// devices are resolved by `priority` -- higher values take precedence.
#[derive(Debug, Clone)]
pub struct MappedRegion {
    /// Device that owns this region.
    pub device_id: DeviceId,
    /// Base address (inclusive).
    pub base: u64,
    /// Size of the region in bytes. The region covers `[base, base + size)`.
    pub size: u64,
    /// Priority for overlap resolution. Higher values take precedence when two
    /// regions cover the same address.
    pub priority: i32,
}

// ---------------------------------------------------------------------------
// FlatViewEntry
// ---------------------------------------------------------------------------

/// A single non-overlapping entry in the flattened address map.
///
/// After [`AddressMap::commit`], the `flat_view` contains a sorted vector of
/// these entries with no gaps or overlaps between entries. Binary search on
/// `base` gives O(log n) lookup.
#[derive(Debug, Clone)]
pub struct FlatViewEntry {
    /// Start address of this entry (inclusive).
    pub base: u64,
    /// Size of this entry in bytes. The entry covers `[base, base + size)`.
    pub size: u64,
    /// Device that owns this address range.
    pub device_id: DeviceId,
    /// Offset into the device's register space where `base` maps.
    ///
    /// For a region mapped at its natural base this is `0`. For a region that
    /// was clipped by a higher-priority overlay, this equals `entry_base -
    /// device_region_base`, so the device sees the correct internal offset
    /// regardless of how the flat view was split.
    pub offset_in_device: u64,
}

// ---------------------------------------------------------------------------
// Mutation
// ---------------------------------------------------------------------------

/// A pending mutation to be applied on the next [`AddressMap::commit`].
#[derive(Debug, Clone)]
pub enum Mutation {
    /// Add a new region mapping.
    Map(MappedRegion),
    /// Remove all committed regions matching `(device_id, base)`.
    Unmap {
        /// Device whose region should be removed.
        device_id: DeviceId,
        /// Base address of the region to remove.
        base: u64,
    },
}

// ---------------------------------------------------------------------------
// AddressMap
// ---------------------------------------------------------------------------

/// O(log n) address map with transactional (batched) mutations.
///
/// See [module-level documentation](self) for design details and examples.
pub struct AddressMap {
    /// All currently committed regions (post-commit).
    regions: Vec<MappedRegion>,
    /// Sorted, non-overlapping flat view for binary-search lookup.
    flat_view: Vec<FlatViewEntry>,
    /// Pending mutations not yet applied.
    pending: Vec<Mutation>,
    /// `true` if there are uncommitted mutations in `pending`.
    dirty: bool,
}

impl AddressMap {
    /// Create an empty address map with no regions.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            flat_view: Vec::new(),
            pending: Vec::new(),
            dirty: false,
        }
    }

    /// Queue a region mapping. The region takes effect on the next
    /// [`commit`](Self::commit).
    pub fn map_region(&mut self, region: MappedRegion) {
        self.pending.push(Mutation::Map(region));
        self.dirty = true;
    }

    /// Queue the removal of a region. The removal takes effect on the next
    /// [`commit`](Self::commit).
    ///
    /// The region is identified by `(device_id, base)` -- all committed
    /// regions matching both fields are removed.
    pub fn unmap_region(&mut self, device_id: DeviceId, base: u64) {
        self.pending.push(Mutation::Unmap { device_id, base });
        self.dirty = true;
    }

    /// Apply all pending mutations, rebuild the flat view, and clear the
    /// pending queue.
    ///
    /// This is the only operation that makes queued mutations visible to
    /// [`lookup`](Self::lookup). Calling `commit` with no pending mutations
    /// is a no-op (the flat view is not rebuilt).
    pub fn commit(&mut self) {
        if self.pending.is_empty() && !self.dirty {
            return;
        }

        for mutation in self.pending.drain(..) {
            match mutation {
                Mutation::Map(region) => {
                    self.regions.push(region);
                }
                Mutation::Unmap { device_id, base } => {
                    self.regions
                        .retain(|r| !(r.device_id == device_id && r.base == base));
                }
            }
        }

        self.rebuild_flat_view();
        self.dirty = false;
    }

    /// Look up the flat-view entry containing `addr`.
    ///
    /// Returns `None` if no committed region covers the address. Uses
    /// `partition_point` for O(log n) binary search on the sorted flat view.
    pub fn lookup(&self, addr: u64) -> Option<&FlatViewEntry> {
        // partition_point returns the first index i where flat_view[i].base > addr.
        // The candidate entry is at i-1 (the last entry with base <= addr).
        let idx = self.flat_view.partition_point(|e| e.base <= addr);
        if idx == 0 {
            return None;
        }
        let entry = &self.flat_view[idx - 1];
        // Use subtraction to avoid overflow: addr is >= entry.base (guaranteed
        // by the partition_point search), so this subtraction is safe.
        if addr - entry.base < entry.size {
            Some(entry)
        } else {
            None
        }
    }

    /// Rebuild the flat view from the committed region set.
    ///
    /// # Algorithm
    ///
    /// 1. Collect all regions as `(base, end, device_id, device_base, priority)`
    ///    tuples.
    /// 2. Sort by priority descending, then base ascending. This ensures
    ///    higher-priority regions are placed first and mask lower-priority ones.
    /// 3. For each region, call [`insert_interval`](Self::insert_interval)
    ///    which fills only gaps not already covered by previously placed
    ///    (higher-priority) entries.
    /// 4. Sort the final flat view by base address for binary search.
    fn rebuild_flat_view(&mut self) {
        self.flat_view.clear();
        if self.regions.is_empty() {
            return;
        }

        // Use u128 for end addresses to handle regions that extend to
        // u64::MAX + 1 without overflow.
        //
        // Tuple fields: (base, end_u128, device_id, device_base, priority)
        let mut intervals: Vec<(u64, u128, DeviceId, u64, i32)> = self
            .regions
            .iter()
            .map(|r| {
                (
                    r.base,
                    u128::from(r.base) + u128::from(r.size),
                    r.device_id,
                    r.base, // original base for offset_in_device calculation
                    r.priority,
                )
            })
            .collect();

        // Sort by priority descending, then base ascending. Higher-priority
        // regions are inserted first and therefore mask lower-priority ones.
        intervals.sort_by(|a, b| b.4.cmp(&a.4).then(a.0.cmp(&b.0)));

        for &(start, end, device_id, device_base, _) in &intervals {
            self.insert_interval(start, end, device_id, device_base);
        }

        // Final sort by base for binary search in lookup().
        self.flat_view.sort_by_key(|e| e.base);
    }

    /// Insert a region `[start, end)` into the flat view, filling only the
    /// gaps not already covered by previously inserted (higher-priority)
    /// entries.
    ///
    /// `device_base` is the region's original base address, used to compute
    /// `offset_in_device` correctly even when the region is split into
    /// multiple flat-view entries.
    ///
    /// `end` is a `u128` to correctly represent exclusive end addresses up to
    /// `2^64` (i.e. regions that extend through `u64::MAX`).
    #[allow(clippy::cast_possible_truncation)] // narrowing u128->u64 is intentional and safe here
    fn insert_interval(
        &mut self,
        start: u64,
        end: u128,
        device_id: DeviceId,
        device_base: u64,
    ) {
        let start_wide = u128::from(start);

        // Collect the sub-ranges of [start, end) already covered by existing
        // (higher-priority) entries.  Use u128 for end arithmetic.
        let mut covered: Vec<(u128, u128)> = Vec::new();
        for e in &self.flat_view {
            let e_start = u128::from(e.base);
            let e_end = e_start + u128::from(e.size);
            if e_start < end && start_wide < e_end {
                covered.push((e_start.max(start_wide), e_end.min(end)));
            }
        }

        if covered.is_empty() {
            // No overlap -- insert the entire range as one entry.
            self.flat_view.push(FlatViewEntry {
                base: start,
                size: (end - start_wide) as u64,
                device_id,
                offset_in_device: start - device_base,
            });
            return;
        }

        // Sort covered ranges by start address, then merge adjacent/overlapping
        // ones into a minimal set of excluded sub-ranges.
        covered.sort_by_key(|c| c.0);

        let mut merged: Vec<(u128, u128)> = Vec::new();
        for c in covered {
            if let Some(last) = merged.last_mut() {
                if c.0 <= last.1 {
                    last.1 = last.1.max(c.1);
                    continue;
                }
            }
            merged.push(c);
        }

        // Fill the uncovered gaps between (and around) the merged covered ranges.
        // All cursor/base values originate from u64, so the truncation back to
        // u64 is lossless.
        let mut cursor: u128 = start_wide;
        for &(cov_start, cov_end) in &merged {
            if cursor < cov_start {
                self.flat_view.push(FlatViewEntry {
                    base: cursor as u64,
                    size: (cov_start - cursor) as u64,
                    device_id,
                    offset_in_device: cursor as u64 - device_base,
                });
            }
            cursor = cov_end;
        }
        if cursor < end {
            self.flat_view.push(FlatViewEntry {
                base: cursor as u64,
                size: (end - cursor) as u64,
                device_id,
                offset_in_device: cursor as u64 - device_base,
            });
        }
    }

    /// Returns `true` if there are uncommitted mutations pending.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Returns the committed region set (all regions applied by prior commits).
    pub fn regions(&self) -> &[MappedRegion] {
        &self.regions
    }

    /// Returns the current flat view (for inspection or debugging).
    ///
    /// The entries are sorted by `base` and non-overlapping after each commit.
    pub fn flat_view(&self) -> &[FlatViewEntry] {
        &self.flat_view
    }
}

impl Default for AddressMap {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers ------------------------------------------------------------

    /// Create a `MappedRegion` with default priority 0.
    fn region(device_id: u64, base: u64, size: u64) -> MappedRegion {
        MappedRegion {
            device_id: DeviceId(device_id),
            base,
            size,
            priority: 0,
        }
    }

    /// Create a `MappedRegion` with explicit priority.
    fn region_prio(device_id: u64, base: u64, size: u64, priority: i32) -> MappedRegion {
        MappedRegion {
            device_id: DeviceId(device_id),
            base,
            size,
            priority,
        }
    }

    // -- Basic map / lookup -------------------------------------------------

    #[test]
    fn empty_map_returns_none() {
        let map = AddressMap::new();
        assert!(map.lookup(0).is_none());
        assert!(map.lookup(0xFFFF_FFFF_FFFF_FFFF).is_none());
    }

    #[test]
    fn single_region_lookup() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x1000, 0x100));
        map.commit();

        // Hit at base.
        let e = map.lookup(0x1000).unwrap();
        assert_eq!(e.device_id, DeviceId(1));
        assert_eq!(e.base, 0x1000);
        assert_eq!(e.size, 0x100);
        assert_eq!(e.offset_in_device, 0);

        // Hit at last byte.
        assert!(map.lookup(0x10FF).is_some());

        // Miss just past end.
        assert!(map.lookup(0x1100).is_none());

        // Miss before base.
        assert!(map.lookup(0x0FFF).is_none());
    }

    #[test]
    fn multiple_non_overlapping_regions() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x0000, 0x1000));
        map.map_region(region(2, 0x1000, 0x1000));
        map.map_region(region(3, 0x2000, 0x1000));
        map.commit();

        assert_eq!(map.lookup(0x0500).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0x1500).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0x2500).unwrap().device_id, DeviceId(3));

        // Past all regions.
        assert!(map.lookup(0x3000).is_none());
    }

    #[test]
    fn lookup_with_gap_between_regions() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x0000, 0x1000));
        // Gap: 0x1000..0x2000
        map.map_region(region(2, 0x2000, 0x1000));
        map.commit();

        assert!(map.lookup(0x0800).is_some());
        assert!(map.lookup(0x1500).is_none()); // in the gap
        assert!(map.lookup(0x2800).is_some());
    }

    // -- Binary search correctness (many regions) ---------------------------

    #[test]
    fn binary_search_100_regions() {
        let mut map = AddressMap::new();
        for i in 0..100u64 {
            map.map_region(region(i, i * 0x1000, 0x1000));
        }
        map.commit();

        for i in 0..100u64 {
            let e = map.lookup(i * 0x1000 + 0x800).unwrap();
            assert_eq!(e.device_id, DeviceId(i), "device {i} lookup failed");
        }

        // Past the last region.
        assert!(map.lookup(100 * 0x1000).is_none());
    }

    // -- Transactional semantics --------------------------------------------

    #[test]
    fn mutations_not_visible_before_commit() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x1000, 0x100));

        // Before commit -- not visible.
        assert!(map.lookup(0x1000).is_none());
        assert!(map.is_dirty());

        map.commit();

        assert!(map.lookup(0x1000).is_some());
        assert!(!map.is_dirty());
    }

    #[test]
    fn unmap_removes_region() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x1000, 0x100));
        map.commit();

        assert!(map.lookup(0x1000).is_some());

        map.unmap_region(DeviceId(1), 0x1000);
        map.commit();

        assert!(map.lookup(0x1000).is_none());
    }

    #[test]
    fn batch_map_and_unmap() {
        let mut map = AddressMap::new();

        // Map three regions.
        map.map_region(region(1, 0x0000, 0x100));
        map.map_region(region(2, 0x1000, 0x100));
        map.map_region(region(3, 0x2000, 0x100));
        map.commit();

        assert_eq!(map.flat_view().len(), 3);

        // Batch: unmap region 2, map region 4 -- both pending before commit.
        map.unmap_region(DeviceId(2), 0x1000);
        map.map_region(region(4, 0x3000, 0x100));
        assert!(map.is_dirty());
        map.commit();

        assert!(map.lookup(0x0000).is_some());
        assert!(map.lookup(0x1000).is_none()); // was unmapped
        assert!(map.lookup(0x2000).is_some());
        assert!(map.lookup(0x3000).is_some()); // newly mapped
        assert!(!map.is_dirty());
    }

    // -- Priority / overlap -------------------------------------------------

    #[test]
    fn higher_priority_wins_full_overlap() {
        let mut map = AddressMap::new();

        // Low-priority background region.
        map.map_region(region_prio(1, 0x0000, 0x10000, 0));
        // High-priority overlay in the middle.
        map.map_region(region_prio(2, 0x4000, 0x2000, 10));
        map.commit();

        // Before the overlay -- device 1.
        assert_eq!(map.lookup(0x0000).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0x3FFF).unwrap().device_id, DeviceId(1));

        // Inside the overlay -- device 2.
        assert_eq!(map.lookup(0x4000).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0x5FFF).unwrap().device_id, DeviceId(2));

        // After the overlay -- device 1 again.
        assert_eq!(map.lookup(0x6000).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0xFFFF).unwrap().device_id, DeviceId(1));
    }

    #[test]
    fn higher_priority_masks_lower_at_start() {
        let mut map = AddressMap::new();
        // Low-priority.
        map.map_region(region_prio(1, 0x0000, 0x4000, 0));
        // High-priority overlay at the start of region 1.
        map.map_region(region_prio(2, 0x0000, 0x1000, 5));
        map.commit();

        assert_eq!(map.lookup(0x0000).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0x0FFF).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0x1000).unwrap().device_id, DeviceId(1));
    }

    #[test]
    fn higher_priority_masks_lower_at_end() {
        let mut map = AddressMap::new();
        map.map_region(region_prio(1, 0x0000, 0x4000, 0));
        // Overlay at the end of region 1.
        map.map_region(region_prio(2, 0x3000, 0x1000, 5));
        map.commit();

        assert_eq!(map.lookup(0x2FFF).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0x3000).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0x3FFF).unwrap().device_id, DeviceId(2));
    }

    #[test]
    fn equal_priority_first_by_base_wins() {
        // When two regions have the same priority and overlap, the one with
        // the lower base sorts first in the priority group and is inserted
        // first, making it take precedence in the overlap.
        let mut map = AddressMap::new();
        map.map_region(region_prio(1, 0x0000, 0x2000, 0));
        map.map_region(region_prio(2, 0x1000, 0x2000, 0));
        map.commit();

        // Device 1 was inserted first (lower base at same priority).
        assert_eq!(map.lookup(0x1000).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0x1FFF).unwrap().device_id, DeviceId(1));

        // Device 2 gets the tail.
        assert_eq!(map.lookup(0x2000).unwrap().device_id, DeviceId(2));
    }

    // -- offset_in_device ---------------------------------------------------

    #[test]
    fn offset_in_device_with_clipped_region() {
        let mut map = AddressMap::new();
        map.map_region(region_prio(1, 0x0000, 0x10000, 0));
        map.map_region(region_prio(2, 0x4000, 0x2000, 10));
        map.commit();

        // The first chunk of device 1: [0x0000, 0x4000), offset 0.
        let e = map.lookup(0x0000).unwrap();
        assert_eq!(e.device_id, DeviceId(1));
        assert_eq!(e.offset_in_device, 0);

        // The second chunk of device 1: [0x6000, 0x10000), offset 0x6000.
        let e = map.lookup(0x6000).unwrap();
        assert_eq!(e.device_id, DeviceId(1));
        assert_eq!(e.offset_in_device, 0x6000);
    }

    #[test]
    fn offset_in_device_for_overlay_is_zero() {
        let mut map = AddressMap::new();
        map.map_region(region_prio(1, 0x0000, 0x10000, 0));
        map.map_region(region_prio(2, 0x4000, 0x2000, 10));
        map.commit();

        // The overlay (device 2) starts at its own base, so offset is 0.
        let e = map.lookup(0x4000).unwrap();
        assert_eq!(e.device_id, DeviceId(2));
        assert_eq!(e.offset_in_device, 0);
    }

    // -- regions() accessor -------------------------------------------------

    #[test]
    fn regions_accessor() {
        let mut map = AddressMap::new();
        assert!(map.regions().is_empty());

        map.map_region(region(1, 0x1000, 0x100));
        map.map_region(region(2, 0x2000, 0x100));
        map.commit();

        assert_eq!(map.regions().len(), 2);
    }

    // -- flat_view structure ------------------------------------------------

    #[test]
    fn flat_view_sorted_and_non_overlapping() {
        let mut map = AddressMap::new();
        // Regions added out of order, with an overlay.
        map.map_region(region_prio(3, 0x8000, 0x1000, 0));
        map.map_region(region_prio(1, 0x0000, 0xA000, -1));
        map.map_region(region_prio(2, 0x2000, 0x2000, 5));
        map.commit();

        let fv = map.flat_view();

        // Verify sorted by base.
        for window in fv.windows(2) {
            assert!(
                window[0].base < window[1].base,
                "flat_view not sorted: {:#x} >= {:#x}",
                window[0].base,
                window[1].base,
            );
        }

        // Verify no overlaps.
        for window in fv.windows(2) {
            let end_a = window[0].base + window[0].size;
            assert!(
                end_a <= window[1].base,
                "flat_view entries overlap: [{:#x}, {:#x}) and [{:#x}, {:#x})",
                window[0].base,
                end_a,
                window[1].base,
                window[1].base + window[1].size,
            );
        }
    }

    // -- Default trait ------------------------------------------------------

    #[test]
    fn default_creates_empty() {
        let map = AddressMap::default();
        assert!(map.flat_view().is_empty());
        assert!(map.regions().is_empty());
        assert!(!map.is_dirty());
    }

    // -- Edge cases ---------------------------------------------------------

    #[test]
    fn region_at_address_zero() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0, 0x100));
        map.commit();

        assert_eq!(map.lookup(0).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0xFF).unwrap().device_id, DeviceId(1));
        assert!(map.lookup(0x100).is_none());
    }

    #[test]
    fn region_at_high_address() {
        let mut map = AddressMap::new();
        let base = 0xFFFF_FFFF_FFFF_F000;
        map.map_region(region(1, base, 0x1000));
        map.commit();

        assert_eq!(map.lookup(base).unwrap().device_id, DeviceId(1));
        assert_eq!(
            map.lookup(base + 0xFFF).unwrap().device_id,
            DeviceId(1),
        );
    }

    #[test]
    fn double_commit_is_idempotent() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x1000, 0x100));
        map.commit();
        let fv_len = map.flat_view().len();

        // Second commit with no new mutations -- should not change flat view.
        map.commit();
        assert_eq!(map.flat_view().len(), fv_len);
    }

    #[test]
    fn unmap_nonexistent_region_is_harmless() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x1000, 0x100));
        map.commit();

        // Unmap something that was never mapped.
        map.unmap_region(DeviceId(99), 0x9999);
        map.commit();

        // Original region still present.
        assert!(map.lookup(0x1000).is_some());
    }

    #[test]
    fn multiple_regions_same_device() {
        let mut map = AddressMap::new();
        // Device 1 has two separate windows.
        map.map_region(MappedRegion {
            device_id: DeviceId(1),
            base: 0x1000,
            size: 0x100,
            priority: 0,
        });
        map.map_region(MappedRegion {
            device_id: DeviceId(1),
            base: 0x2000,
            size: 0x100,
            priority: 0,
        });
        map.commit();

        let e1 = map.lookup(0x1050).unwrap();
        let e2 = map.lookup(0x2050).unwrap();

        assert_eq!(e1.device_id, e2.device_id);
        assert_eq!(e1.device_id, DeviceId(1));
        // Each window starts at its own base, so offset is 0 for both.
        assert_eq!(e1.offset_in_device, 0);
        assert_eq!(e2.offset_in_device, 0);
    }

    #[test]
    fn three_layer_priority_stack() {
        let mut map = AddressMap::new();
        // Three layers covering a range with increasingly narrow overlays.
        map.map_region(region_prio(1, 0x0000, 0x10000, 0)); // bottom
        map.map_region(region_prio(2, 0x2000, 0x8000, 5));   // middle
        map.map_region(region_prio(3, 0x4000, 0x2000, 10));  // top

        map.commit();

        assert_eq!(map.lookup(0x1000).unwrap().device_id, DeviceId(1));
        assert_eq!(map.lookup(0x3000).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0x5000).unwrap().device_id, DeviceId(3));
        assert_eq!(map.lookup(0x7000).unwrap().device_id, DeviceId(2));
        assert_eq!(map.lookup(0xB000).unwrap().device_id, DeviceId(1));
    }

    #[test]
    fn unmap_only_matches_device_and_base() {
        let mut map = AddressMap::new();
        // Two different devices at different bases.
        map.map_region(region(1, 0x1000, 0x100));
        map.map_region(region(2, 0x2000, 0x100));
        map.commit();

        // Unmap device 1 at base 0x1000 -- should not affect device 2.
        map.unmap_region(DeviceId(1), 0x1000);
        map.commit();

        assert!(map.lookup(0x1000).is_none());
        assert!(map.lookup(0x2000).is_some());
    }

    #[test]
    fn map_after_unmap_in_same_batch() {
        let mut map = AddressMap::new();
        map.map_region(region(1, 0x1000, 0x100));
        map.commit();

        // In one batch: unmap old, map new at same address with different device.
        map.unmap_region(DeviceId(1), 0x1000);
        map.map_region(region(2, 0x1000, 0x100));
        map.commit();

        let e = map.lookup(0x1000).unwrap();
        assert_eq!(e.device_id, DeviceId(2));
    }
}
