# helm-timing — LLD: MicroarchProfile

## Overview

`MicroarchProfile` is an immutable configuration object that parameterizes all three timing models. It is loaded from a JSON file once at simulation startup and then frozen; no field can be changed at runtime (Q48). All timing models hold an `Arc<MicroarchProfile>`, ensuring zero-copy sharing.

---

## Top-Level Struct

```rust
use std::collections::HashMap;
use serde::Deserialize;

/// Immutable microarchitecture configuration for a target core.
/// Loaded from JSON; private fields with public getters enforce immutability.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicroarchProfile {
    /// Human-readable identifier, e.g., "cortex-a72" or "generic-ooo".
    pub(crate) name: String,

    /// Human-readable description.
    pub(crate) description: String,

    // ── Virtual mode parameters ──────────────────────────────────────────
    /// Instructions-per-cycle estimate used by Virtual timing mode.
    /// Default: 1.0. Must be > 0.0.
    pub(crate) virtual_ipc: f64,

    /// Instruction count between EventQueue drain calls in Virtual mode.
    /// Default: 10_000.
    pub(crate) virtual_interval_insns: u64,

    // ── Interval mode parameters ─────────────────────────────────────────
    /// Instruction count per Interval simulation window. Default: 10_000.
    pub(crate) interval_insns: u64,

    // ── Branch predictor ─────────────────────────────────────────────────
    pub(crate) bp: BpConfig,

    // ── Functional unit latencies ────────────────────────────────────────
    /// Latency in cycles per functional unit class.
    /// Used by Interval (OoOWindow) and Accurate (EX stage).
    pub(crate) fu_latencies: HashMap<FuClass, u8>,

    // ── Cache hierarchy ──────────────────────────────────────────────────
    pub(crate) l1i: CacheConfig,
    pub(crate) l1d: CacheConfig,
    pub(crate) l2: CacheConfig,
    pub(crate) l3: Option<CacheConfig>,

    /// Cycles penalty for an L1 miss (goes to L2 or further).
    /// Virtual and Interval modes use this directly.
    pub(crate) l1_miss_penalty_cycles: u8,

    /// Cycles penalty for a full memory access (all caches miss, goes to DRAM).
    pub(crate) mem_access_penalty_cycles: u16,

    // ── TLB ──────────────────────────────────────────────────────────────
    pub(crate) itlb: TlbConfig,
    pub(crate) dtlb: TlbConfig,
    pub(crate) l2_tlb: Option<TlbConfig>,

    // ── Prefetch ─────────────────────────────────────────────────────────
    pub(crate) prefetch: PrefetchConfig,
}
```

### Public Getter Methods

Fields are `pub(crate)` with public getters to enforce immutability from outside the crate and from Python (Q48).

```rust
impl MicroarchProfile {
    pub fn name(&self) -> &str { &self.name }
    pub fn description(&self) -> &str { &self.description }
    pub fn virtual_ipc(&self) -> f64 { self.virtual_ipc }
    pub fn virtual_interval_insns(&self) -> u64 { self.virtual_interval_insns }
    pub fn interval_insns(&self) -> u64 { self.interval_insns }
    pub fn bp(&self) -> &BpConfig { &self.bp }
    pub fn l1i(&self) -> &CacheConfig { &self.l1i }
    pub fn l1d(&self) -> &CacheConfig { &self.l1d }
    pub fn l2(&self) -> &CacheConfig { &self.l2 }
    pub fn l3(&self) -> Option<&CacheConfig> { self.l3.as_ref() }
    pub fn l1_miss_penalty_cycles(&self) -> u8 { self.l1_miss_penalty_cycles }
    pub fn mem_access_penalty_cycles(&self) -> u16 { self.mem_access_penalty_cycles }
    pub fn itlb(&self) -> &TlbConfig { &self.itlb }
    pub fn dtlb(&self) -> &TlbConfig { &self.dtlb }
    pub fn l2_tlb(&self) -> Option<&TlbConfig> { self.l2_tlb.as_ref() }
    pub fn prefetch(&self) -> &PrefetchConfig { &self.prefetch }
    pub fn branch_mispredict_penalty_cycles(&self) -> u8 {
        self.bp.mispredict_penalty_cycles
    }

    /// Returns the execution latency for a given functional unit class.
    /// Defaults to 1 cycle if not specified in the profile.
    pub fn fu_latency(&self, fu: FuClass) -> u8 {
        *self.fu_latencies.get(&fu).unwrap_or(&1)
    }

    /// Construct from a JSON string (e.g., from include_str! or a file).
    pub fn from_json(json: &str) -> Result<Self, ProfileError> {
        let profile: MicroarchProfile = serde_json::from_str(json)
            .map_err(ProfileError::Parse)?;
        profile.validate()?;
        Ok(profile)
    }

    /// Load from a file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self, ProfileError> {
        let json = std::fs::read_to_string(path)
            .map_err(ProfileError::Io)?;
        Self::from_json(&json)
    }

    fn validate(&self) -> Result<(), ProfileError> {
        if self.virtual_ipc <= 0.0 {
            return Err(ProfileError::Validation("virtual_ipc must be > 0.0".into()));
        }
        if self.interval_insns == 0 {
            return Err(ProfileError::Validation("interval_insns must be > 0".into()));
        }
        Ok(())
    }
}
```

