//! Translation Lookaside Buffer (stub).

use helm_core::types::Addr;
use std::collections::HashMap;

pub struct TlbEntry {
    pub vpn: u64,
    pub ppn: u64,
    pub valid: bool,
}

pub struct Tlb {
    entries: HashMap<u64, TlbEntry>,
    capacity: usize,
}

impl Tlb {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            capacity,
        }
    }

    pub fn lookup(&self, vaddr: Addr) -> Option<Addr> {
        let vpn = vaddr >> 12;
        self.entries
            .get(&vpn)
            .map(|e| (e.ppn << 12) | (vaddr & 0xFFF))
    }

    pub fn insert(&mut self, vaddr: Addr, paddr: Addr) {
        let vpn = vaddr >> 12;
        let ppn = paddr >> 12;
        if self.entries.len() >= self.capacity {
            // Evict first entry (naive).
            if let Some(&key) = self.entries.keys().next() {
                self.entries.remove(&key);
            }
        }
        self.entries.insert(
            vpn,
            TlbEntry {
                vpn,
                ppn,
                valid: true,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_miss_returns_none() {
        let tlb = Tlb::new(16);
        assert!(tlb.lookup(0x1000).is_none());
    }

    #[test]
    fn insert_then_lookup_returns_translated_addr() {
        let mut tlb = Tlb::new(16);
        tlb.insert(0x0000_1000, 0x0080_1000);
        let pa = tlb.lookup(0x0000_1ABC);
        // Same page, offset 0xABC should be preserved.
        assert_eq!(pa, Some(0x0080_1ABC));
    }

    #[test]
    fn eviction_occurs_at_capacity() {
        let mut tlb = Tlb::new(2);
        tlb.insert(0x1000, 0xA000);
        tlb.insert(0x2000, 0xB000);
        tlb.insert(0x3000, 0xC000); // should evict one
                                    // At least the newest should be present.
        assert!(tlb.lookup(0x3000).is_some());
    }
}
