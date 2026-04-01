//! `helm-spy` -- Analysis primitives for the helm-ng instrumentation stack.
// Internal analysis crate — docs and unsafe-code warnings suppressed at crate level.
// unsafe_code: TraceRing uses UnsafeCell + raw ptr for its SPSC lock-free ring buffer.
#![allow(missing_docs, unsafe_code)]
//!
//! This crate provides the collection layer (Layer 2) of the helm-ng
//! instrumentation architecture. It contains typed, lock-free (where possible)
//! analysis primitives that accumulate performance data during simulation.
//!
//! # Design Principles
//!
//! 1. **Collection is not delivery** -- this crate has no formatting, no I/O,
//!    and no dependency on `helm-report`. Data is collected here; delivery
//!    happens elsewhere.
//!
//! 2. **No heap allocation per event** in the hot path. Counters use
//!    `AtomicU64::fetch_add(Relaxed)`, histograms use `partition_point` +
//!    atomic add, heatmaps use `DashMap` shard locks.
//!
//! 3. **No Mutex in the hot loop** -- only `RingBuffer` and `EventStream`
//!    use Mutex, and they are explicitly documented as bounded-overhead
//!    primitives for low-rate events.
//!
//! # Modules
//!
//! - [`events`] -- Event types (`InsnInfo`, `BranchInfo`, `MemInfo`, etc.)
//! - [`primitives`] -- Collection primitives (`Counter`, `Histogram`, `HeatMap`, etc.)
//! - [`analysis`] -- Analysis models (`InsnMix`, `CacheModel`, `BranchPredictor`)
//! - [`trigger`] -- Conditional trigger system
//! - [`window`] -- Time-window gating for observation
//! - [`quantum`] -- `QuantumObserver` trait for quantum-boundary flush
//! - [`session`] -- `HelmSpy` aggregator

pub mod analysis;
pub mod bridge;
pub mod events;
pub mod primitives;
pub mod quantum;
pub mod session;
pub mod trigger;
pub mod window;

pub use bridge::ProbePluginBridge;
pub use trigger::{new_gate, Gate, Trigger, TriggerKind};
pub use window::Window;
