//! # helm-stats
//!
//! Collects simulation statistics (IPC, branch MPKI, cache hit rates, etc.)
//! and supports serialisation for post-processing.

pub mod collector;
pub mod counters;

pub use collector::StatsCollector;

#[cfg(test)]
mod tests;