---

## `CacheConfig`

```rust
/// Configuration for one cache level.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Total cache size in bytes. Must be a power of 2.
    pub size_bytes: usize,

    /// Number of cache ways (associativity). Must be a power of 2.
    pub ways: usize,

    /// Cache line size in bytes. Typically 64.
    pub line_size_bytes: usize,

    /// Access latency in cycles on a hit.
    pub hit_latency_cycles: u8,

    /// Replacement policy.
    pub replacement: ReplacementPolicy,

    /// Write policy.
    pub write_policy: WritePolicy,

    /// Number of MSHRs (miss status holding registers).
    /// Limits outstanding misses. Typical: 4–16.
    pub mshr_count: u8,
}

impl CacheConfig {
    /// Number of sets = size / (ways * line_size).
    pub fn num_sets(&self) -> usize {
        self.size_bytes / (self.ways * self.line_size_bytes)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementPolicy {
    Lru,
    Plru,
    Random,
    Lfu,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WritePolicy {
    WriteBack,
    WriteThrough,
}
```

---

## `TlbConfig`

```rust
/// Configuration for one TLB.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TlbConfig {
    /// Number of TLB entries.
    pub entries: usize,

    /// Associativity. Use `entries` for fully associative.
    pub ways: usize,

    /// Access latency in cycles on a hit.
    pub hit_latency_cycles: u8,

    /// Page table walk penalty on a miss (cycles to walk the page table).
    pub miss_penalty_cycles: u16,

    /// ASID width in bits. 0 = no ASID support.
    pub asid_bits: u8,
}
```

---

## `BpConfig`

```rust
/// Branch predictor configuration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BpConfig {
    /// Branch predictor type.
    pub predictor: BranchPredictorKind,

    /// Misprediction penalty in cycles. Used directly by all timing models.
    pub mispredict_penalty_cycles: u8,

    /// BTB (Branch Target Buffer) entries.
    pub btb_entries: usize,

    /// RAS (Return Address Stack) depth.
    pub ras_depth: usize,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BranchPredictorKind {
    /// Always predict not-taken.
    Static,
    /// Bimodal 2-bit saturating counter table.
    Bimodal,
    /// GSHARE (global history XOR PC).
    Gshare,
    /// TAGE (tagged geometric history).
    Tage,
    /// No predictor: always mispredict (worst case analysis).
    None,
}
```

---

## `PrefetchConfig`

```rust
/// Hardware prefetcher configuration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PrefetchConfig {
    /// Whether hardware prefetching is modeled.
    pub enabled: bool,

    /// Prefetch distance in cache lines.
    pub distance: u8,

    /// Degree (how many lines to prefetch ahead).
    pub degree: u8,
}
```

---

## Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Validation error: {0}")]
    Validation(String),
}
```

---

## Shipped Profiles

Profiles are embedded as compile-time strings via `include_str!` and also written to `$INSTALL_DIR/profiles/` during installation.

```rust
pub mod profiles {
    pub const GENERIC_INORDER: &str =
        include_str!("../profiles/generic-inorder.json");
    pub const GENERIC_OOO: &str =
        include_str!("../profiles/generic-ooo.json");
    pub const SIFIVE_U74: &str =
        include_str!("../profiles/sifive-u74.json");
    pub const CORTEX_A72: &str =
        include_str!("../profiles/cortex-a72.json");

