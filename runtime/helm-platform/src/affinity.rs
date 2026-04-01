//! CPU affinity map -- maps CPU indices to MPIDR values.
//!
//! Populated from Python config via `register_affinity(cpu_idx, mpidr)`.
//! Read by GIC distributor and other SMP-aware components at `elaborate()`.

use std::collections::HashMap;

/// Maps CPU index to MPIDR value for multi-processor topology.
///
/// Populated from Python configuration. Read-only after `elaborate()`.
#[derive(Debug, Clone, Default)]
pub struct AffinityMap {
    /// CPU index -> MPIDR.
    cpu_to_mpidr: HashMap<usize, u64>,
    /// MPIDR -> CPU index (reverse lookup).
    mpidr_to_cpu: HashMap<u64, usize>,
}

impl AffinityMap {
    /// Create an empty affinity map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a CPU with its MPIDR value.
    ///
    /// # Panics
    /// Panics if `cpu_idx` or `mpidr` is already registered (1:1 mapping required).
    pub fn register(&mut self, cpu_idx: usize, mpidr: u64) {
        assert!(
            !self.cpu_to_mpidr.contains_key(&cpu_idx),
            "CPU index {cpu_idx} already registered"
        );
        assert!(
            !self.mpidr_to_cpu.contains_key(&mpidr),
            "MPIDR {mpidr:#x} already registered"
        );
        self.cpu_to_mpidr.insert(cpu_idx, mpidr);
        self.mpidr_to_cpu.insert(mpidr, cpu_idx);
    }

    /// Look up the MPIDR for a CPU index.
    pub fn mpidr_of(&self, cpu_idx: usize) -> Option<u64> {
        self.cpu_to_mpidr.get(&cpu_idx).copied()
    }

    /// Look up the CPU index for an MPIDR value.
    pub fn cpu_of(&self, mpidr: u64) -> Option<usize> {
        self.mpidr_to_cpu.get(&mpidr).copied()
    }

    /// Number of registered CPUs.
    pub fn len(&self) -> usize {
        self.cpu_to_mpidr.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.cpu_to_mpidr.is_empty()
    }

    /// Iterate over (`cpu_idx`, mpidr) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.cpu_to_mpidr.iter().map(|(&idx, &mpidr)| (idx, mpidr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let mut map = AffinityMap::new();
        map.register(0, 0x0000_0000);
        map.register(1, 0x0000_0001);
        map.register(2, 0x0000_0100);

        assert_eq!(map.mpidr_of(0), Some(0x0000_0000));
        assert_eq!(map.mpidr_of(1), Some(0x0000_0001));
        assert_eq!(map.cpu_of(0x0000_0100), Some(2));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn missing_lookups() {
        let map = AffinityMap::new();
        assert_eq!(map.mpidr_of(0), None);
        assert_eq!(map.cpu_of(0), None);
        assert!(map.is_empty());
    }

    #[test]
    #[should_panic(expected = "CPU index 0 already registered")]
    fn duplicate_cpu_panics() {
        let mut map = AffinityMap::new();
        map.register(0, 0x0000_0000);
        map.register(0, 0x0000_0001);
    }
}
