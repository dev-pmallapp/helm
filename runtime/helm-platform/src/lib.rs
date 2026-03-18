//! Platform definitions — device topology and runtime attachment.
//!
//! A [`Platform`] describes a machine's address map and device wiring.
//! Platforms expose [`AttachableSlot`]s for runtime device attachment
//! (before `run()` is called — design rule 7: config frozen after build).

pub mod topology;
pub mod aarch64;

// ── Platform trait ──────────────────────────────────────────────────────────

/// A platform describes how devices are wired into a system.
///
/// Platforms are constructed by Python config code and passed to the engine.
/// The `build()` method is called once during `elaborate()` to populate the
/// system memory map with devices.
pub trait Platform: Send {
    /// Human-readable platform name (e.g. "arm-virt").
    fn name(&self) -> &str;

    /// Slots available for runtime device attachment.
    fn attachment_slots(&self) -> &[AttachableSlot];
}

// ── PlatformError ───────────────────────────────────────────────────────────

/// Errors from platform operations.
#[derive(Debug)]
pub enum PlatformError {
    /// A device could not be created.
    DeviceCreation(String),
    /// The requested slot is full.
    SlotFull {
        /// Name of the full slot.
        slot: String,
    },
    /// Configuration is frozen (run() already called).
    ConfigFrozen,
    /// Generic platform error.
    Other(String),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceCreation(msg) => write!(f, "device creation failed: {msg}"),
            Self::SlotFull { slot } => write!(f, "slot '{slot}' is full"),
            Self::ConfigFrozen => write!(f, "configuration is frozen after run()"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for PlatformError {}

// ── AttachableSlot ──────────────────────────────────────────────────────────

/// A slot where devices can be attached at runtime (before `run()`).
///
/// Platforms expose these to allow Python config scripts to add devices
/// dynamically, similar to QEMU's `-device` flag.
#[derive(Debug, Clone)]
pub struct AttachableSlot {
    /// Slot name (e.g. "mmio", "pci0", "virtio-mmio.0").
    pub name: &'static str,
    /// What kind of attachment this slot accepts.
    pub slot_type: SlotType,
    /// Maximum devices this slot can hold.
    pub max_devices: usize,
}

/// The type of bus/transport a slot provides.
#[derive(Debug, Clone)]
pub enum SlotType {
    /// Direct MMIO mapping at an address within the given range.
    Mmio {
        /// (start, end) of the address range available for device placement.
        base_range: (u64, u64),
    },
    /// PCI endpoint slot.
    Pci,
    /// VirtIO MMIO transport slot.
    VirtioMmio,
}
