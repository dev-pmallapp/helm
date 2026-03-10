//! Rust-side representation of TLM-2.0 generic payload.

use serde::{Deserialize, Serialize};

/// TLM command type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlmCommand {
    Read,
    Write,
    Ignore,
}

/// TLM response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlmResponse {
    Ok,
    IncompleteResponse,
    GenericError,
    AddressError,
    CommandError,
    BurstError,
    ByteEnableError,
}

/// Rust mirror of `tlm_generic_payload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlmTransaction {
    pub address: u64,
    pub command: TlmCommand,
    pub data: Vec<u8>,
    pub byte_enables: Option<Vec<u8>>,
    pub streaming_width: u32,
    pub response: TlmResponse,
    /// Annotated delay in nanoseconds.
    pub delay_ns: f64,
}

impl TlmTransaction {
    pub fn read(address: u64, length: usize) -> Self {
        Self {
            address,
            command: TlmCommand::Read,
            data: vec![0u8; length],
            byte_enables: None,
            streaming_width: length as u32,
            response: TlmResponse::IncompleteResponse,
            delay_ns: 0.0,
        }
    }

    pub fn write(address: u64, data: Vec<u8>) -> Self {
        let len = data.len() as u32;
        Self {
            address,
            command: TlmCommand::Write,
            data,
            byte_enables: None,
            streaming_width: len,
            response: TlmResponse::IncompleteResponse,
            delay_ns: 0.0,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.response == TlmResponse::Ok
    }
}
