"""AArch64 device re-exports — thin wrappers for discoverability."""

# GicV2 and Pl011 are implemented as #[pyclass] in Rust.
# Re-export them here so users can do `from helm.aarch64 import GicV2`.
from _helm_ng import GicV2, Pl011

__all__ = ["GicV2", "Pl011"]
