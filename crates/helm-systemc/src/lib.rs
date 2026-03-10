//! # helm-systemc
//!
//! Interfaces for co-simulating HELM with SystemC/TLM-2.0 models.
//!
//! This crate defines the Rust-side traits and bridge abstractions.
//! The actual C++ FFI (via `cxx`) is deferred until a SystemC library
//! is linked; everything here is pure Rust so the crate compiles
//! without a C++ toolchain.
//!
//! # Bridge variants
//!
//! | Mode | Type | Overhead |
//! |------|------|----------|
//! | In-process | [`InProcessConfig`] | lowest (1.5-3x) |
//! | Shared memory | [`ShmemConfig`] | medium (3-10x) |
//! | Socket | [`SocketConfig`] | highest (10-50x) |

pub mod bridge;
pub mod clock;
pub mod tlm;

pub use bridge::{BridgeConfig, BridgeMode, SystemCBridge};
pub use clock::ClockDomain;
pub use tlm::{TlmCommand, TlmResponse, TlmTransaction};

#[cfg(test)]
mod tests;