    /// Returns a profile by short name.
    pub fn builtin(name: &str) -> Option<&'static str> {
        match name {
            "generic-inorder" => Some(GENERIC_INORDER),
            "generic-ooo"     => Some(GENERIC_OOO),
            "sifive-u74"      => Some(SIFIVE_U74),
            "cortex-a72"      => Some(CORTEX_A72),
            _ => None,
        }
    }
}
```

---

## JSON Schema — `cortex-a72.json` Example

```json
{
  "name": "cortex-a72",
  "description": "Arm Cortex-A72 (ARMv8-A, 3-wide OoO, 4.7 GHz), representative of Raspberry Pi 4.",

  "virtual_ipc": 2.4,
  "virtual_interval_insns": 10000,
  "interval_insns": 10000,

  "bp": {
    "predictor": "tage",
    "mispredict_penalty_cycles": 15,
    "btb_entries": 4096,
    "ras_depth": 8
  },

  "fu_latencies": {
    "INT":    1,
    "BRANCH": 1,
    "MUL":    3,
    "DIV":    12,
    "FP":     4,
    "LOAD":   4,
    "STORE":  1,
    "CSR":    1,
    "ATOMIC": 5,
    "FENCE":  1
  },

  "l1i": {
    "size_bytes": 49152,
    "ways": 3,
    "line_size_bytes": 64,
    "hit_latency_cycles": 3,
    "replacement": "plru",
    "write_policy": "write_back",
    "mshr_count": 8
  },

  "l1d": {
    "size_bytes": 32768,
    "ways": 2,
    "line_size_bytes": 64,
    "hit_latency_cycles": 4,
    "replacement": "plru",
    "write_policy": "write_back",
    "mshr_count": 8
  },

  "l2": {
    "size_bytes": 1048576,
    "ways": 16,
    "line_size_bytes": 64,
    "hit_latency_cycles": 16,
    "replacement": "plru",
    "write_policy": "write_back",
    "mshr_count": 16
  },

  "l3": null,

  "l1_miss_penalty_cycles": 12,
  "mem_access_penalty_cycles": 160,

  "itlb": {
    "entries": 48,
    "ways": 48,
    "hit_latency_cycles": 1,
    "miss_penalty_cycles": 50,
    "asid_bits": 16
  },

  "dtlb": {
    "entries": 32,
    "ways": 32,
    "hit_latency_cycles": 1,
    "miss_penalty_cycles": 50,
    "asid_bits": 16
  },

  "l2_tlb": {
    "entries": 1024,
    "ways": 4,
    "hit_latency_cycles": 5,
    "miss_penalty_cycles": 50,
    "asid_bits": 16
  },

  "prefetch": {
    "enabled": true,
    "distance": 8,
    "degree": 2
  }
}
```

---

## `helm validate` CLI Design

`helm validate` compares a simulation run against pre-collected hardware performance counter data (Q50). No live hardware access is needed; the reference data is a JSON file collected in advance with `perf stat -j` or vendor-specific tools.

### Command

```
helm validate \
    --profile sifive-u74.json \
    --counters reference/dhrystone-u74.json \
    --workload dhrystone \
    --timing interval \
    [--tolerance 0.05]
```

### Reference Counter Format (`reference/dhrystone-u74.json`)

```json
{
  "platform": "SiFive HiFive Unmatched (U74-MC)",
  "workload": "dhrystone",
  "counters": {
    "instructions": 2147483648,
    "cycles": 1073741824,
    "cache-misses": 1048576,
    "branch-misses": 2097152,
    "ipc": 2.0
  }
}
```

### Validation Algorithm

```rust
pub struct ValidationReport {
    pub workload: String,
    pub profile: String,
    pub timing_mode: String,
    pub counters: Vec<CounterComparison>,
    pub passed: bool,
}

pub struct CounterComparison {
    pub name: String,
    pub simulated: f64,
    pub reference: f64,
    pub error_pct: f64,
    pub within_tolerance: bool,
}

pub fn validate(
    profile: &MicroarchProfile,
    reference: &ReferenceCounters,
    tolerance: f64,
) -> ValidationReport {
    // Run the workload in simulation, collect counters from helm-stats.
    // Compare each counter: error_pct = |sim - ref| / ref * 100.
    // passed = all counters within tolerance (default 5%).
    todo!("Phase 0 stub: run workload, collect, compare")
}
```

### Output

```
helm validate report — cortex-a72 / dhrystone / interval mode
┌─────────────────────┬────────────────┬────────────────┬──────────┬────────┐
│ Counter             │ Simulated      │ Reference      │ Error %  │ Pass?  │
├─────────────────────┼────────────────┼────────────────┼──────────┼────────┤
│ instructions        │ 2,147,483,648  │ 2,147,483,648  │   0.00%  │ ✓      │
│ cycles              │ 1,120,000,000  │ 1,073,741,824  │   4.31%  │ ✓      │
│ ipc                 │ 1.92           │ 2.00           │   4.00%  │ ✓      │
│ cache-misses        │ 1,150,000      │ 1,048,576      │   9.67%  │ ✗ >5%  │
│ branch-misses       │ 2,050,000      │ 2,097,152      │   2.25%  │ ✓      │
└─────────────────────┴────────────────┴────────────────┴──────────┴────────┘
Result: FAIL (1 counter out of tolerance)
```

### CLI Integration

`helm validate` is a subcommand of the `helm` binary (in the `helm-cli` crate). It:
1. Loads the profile with `MicroarchProfile::from_file`.
2. Loads the reference counters from the JSON file.
3. Instantiates `HelmEngine<IntervalTimed>` (or `Virtual`/`Accurate` per `--timing`).
4. Runs the workload ELF binary via `helm-engine/se`.
5. Collects counters from `helm-stats`.
6. Calls `validate()` and prints the report.
7. Exits 0 on pass, 1 on fail (for CI integration).
