//! Unified error types for the HELM simulator.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum HelmError {
    #[error("ISA error: {0}")]
    Isa(String),

    #[error("Decode error at address {addr:#x}: {reason}")]
    Decode { addr: u64, reason: String },

    #[error("Translation error: {0}")]
    Translation(String),

    #[error("Syscall error: syscall {number} — {reason}")]
    Syscall { number: u64, reason: String },

    #[error("Memory error at address {addr:#x}: {reason}")]
    Memory { addr: u64, reason: String },

    #[error("Pipeline error: {0}")]
    Pipeline(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type HelmResult<T> = Result<T, HelmError>;
