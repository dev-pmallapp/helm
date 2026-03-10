//! Error types for LLVM IR frontend

use thiserror::Error;

/// Result type for LLVM operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in the LLVM IR frontend
#[derive(Error, Debug)]
pub enum Error {
    #[error("Failed to parse LLVM IR: {0}")]
    ParseError(String),

    #[error("Invalid LLVM instruction: {0}")]
    InvalidInstruction(String),

    #[error("Unsupported LLVM operation: {0}")]
    UnsupportedOperation(String),

    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),

    #[error("Scheduling error: {0}")]
    SchedulingError(String),

    #[error("Type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("LLVM error: {0}")]
    LLVMError(String),

    #[error("{0}")]
    Other(String),
}
