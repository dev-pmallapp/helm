//! KVM-specific error types.

use thiserror::Error;

/// Errors returned by the KVM backend.
#[derive(Error, Debug)]
pub enum KvmError {
    /// The host kernel does not expose `/dev/kvm` or lacks a required
    /// capability.
    #[error("KVM not available: {0}")]
    Unavailable(String),

    /// A KVM ioctl returned an OS error.
    #[error("KVM ioctl {name} failed: {source}")]
    Ioctl {
        name: &'static str,
        source: std::io::Error,
    },

    /// The requested capability is not supported by this KVM instance.
    #[error("KVM capability not supported: {0}")]
    CapabilityMissing(String),

    /// An invalid parameter was passed to a KVM operation.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// Memory mapping failed.
    #[error("mmap failed: {0}")]
    Mmap(std::io::Error),

    /// The guest vCPU encountered an unrecoverable error.
    #[error("vCPU internal error: suberror={suberror}")]
    InternalError { suberror: u32 },

    /// KVM is not supported on this platform (non-Linux or ISA mismatch).
    #[error("KVM not supported on this platform")]
    Unsupported,
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, KvmError>;

impl KvmError {
    /// Build an [`Ioctl`](KvmError::Ioctl) from the last OS error.
    pub fn last_os_ioctl(name: &'static str) -> Self {
        Self::Ioctl {
            name,
            source: std::io::Error::last_os_error(),
        }
    }
}

impl From<KvmError> for helm_core::HelmError {
    fn from(e: KvmError) -> Self {
        helm_core::HelmError::Config(format!("KVM: {e}"))
    }
}
