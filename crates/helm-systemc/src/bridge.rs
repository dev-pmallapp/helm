//! Bridge abstraction — routes HELM MMIO to SystemC modules.

use super::tlm::{TlmResponse, TlmTransaction};
use helm_core::HelmResult;

/// Transport mode between HELM and SystemC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMode {
    /// Link with libsystemc.so, call FFI directly (fastest).
    InProcess,
    /// Shared-memory ring buffers between processes.
    SharedMemory,
    /// JSON over Unix socket (most flexible).
    Socket,
}

/// TLM timing style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlmTimingMode {
    /// `b_transport` with temporal decoupling.
    LooselyTimed,
    /// `nb_transport_fw/bw` with phase annotations.
    ApproximatelyTimed,
}

/// Configuration for the SystemC bridge.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub mode: BridgeMode,
    pub timing: TlmTimingMode,
    /// Synchronisation quantum in nanoseconds.
    pub quantum_ns: f64,
    /// CPU clock frequency for cycle/ns conversion.
    pub cpu_frequency_hz: u64,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            mode: BridgeMode::InProcess,
            timing: TlmTimingMode::LooselyTimed,
            quantum_ns: 10_000.0,            // 10 us
            cpu_frequency_hz: 1_000_000_000, // 1 GHz
        }
    }
}

/// Trait that every bridge backend implements.
///
/// This is the Rust-side contract.  The actual C++ FFI, shared-memory
/// transport, or socket transport implements this trait.
pub trait SystemCBridge: Send {
    /// Execute a TLM transaction and return the completed payload.
    fn transact(&mut self, txn: &mut TlmTransaction) -> HelmResult<()>;

    /// Advance the SystemC kernel by one quantum.
    fn sync_quantum(&mut self) -> HelmResult<()>;

    /// Current SystemC simulation time in nanoseconds.
    fn systemc_time_ns(&self) -> f64;
}

/// Stub bridge used when no SystemC library is linked.
/// Returns a successful response with zero delay for every transaction.
pub struct StubBridge {
    time_ns: f64,
    quantum_ns: f64,
}

impl StubBridge {
    pub fn new(config: &BridgeConfig) -> Self {
        Self {
            time_ns: 0.0,
            quantum_ns: config.quantum_ns,
        }
    }
}

impl SystemCBridge for StubBridge {
    fn transact(&mut self, txn: &mut TlmTransaction) -> HelmResult<()> {
        txn.response = TlmResponse::Ok;
        txn.delay_ns = 0.0;
        Ok(())
    }

    fn sync_quantum(&mut self) -> HelmResult<()> {
        self.time_ns += self.quantum_ns;
        Ok(())
    }

    fn systemc_time_ns(&self) -> f64 {
        self.time_ns
    }
}
