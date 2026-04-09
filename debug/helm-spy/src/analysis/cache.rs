#[cfg(feature = "instrumentation")]
use crate::trigger::Gate;
#[cfg(feature = "instrumentation")]
use std::sync::Arc;
use std::sync::Mutex;

struct CacheState {
    sets: usize,
    ways: usize,
    line_size: usize,
    tags: Vec<Vec<u64>>, // [set][way]
    lru: Vec<Vec<u32>>,  // [set][way] = LRU counter (higher = more recently used)
    hits: u64,
    misses: u64,
    clock: u32, // monotonic counter for LRU
}

/// LRU set-associative cache model.
/// Uses Mutex<CacheState> -- the lock is acceptable since cache simulation
/// is per-quantum (not truly hot path).
pub struct CacheModel {
    name: String,
    state: Mutex<CacheState>,
}

impl CacheModel {
    pub fn new(name: &str, size_bytes: usize, ways: usize, line_size: usize) -> Self {
        assert!(line_size.is_power_of_two(), "line_size must be power of 2");
        assert!(ways > 0, "ways must be > 0");
        let sets = size_bytes / (ways * line_size);
        assert!(sets > 0, "cache must have at least 1 set");

        let tags = vec![vec![u64::MAX; ways]; sets];
        let lru = vec![vec![0u32; ways]; sets];

        Self {
            name: name.to_string(),
            state: Mutex::new(CacheState {
                sets,
                ways,
                line_size,
                tags,
                lru,
                hits: 0,
                misses: 0,
                clock: 0,
            }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Simulate a cache access at the given address.
    pub fn access(&self, addr: u64) {
        let mut s = self.state.lock().unwrap();
        let line_addr = addr / s.line_size as u64;
        let set_idx = (line_addr as usize) % s.sets;
        let tag = line_addr / s.sets as u64;

        s.clock += 1;
        let clock = s.clock;

        // Check for hit
        for w in 0..s.ways {
            if s.tags[set_idx][w] == tag {
                s.hits += 1;
                s.lru[set_idx][w] = clock;
                return;
            }
        }

        // Miss: find LRU way (lowest counter)
        s.misses += 1;
        let mut lru_way = 0;
        let mut lru_val = s.lru[set_idx][0];
        for w in 1..s.ways {
            if s.lru[set_idx][w] < lru_val {
                lru_val = s.lru[set_idx][w];
                lru_way = w;
            }
        }
        s.tags[set_idx][lru_way] = tag;
        s.lru[set_idx][lru_way] = clock;
    }

    /// Subscribe to mem probe events — updates the cache on every data access.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_to_mem(self: &Arc<Self>, probes: &mut helm_probe::CpuProbes) {
        let c = Arc::clone(self);
        probes
            .mem
            .subscribe(move |ev: &helm_probe::MemAccessEvent| {
                c.access(ev.addr);
            });
    }

    /// Subscribe gated by a Gate — only processes accesses while gate is armed.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe_to_mem_gated(
        self: &Arc<Self>,
        probes: &mut helm_probe::CpuProbes,
        gate: Gate,
    ) {
        let c = Arc::clone(self);
        probes
            .mem
            .subscribe(move |ev: &helm_probe::MemAccessEvent| {
                if gate.load(std::sync::atomic::Ordering::Relaxed) {
                    c.access(ev.addr);
                }
            });
    }

    pub fn hit_rate(&self) -> f64 {
        let s = self.state.lock().unwrap();
        let total = s.hits + s.misses;
        if total == 0 {
            0.0
        } else {
            s.hits as f64 / total as f64
        }
    }

    pub fn hits(&self) -> u64 {
        self.state.lock().unwrap().hits
    }

    pub fn misses(&self) -> u64 {
        self.state.lock().unwrap().misses
    }

    /// Misses per kilo-instruction.
    pub fn mpki(&self, insn_count: u64) -> f64 {
        if insn_count == 0 {
            return 0.0;
        }
        self.misses() as f64 / insn_count as f64 * 1000.0
    }

    pub fn reset(&self) {
        let mut s = self.state.lock().unwrap();
        for set in &mut s.tags {
            for tag in set.iter_mut() {
                *tag = u64::MAX;
            }
        }
        for set in &mut s.lru {
            for counter in set.iter_mut() {
                *counter = 0;
            }
        }
        s.hits = 0;
        s.misses = 0;
        s.clock = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_on_second_access() {
        // 1KB cache, 2-way, 64-byte lines -> 8 sets
        let cache = CacheModel::new("L1D", 1024, 2, 64);

        cache.access(0x1000); // miss
        cache.access(0x1000); // hit (same line)

        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert!((cache.hit_rate() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn cache_miss_on_new_address() {
        let cache = CacheModel::new("L1D", 1024, 2, 64);

        // Access different cache lines
        cache.access(0x1000);
        cache.access(0x2000);
        cache.access(0x3000);

        assert_eq!(cache.misses(), 3);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn cache_same_line_hits() {
        let cache = CacheModel::new("L1D", 1024, 2, 64);

        // All within the same 64-byte line (0x1000..0x103F)
        cache.access(0x1000);
        cache.access(0x1010);
        cache.access(0x103F);

        // First is miss, rest are hits
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 2);
    }

    #[test]
    fn cache_lru_eviction() {
        // Tiny cache: 128 bytes, 2-way, 64-byte lines -> 1 set
        let cache = CacheModel::new("tiny", 128, 2, 64);

        cache.access(0x0000); // miss, set 0, way 0
        cache.access(0x1000); // miss, set 0, way 1 (different tag, same set)
        cache.access(0x0000); // hit (still in way 0)
        cache.access(0x2000); // miss, evicts LRU (way 1, which had 0x1000)
        cache.access(0x1000); // miss (was evicted)

        assert_eq!(cache.misses(), 4);
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn cache_mpki() {
        let cache = CacheModel::new("mpki", 1024, 2, 64);
        cache.access(0x1000);
        cache.access(0x2000);
        cache.access(0x3000);
        // 3 misses
        let mpki = cache.mpki(1000);
        assert!((mpki - 3.0).abs() < 1e-10);
    }

    #[test]
    fn cache_hit_rate_empty() {
        let cache = CacheModel::new("empty", 1024, 2, 64);
        assert_eq!(cache.hit_rate(), 0.0);
    }

    #[test]
    fn cache_reset() {
        let cache = CacheModel::new("reset", 1024, 2, 64);
        cache.access(0x1000);
        cache.access(0x1000);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);

        cache.reset();
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 0);

        // After reset, same address should miss again
        cache.access(0x1000);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
    }
}
