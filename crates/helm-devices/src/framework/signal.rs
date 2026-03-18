//! Named signal constants for device control.
//!
//! Signals are string-identified control inputs that the platform can assert
//! on a device without a full MMIO write. Devices receive signals via
//! [`Device::signal()`](super::device::Device::signal).
//!
//! Unknown signals should be silently ignored by devices. These constants
//! define the well-known signal names used by the platform and built-in
//! devices.

/// Hardware reset -- device returns to power-on state.
pub const SIGNAL_RESET: &str = "reset";

/// Functional clock gating. `val=1`: running, `val=0`: halted.
pub const SIGNAL_CLOCK_ENABLE: &str = "clock_enable";

/// DMA controller acknowledges a transfer completion.
pub const SIGNAL_DMA_ACK: &str = "dma_ack";

/// Non-maskable interrupt input (for interrupt controllers).
pub const SIGNAL_NMI: &str = "nmi";
