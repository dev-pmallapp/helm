//! # helm-memory
//!
//! Models the memory hierarchy: multi-level caches (L1i/L1d/L2/L3),
//! a simple coherence protocol, TLB, and DRAM timing.

pub mod address_space;
pub mod cache;
pub mod coherence;
mod exec_mem_impl;
pub mod flat;
pub mod mmu;
pub mod tlb;

use helm_core::config::MemoryConfig;

/// Assembled memory subsystem for a single core (or shared levels).
pub struct MemorySubsystem {
    pub config: MemoryConfig,
    pub l1i: Option<cache::Cache>,
    pub l1d: Option<cache::Cache>,
    pub l2: Option<cache::Cache>,
    pub l3: Option<cache::Cache>,
    pub dram_latency: u64,
}

impl MemorySubsystem {
    pub fn from_config(config: MemoryConfig) -> Self {
        let l1i = config.l1i.as_ref().map(cache::Cache::from_config);
        let l1d = config.l1d.as_ref().map(cache::Cache::from_config);
        let l2 = config.l2.as_ref().map(cache::Cache::from_config);
        let l3 = config.l3.as_ref().map(cache::Cache::from_config);
        let dram_latency = config.dram_latency_cycles;
        Self {
            config,
            l1i,
            l1d,
            l2,
            l3,
            dram_latency,
        }
    }
}

#[cfg(test)]
mod tests;
