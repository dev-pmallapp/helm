//! Set-associative cache model.

use helm_core::config::CacheConfig;
use helm_core::types::Addr;

#[derive(Debug, Clone)]
pub struct CacheLine {
    pub tag: u64,
    pub valid: bool,
    pub dirty: bool,
}

pub struct CacheSet {
    pub lines: Vec<CacheLine>,
}

pub struct Cache {
    pub sets: Vec<CacheSet>,
    pub associativity: u32,
    pub line_size: u32,
    pub latency: u64,
    num_sets: usize,
}

/// Result of a cache access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAccessResult {
    Hit,
    Miss,
}

impl Cache {
    pub fn from_config(cfg: &CacheConfig) -> Self {
        let total_bytes = parse_size(&cfg.size);
        let line_size = cfg.line_size;
        let assoc = cfg.associativity;
        let num_sets = (total_bytes / (line_size * assoc) as u64) as usize;

        let sets = (0..num_sets)
            .map(|_| CacheSet {
                lines: (0..assoc)
                    .map(|_| CacheLine {
                        tag: 0,
                        valid: false,
                        dirty: false,
                    })
                    .collect(),
            })
            .collect();

        Self {
            sets,
            associativity: assoc,
            line_size,
            latency: cfg.latency_cycles,
            num_sets,
        }
    }

    /// Probe the cache for an address. Returns hit/miss.
    pub fn access(&mut self, addr: Addr, _is_write: bool) -> CacheAccessResult {
        let offset_bits = (self.line_size as f64).log2() as u32;
        let set_idx = ((addr >> offset_bits) as usize) % self.num_sets;
        let tag = addr >> (offset_bits + (self.num_sets as f64).log2() as u32);

        let set = &mut self.sets[set_idx];
        for line in &set.lines {
            if line.valid && line.tag == tag {
                return CacheAccessResult::Hit;
            }
        }
        // Miss — simple LRU stub: replace the first invalid or the last line.
        let victim = set
            .lines
            .iter()
            .position(|l| !l.valid)
            .unwrap_or(set.lines.len() - 1);
        set.lines[victim] = CacheLine {
            tag,
            valid: true,
            dirty: false,
        };
        CacheAccessResult::Miss
    }
}

pub(crate) fn parse_size(s: &str) -> u64 {
    let s = s.trim().to_uppercase();
    if let Some(num) = s.strip_suffix("KB") {
        num.trim().parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(num) = s.strip_suffix("MB") {
        num.trim().parse::<u64>().unwrap_or(0) * 1024 * 1024
    } else if let Some(num) = s.strip_suffix("GB") {
        num.trim().parse::<u64>().unwrap_or(0) * 1024 * 1024 * 1024
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}
