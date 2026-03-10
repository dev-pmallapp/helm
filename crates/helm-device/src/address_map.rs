//! Address map — O(log n) device dispatch with transactional mutations.
//!
//! Replaces the O(n) linear scan in [`DeviceBus`](crate::bus::DeviceBus) with a
//! sorted flat view and binary search. Mutations (map/unmap) are batched and
//! applied atomically via [`commit()`](AddressMap::commit), which rebuilds the
//! flat view and notifies listeners (e.g. JIT TLB invalidation).
//!
//! The existing `DeviceBus` continues to work; this is a parallel replacement.

use crate::device::{Device, DeviceId};
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// Opaque handle returned by [`AddressMap::map_region`]. Used to unmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionHandle(u32);

/// A device stored in the address map, indexed by [`DeviceId`].
struct DeviceEntry {
    device: Box<dyn Device>,
    name: String,
}

/// A mapped region linking an address range to a device.
#[derive(Debug, Clone)]
struct MappedRegion {
    handle: RegionHandle,
    device_id: DeviceId,
    base: Addr,
    size: u64,
    /// Priority for overlapping regions (higher wins).
    priority: i32,
}

/// A single entry in the flattened, sorted, non-overlapping address map.
#[derive(Debug, Clone)]
pub struct FlatViewEntry {
    pub start: Addr,
    pub end: Addr,
    pub device_id: DeviceId,
    /// Offset into the device's register space where `start` maps.
    pub offset_in_device: Addr,
}

/// Pending mutation to be applied on [`commit()`](AddressMap::commit).
enum Mutation {
    MapRegion(MappedRegion),
    UnmapRegion(RegionHandle),
}

/// Listener notified when the flat view changes (e.g. JIT TLB invalidation).
pub trait AddressMapListener: Send + Sync {
    /// A region was added to the flat view.
    fn on_region_add(&mut self, start: Addr, end: Addr, device_id: DeviceId);
    /// A region was removed from the flat view.
    fn on_region_remove(&mut self, start: Addr, end: Addr, device_id: DeviceId);
}

/// O(log n) address map with transactional mutations.
pub struct AddressMap {
    /// Device storage, indexed by DeviceId.
    devices: Vec<Option<DeviceEntry>>,
    /// All currently active mapped regions (post-commit).
    regions: Vec<MappedRegion>,
    /// Sorted, non-overlapping flat view for fast dispatch.
    flat_view: Vec<FlatViewEntry>,
    /// Pending mutations not yet committed.
    pending: Vec<Mutation>,
    /// Next region handle.
    next_handle: u32,
    /// Listeners notified on commit.
    listeners: Vec<Box<dyn AddressMapListener>>,
}

