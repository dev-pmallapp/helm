//! `helm-debug` — GDB RSP stub, trace logger, and checkpoint manager.
//!
//! # Phase 0
//! Stubs only — no actual TCP listener or checkpoint serialisation.
//!
//! # Phase 2
//! - GDB RSP over TCP (port 1234 default)
//! - TraceLogger: `HelmEventBus` subscriber → `.jsonl` output
//! - CheckpointManager: serialize all `AttrRegistry` values to CBOR

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

/// Writes structured trace events to a `.jsonl` file.
///
/// Subscribes to `HelmEventBus` events in Phase 2.
pub struct TraceLogger {
    // TODO(phase-2): BufWriter<File> + event filter
}

impl TraceLogger {
    pub fn new() -> Self { Self {} }

    /// Log a named event with a u64 value.
    pub fn log(&mut self, _event: &str, _val: u64) {
        // TODO(phase-2): write JSON line
    }
}

impl Default for TraceLogger {
    fn default() -> Self { Self::new() }
}

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
