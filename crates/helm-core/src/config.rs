//! Runtime configuration structures, typically populated from the Python layer.

use crate::types::{ExecMode, IsaKind};
use serde::{Deserialize, Serialize};

/// Top-level platform configuration, mirroring the Python `Platform` class.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformConfig {
    pub name: String,
    pub isa: IsaKind,
    pub exec_mode: ExecMode,
    pub cores: Vec<CoreConfig>,
    pub memory: MemoryConfig,
}

/// Per-core microarchitectural parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    pub name: String,
    pub width: u32,
    pub rob_size: u32,
    pub iq_size: u32,
    pub lq_size: u32,
    pub sq_size: u32,
    pub branch_predictor: BranchPredictorConfig,
}

/// Branch-predictor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BranchPredictorConfig {
    Static,
    Bimodal { table_size: u32 },
    GShare { history_bits: u32 },
    TAGE { history_length: u32 },
    Tournament,
}

/// Memory-hierarchy description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub l1i: Option<CacheConfig>,
    pub l1d: Option<CacheConfig>,
    pub l2: Option<CacheConfig>,
    pub l3: Option<CacheConfig>,
    pub dram_latency_cycles: u64,
}

/// Single cache-level parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub size: String,
    pub associativity: u32,
    pub latency_cycles: u64,
    pub line_size: u32,
}
