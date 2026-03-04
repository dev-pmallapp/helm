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
