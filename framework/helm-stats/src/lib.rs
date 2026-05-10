//! `helm-stats` -- gem5-style performance counters, histograms, label
//! counters, and a hierarchical registry, with feature-gated zero-cost
//! interfaces.
//!
//! # Feature model
//!
//! All stats features are **default-off**. A plain `cargo build
//! --release` produces a binary with zero stats storage and zero
//! call-site cost: every type in the public surface is a ZST and every
//! method is `#[inline(always)]` empty.
//!
//! Dev / profiling builds opt in via `--features=stats` (or via
//! aggregate `dev-instrumentation` / `profiling` features on
//! `helm-cli`, which forward to this crate).
//!
//! See `docs/design/helm-stats/HLD.md` and the research note at
//! `docs/research/gem5-stats-helm-adaptation.md`.

#![allow(clippy::module_name_repetitions)]
#![allow(missing_docs)]

mod counter;
mod cpu;
mod formula;
mod histogram;
mod intc;
mod io;
mod iommu;
mod jit;
mod label;
mod mem;
mod producer;
mod registry;
mod rtc;

pub use counter::PerfCounter;
pub use cpu::CpuStats;
pub use formula::PerfFormula;
pub use histogram::PerfHistogram;
pub use intc::IntcStats;
pub use io::IoStats;
pub use iommu::IommuStats;
pub use jit::JitPerfStats;
pub use label::LabelCounter;
pub use mem::MemStats;
pub use producer::{StatsProducer, StatsScope};
pub use registry::{StatsRegistry, StatsRegistryRead};
pub use rtc::RtcStats;
