use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use crate::api::{HelmPlugin, PluginArgs};
use crate::runtime::PluginRegistry;
use crate::runtime::MemFilter;

/// Parse a size string like "32KB", "8MB", "64" (bytes).
fn parse_size(s: &str) -> usize {
    let s = s.trim();
    if let Some(n) = s.strip_suffix("KB").or_else(|| s.strip_suffix("kb")) {
        n.trim().parse::<usize>().unwrap_or(32) * 1024
    } else if let Some(n) = s.strip_suffix("MB").or_else(|| s.strip_suffix("mb")) {
        n.trim().parse::<usize>().unwrap_or(1) * 1024 * 1024
    } else {
        s.parse::<usize>().unwrap_or(32 * 1024)
    }
}

/// LRU set-associative cache state, shared between the plugin and the callback closure.
struct CacheState {
    /// Each entry is one set; each set is a Vec of `assoc` tag slots.
    /// Position 0 = MRU, last position = LRU.
    sets: Vec<Vec<u64>>,
    assoc: usize,
    num_sets: usize,
    line_bits: u32,
    set_bits: u32,
}

impl CacheState {
    fn new(total_size: usize, assoc: usize, line_size: usize) -> Self {
        let line_size = line_size.next_power_of_two().max(1);
        let num_sets = (total_size / (assoc * line_size)).next_power_of_two().max(1);
        let line_bits = line_size.trailing_zeros();
        let set_bits = num_sets.trailing_zeros();
        CacheState {
            sets: vec![Vec::with_capacity(assoc); num_sets],
            assoc,
            num_sets,
            line_bits,
            set_bits,
        }
    }

    /// Returns true on hit, false on miss.  Updates LRU state.
    fn access(&mut self, vaddr: u64) -> bool {
        let set_idx = ((vaddr >> self.line_bits) as usize) & (self.num_sets - 1);
        let tag = vaddr >> (self.line_bits + self.set_bits);
        let set = &mut self.sets[set_idx];

        // Search for tag (hit path).
        if let Some(pos) = set.iter().position(|&t| t == tag) {
            // Move to MRU position (front).
            set.remove(pos);
            set.insert(0, tag);
            return true;
        }

        // Miss path — evict LRU if needed.
        if set.len() >= self.assoc {
            set.pop(); // LRU is at the back
        }
        set.insert(0, tag);
        false
    }
}

/// Set-associative cache simulator.
pub struct CacheSim {
    hits:   Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    state:  Arc<Mutex<CacheState>>,
}

impl CacheSim {
    pub fn new() -> Self {
        Self {
            hits:   Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            state:  Arc::new(Mutex::new(CacheState::new(32 * 1024, 8, 64))),
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    pub fn hit_rate(&self) -> f64 {
        let h = self.hits() as f64;
        let m = self.misses() as f64;
        let total = h + m;
        if total == 0.0 { 0.0 } else { h / total }
    }
}

impl Default for CacheSim {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for CacheSim {
    fn name(&self) -> &str {
        "cache_sim"
    }

    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        let l1d_size  = parse_size(args.get_or("l1d_size",  "32KB"));
        let l1d_assoc = args.get_usize("l1d_assoc").unwrap_or(8).max(1);
        let l1d_line  = parse_size(args.get_or("l1d_line", "64"));

        self.state = Arc::new(Mutex::new(CacheState::new(l1d_size, l1d_assoc, l1d_line)));

        let state  = Arc::clone(&self.state);
        let hits   = Arc::clone(&self.hits);
        let misses = Arc::clone(&self.misses);

        reg.on_mem_access(MemFilter::All, Box::new(move |_vcpu_idx, mem_info| {
            let hit = state.lock().unwrap().access(mem_info.vaddr);
            if hit {
                hits.fetch_add(1, Ordering::Relaxed);
            } else {
                misses.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    fn atexit(&mut self) {
        let h = self.hits();
        let m = self.misses();
        let total = h + m;
        let rate = self.hit_rate() * 100.0;
        log::info!("[cache_sim] L1D: total={} hits={} misses={} hit_rate={:.2}%", total, h, m, rate);
    }
}