impl AddressMap {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            regions: Vec::new(),
            flat_view: Vec::new(),
            pending: Vec::new(),
            next_handle: 0,
            listeners: Vec::new(),
        }
    }

    /// Attach a device to the map. Returns a [`DeviceId`] for later reference.
    /// Does NOT create any address mapping — call [`map_region`] for that.
    pub fn attach(&mut self, name: impl Into<String>, device: Box<dyn Device>) -> DeviceId {
        let id = self.devices.len() as DeviceId;
        self.devices.push(Some(DeviceEntry {
            device,
            name: name.into(),
        }));
        id
    }

    /// Detach a device, removing it from the map and returning it.
    /// Any mapped regions for this device are removed on the next [`commit`].
    pub fn detach(&mut self, id: DeviceId) -> Option<Box<dyn Device>> {
        let entry = self.devices.get_mut(id as usize)?.take()?;
        // Queue removal of all regions belonging to this device
        let handles: Vec<RegionHandle> = self
            .regions
            .iter()
            .filter(|r| r.device_id == id)
            .map(|r| r.handle)
            .collect();
        for h in handles {
            self.pending.push(Mutation::UnmapRegion(h));
        }
        Some(entry.device)
    }

    /// Queue a region mapping. Takes effect on [`commit`].
    pub fn map_region(
        &mut self,
        device_id: DeviceId,
        base: Addr,
        size: u64,
        priority: i32,
    ) -> RegionHandle {
        let handle = RegionHandle(self.next_handle);
        self.next_handle += 1;
        self.pending.push(Mutation::MapRegion(MappedRegion {
            handle,
            device_id,
            base,
            size,
            priority,
        }));
        handle
    }

    /// Queue a region removal. Takes effect on [`commit`].
    pub fn unmap_region(&mut self, handle: RegionHandle) {
        self.pending.push(Mutation::UnmapRegion(handle));
    }

    /// Apply all pending mutations and rebuild the flat view.
    pub fn commit(&mut self) {
        // Apply mutations
        for mutation in self.pending.drain(..) {
            match mutation {
                Mutation::MapRegion(region) => {
                    self.regions.push(region);
                }
                Mutation::UnmapRegion(handle) => {
                    self.regions.retain(|r| r.handle != handle);
                }
            }
        }

        let old_flat = std::mem::take(&mut self.flat_view);
        self.rebuild_flat_view();

        // Notify listeners of changes
        if !self.listeners.is_empty() {
            // Find removed entries
            for old in &old_flat {
                let still_exists = self.flat_view.iter().any(|n| {
                    n.start == old.start && n.end == old.end && n.device_id == old.device_id
                });
                if !still_exists {
                    for listener in &mut self.listeners {
                        listener.on_region_remove(old.start, old.end, old.device_id);
                    }
                }
            }
            // Find added entries
            for new_entry in &self.flat_view {
                let is_new = !old_flat.iter().any(|o| {
                    o.start == new_entry.start
                        && o.end == new_entry.end
                        && o.device_id == new_entry.device_id
                });
                if is_new {
                    for listener in &mut self.listeners {
                        listener.on_region_add(
                            new_entry.start,
                            new_entry.end,
                            new_entry.device_id,
                        );
                    }
                }
            }
        }
    }

    /// Rebuild the flat view from the active region list.
    ///
    /// Algorithm: sort regions by priority (descending), then greedily assign
    /// non-overlapping segments. Higher priority masks lower.
    fn rebuild_flat_view(&mut self) {
        self.flat_view.clear();
        if self.regions.is_empty() {
            return;
        }

        // Collect into owned tuples to avoid borrowing self.regions while
        // mutating self.flat_view via insert_interval.
        let mut intervals: Vec<(Addr, Addr, DeviceId, i32)> = self
            .regions
            .iter()
            .map(|r| (r.base, r.base.saturating_add(r.size), r.device_id, r.priority))
            .collect();
        // Sort by priority descending, then base ascending
        intervals.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)));

        for &(start, end, device_id, _) in &intervals {
            self.insert_interval(start, end, device_id);
        }

        self.flat_view.sort_by_key(|e| e.start);
    }

    /// Insert a region into the flat view, filling gaps around existing
    /// higher-priority entries.
    fn insert_interval(&mut self, start: Addr, end: Addr, device_id: DeviceId) {
        // Find overlapping existing entries (higher priority, already placed)
        let mut covered: Vec<(Addr, Addr)> = Vec::new();
        for e in &self.flat_view {
            if e.start < end && start < e.end {
                covered.push((e.start.max(start), e.end.min(end)));
            }
        }

        if covered.is_empty() {
            self.flat_view.push(FlatViewEntry {
                start,
                end,
                device_id,
                offset_in_device: 0,
            });
            return;
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

        // Fill gaps
        let base = start;
        let mut cursor = start;
        for (cov_start, cov_end) in &merged {
            if cursor < *cov_start {
                self.flat_view.push(FlatViewEntry {
                    start: cursor,
                    end: *cov_start,
                    device_id,
                    offset_in_device: cursor - base,
                });
            }
            cursor = *cov_end;
        }
        if cursor < end {
            self.flat_view.push(FlatViewEntry {
                start: cursor,
                end,
                device_id,
                offset_in_device: cursor - base,
            });
        }
    }

    /// Look up which device owns an address. O(log n) binary search.
    pub fn lookup(&self, addr: Addr) -> Option<&FlatViewEntry> {
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

    /// Dispatch a transaction to the device at `txn.addr`. O(log n).
    pub fn dispatch(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let entry = self.lookup(txn.addr).ok_or_else(|| helm_core::HelmError::Memory {
            addr: txn.addr,
            reason: "no device mapped at this address".into(),
        })?;
        let device_id = entry.device_id;
        let offset = entry.offset_in_device + (txn.addr - entry.start);
        txn.offset = offset;

        let dev = self
            .devices
            .get_mut(device_id as usize)
            .and_then(|e| e.as_mut())
            .ok_or_else(|| helm_core::HelmError::Memory {
                addr: txn.addr,
                reason: "device detached".into(),
            })?;
        dev.device.transact(txn)
    }

    /// Fast-path read — O(log n) lookup, no Transaction overhead.
    pub fn read_fast(&mut self, addr: Addr, size: usize) -> HelmResult<u64> {
        let entry = self.lookup(addr).ok_or_else(|| helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })?;
        let device_id = entry.device_id;
        let offset = entry.offset_in_device + (addr - entry.start);

        let dev = self
            .devices
            .get_mut(device_id as usize)
            .and_then(|e| e.as_mut())
            .ok_or_else(|| helm_core::HelmError::Memory {
                addr,
                reason: "device detached".into(),
            })?;
        dev.device.read_fast(offset, size)
    }

    /// Fast-path write — O(log n) lookup, no Transaction overhead.
    pub fn write_fast(&mut self, addr: Addr, size: usize, value: u64) -> HelmResult<()> {
        let entry = self.lookup(addr).ok_or_else(|| helm_core::HelmError::Memory {
            addr,
            reason: "no device mapped at this address".into(),
        })?;
        let device_id = entry.device_id;
        let offset = entry.offset_in_device + (addr - entry.start);

        let dev = self
            .devices
            .get_mut(device_id as usize)
            .and_then(|e| e.as_mut())
            .ok_or_else(|| helm_core::HelmError::Memory {
                addr,
                reason: "device detached".into(),
            })?;
        dev.device.write_fast(offset, size, value)
    }

    /// Add a listener that is notified when the flat view changes.
    pub fn add_listener(&mut self, listener: Box<dyn AddressMapListener>) {
        self.listeners.push(listener);
    }

    /// The current flat view (for inspection/debugging).
    pub fn flat_view(&self) -> &[FlatViewEntry] {
        &self.flat_view
    }

    /// Number of attached devices (including detached slots).
    pub fn num_devices(&self) -> usize {
        self.devices.iter().filter(|e| e.is_some()).count()
    }

    /// Get a reference to a device by ID.
    pub fn device(&self, id: DeviceId) -> Option<&dyn Device> {
        self.devices
            .get(id as usize)?
            .as_ref()
            .map(|e| e.device.as_ref())
    }

    /// Get a mutable reference to a device by ID.
    pub fn device_mut(&mut self, id: DeviceId) -> Option<&mut dyn Device> {
        let entry = self.devices.get_mut(id as usize)?.as_mut()?;
        Some(entry.device.as_mut())
    }

    /// Device name by ID.
    pub fn device_name(&self, id: DeviceId) -> Option<&str> {
        self.devices
            .get(id as usize)?
            .as_ref()
            .map(|e| e.name.as_str())
    }
}

impl Default for AddressMap {
    fn default() -> Self {
        Self::new()
    }
}
