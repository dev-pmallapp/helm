//! Platform definitions — device topology and frozen build metadata.
//!
//! A [`Platform`] describes a machine's address map and device wiring.
//! Platforms expose [`AttachableSlot`]s for runtime device attachment
//! (before `run()` is called — design rule 7: config frozen after build).
//!
//! On `main`, `runtime/helm-platform` is the descriptor/metadata layer:
//! it owns frozen platform layout, quirk metadata, and selection/defaulting
//! helpers. Final executable built-system integration remains in the runtime
//! integration layer (`runtime/helm-engine/src/platform/*`) rather than in the
//! descriptor crate itself.

pub mod aarch64;
pub mod affinity;
pub mod build;
pub mod quirks;
pub mod selection;
pub mod topology;

pub use affinity::AffinityMap;
pub use build::{AddressRegionSpec, InterruptRouteSpec, PlatformBuildPlan, RegionKind};
pub use quirks::{BoardQuirk, PlatformQuirk, QuirkKey, QuirkSet, QuirkSpec};
pub use selection::{
    classify_builtin_mapped_device, default_system_platform_for_isa,
    derive_built_in_freeze_defaults, freeze_built_in_discovered_config,
    validate_non_overlapping_mappings, BuiltInDiscoveredConfig, BuiltInFreezeDefaults,
    BuiltInMappedDevice, BuiltInMappedDeviceKind, BuiltInPlatform,
};

// ── Platform trait ──────────────────────────────────────────────────────────

/// A platform describes how devices are wired into a system.
///
/// Platforms are constructed by Python config code and passed to the runtime
/// integration layer.
///
/// The trait exposes frozen topology/attachment metadata only; it does not
/// itself imply ownership of final executable system realization.
pub trait Platform: Send {
    /// Human-readable platform name (e.g. "arm-virt").
    fn name(&self) -> &str;

    /// Slots available for runtime device attachment.
    fn attachment_slots(&self) -> &[AttachableSlot];

    /// Frozen metadata for the platform's address layout and fixed wiring.
    fn build_plan(&self) -> PlatformBuildPlan;
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
    /// Configuration is frozen (`run()` already called).
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
    /// `VirtIO` MMIO transport slot.
    VirtioMmio,
}

// ── Platform registry ───────────────────────────────────────────────────────

/// Descriptor returned by [`list_platforms`].
#[derive(Debug, Clone)]
pub struct PlatformInfo {
    /// CLI name (e.g. `"arm-virt"`).
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Supported ISA.
    pub isa: &'static str,
}

/// Return the set of built-in platforms.
pub fn list_platforms() -> Vec<PlatformInfo> {
    vec![PlatformInfo {
        name: "arm-virt",
        description: "QEMU-compatible ARM virt machine (GICv2/v3, PL011 UART)",
        isa: "aarch64",
    }]
}
