#![allow(missing_docs)]

//! `helm-debug` — GDB RSP stub and checkpoint manager.
//!
//! Diagnostic macros (`sim_stub!`, `sim_warn!`, `sim_info!`) have moved to
//! `helm-diag`. This crate re-exports them at its root so legacy import paths
//! continue to compile during migration.
//!
//! # Phase 2 (planned)
//! - GDB RSP over TCP
//! - CheckpointManager: CBOR serialization

// Re-export helm-diag diagnostic macros so `use helm_debug::{sim_stub, …}`
// keeps working for any call sites not yet migrated.
pub use helm_diag::{sim_stub, sim_warn, sim_info};

use helm_core::AttrRegistry;
// ── CheckpointManager ─────────────────────────────────────────────────────────
/// Saves and restores architectural state via `AttrRegistry`.
///
/// Checkpoint format: CBOR (Phase 2). Stub returns empty bytes for now.
#[derive(Default)]
pub struct CheckpointManager;
impl CheckpointManager {
    pub fn new() -> Self { Self }
    /// Serialize all registered attributes to bytes.
    pub fn save(&self, _registry: &AttrRegistry) -> Vec<u8> {
        // TODO(phase-2): serialize to CBOR
        Vec::new()
    }
    /// Restore attributes from previously saved bytes.
    pub fn restore(&self, _registry: &mut AttrRegistry, _data: &[u8]) -> Result<(), DebugError> {
        // TODO(phase-2): deserialize from CBOR
        Ok(())
    }
}
// ── TraceLogger ───────────────────────────────────────────────────────────────
// ── GdbServer ─────────────────────────────────────────────────────────────────
/// GDB Remote Serial Protocol server.
///
/// Listens on TCP, accepts one client, dispatches RSP packets to the engine.
pub struct GdbServer {
    port: u16,
}
impl GdbServer {
    pub fn new(port: u16) -> Self { Self { port } }
    /// Start listening. Blocks until a client connects.
    pub fn listen(&self) -> Result<(), DebugError> {
        // TODO(phase-2): bind TCP, RSP handshake, packet loop
        let _ = self.port;
        Err(DebugError::NotImplemented)
    }
}
// ── Errors ────────────────────────────────────────────────────────────────────
#[derive(Debug, thiserror::Error)]
pub enum DebugError {
    #[error("not yet implemented")]
    NotImplemented,
    #[error("checkpoint data corrupt or version mismatch")]
    CorruptCheckpoint,
    #[error("GDB RSP error: {msg}")]
    Rsp { msg: String },
}
