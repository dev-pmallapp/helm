//! Built-in platform selection helpers.
//!
//! This module provides typed selection and mapping artifacts for built-in
//! platforms without forcing callers to name crate-local platform structs
//! directly.

use crate::aarch64::virt::ArmVirtPlatform;
use crate::{Platform, PlatformError};

/// One discovered or requested mapped device in a built-in platform config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltInMappedDevice {
    /// Guest-physical base address.
    pub base: u64,
    /// Mapping size in bytes.
    pub size: usize,
    /// Bank selector used by higher-level config.
    pub bank: u32,
    /// Semantic mapped-device kind for validation.
    pub kind: BuiltInMappedDeviceKind,
}

/// Shared discovery record produced by higher-level config walks before a
/// simulator build request is frozen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuiltInDiscoveredConfig {
    /// Discovered CPU ISA name, if one was explicitly configured.
    pub cpu_isa: Option<String>,
    /// Direct RAM size configured outside an explicit memory-space mapping.
    pub direct_ram_size: Option<usize>,
    /// Discovered RAM mapping `(base, size)` when one exists.
    pub mapped_ram: Option<(u64, usize)>,
    /// All discovered built-in mapped devices.
    pub mappings: Vec<BuiltInMappedDevice>,
}

/// Shared defaulting result used while freezing a simulator build request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltInFreezeDefaults {
    /// Selected built-in platform, if system-mode execution requires one.
    pub platform: Option<BuiltInPlatform>,
    /// Final guest-physical memory base after platform/default resolution.
    pub mem_base: u64,
    /// Final guest-visible RAM size after discovery/default resolution.
    pub mem_size: usize,
}

/// Classification for one discovered mapped device in built-in platform config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltInMappedDeviceKind {
    /// Guest RAM mapping.
    Ram,
    /// GICv2 fixed MMIO mapping discovered from config.
    GicV2 {
        /// Interrupt capacity declared by the configured GIC instance.
        num_irqs: u32,
    },
    /// PL011 UART fixed MMIO mapping.
    Pl011,
    /// Any other mapped device type discovered from higher-level config.
    Unknown {
        /// Python-side type name captured during config discovery.
        python_type: String,
    },
}

/// Classify one built-in mapped device from a higher-level config type name.
///
/// The caller provides any extra metadata that only the higher-level config
/// layer can inspect directly. Unknown types are preserved as `Unknown`
/// instead of being rejected so attachment-window validation can decide
/// whether they are allowed in system mode.
pub fn classify_builtin_mapped_device(
    python_type: &str,
    gic_num_irqs: Option<u32>,
) -> Result<BuiltInMappedDeviceKind, PlatformError> {
    match python_type {
        "Ram" => Ok(BuiltInMappedDeviceKind::Ram),
        "GicV2" => Ok(BuiltInMappedDeviceKind::GicV2 {
            num_irqs: gic_num_irqs
                .ok_or_else(|| PlatformError::other("GicV2 discovery requires num_irqs metadata"))?,
        }),
        "Pl011" => Ok(BuiltInMappedDeviceKind::Pl011),
        other => Ok(BuiltInMappedDeviceKind::Unknown {
            python_type: other.to_string(),
        }),
    }
}

/// Validate that discovered built-in mappings do not overlap in address space.
pub fn validate_non_overlapping_mappings(
    mappings: &[BuiltInMappedDevice],
) -> Result<(), PlatformError> {
    let mut sorted: Vec<&BuiltInMappedDevice> = mappings.iter().collect();
    sorted.sort_by_key(|entry| entry.base);

    for pair in sorted.windows(2) {
        let left = pair[0];
        let right = pair[1];
        let left_end = u128::from(left.base) + u128::from(left.size as u64);
        if left_end > u128::from(right.base) {
            return Err(PlatformError::other(format!(
                "overlapping memory mappings: [{:#x}, {:#x}) overlaps [{:#x}, {:#x})",
                left.base,
                left.base.saturating_add(left.size as u64),
                right.base,
                right.base.saturating_add(right.size as u64),
            )));
        }
    }

    Ok(())
}

/// One built-in platform selectable by configuration code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInPlatform {
    /// QEMU-compatible ARM virt machine.
    ArmVirt,
}

impl BuiltInPlatform {
    /// Stable CLI / config name for the platform.
    pub fn name(self) -> &'static str {
        match self {
            Self::ArmVirt => "arm-virt",
        }
    }

    /// Frozen build plan for the selected platform.
    pub fn build_plan(self) -> crate::PlatformBuildPlan {
        match self {
            Self::ArmVirt => ArmVirtPlatform.build_plan(),
        }
    }

    /// Default RAM base for the selected platform.
    pub fn default_ram_base(self) -> u64 {
        match self {
            Self::ArmVirt => ArmVirtPlatform.default_ram_base(),
        }
    }

    /// Validate system-mode mappings for the selected platform.
    pub fn validate_system_mappings(
        self,
        mappings: &[BuiltInMappedDevice],
    ) -> Result<(), PlatformError> {
        match self {
            Self::ArmVirt => ArmVirtPlatform.validate_system_mappings(mappings),
        }
    }
}

/// Return the default built-in system platform for one ISA name.
pub fn default_system_platform_for_isa(isa: &str) -> Option<BuiltInPlatform> {
    match isa {
        "aarch64" => Some(BuiltInPlatform::ArmVirt),
        _ => None,
    }
}

/// Derive built-in platform selection and memory defaults from one discovered
/// config record.
pub fn derive_built_in_freeze_defaults(
    is_system_mode: bool,
    isa_name: &str,
    discovered: &BuiltInDiscoveredConfig,
    default_mem_size: usize,
) -> Result<BuiltInFreezeDefaults, PlatformError> {
    let platform =
        if is_system_mode {
            Some(default_system_platform_for_isa(isa_name).ok_or_else(|| {
                PlatformError::other(format!(
                    "no default system platform is defined for ISA '{isa_name}'"
                ))
            })?)
        } else {
            None
        };

    let (mem_base, mem_size) = discovered.mapped_ram.unwrap_or_else(|| {
        (
            platform.map_or(0, BuiltInPlatform::default_ram_base),
            discovered.direct_ram_size.unwrap_or(default_mem_size),
        )
    });

    Ok(BuiltInFreezeDefaults {
        platform,
        mem_base,
        mem_size,
    })
}

/// Validate one discovered built-in config record and derive the shared
/// defaulting result used for freezing.
pub fn freeze_built_in_discovered_config(
    is_system_mode: bool,
    isa_name: &str,
    discovered: &BuiltInDiscoveredConfig,
    default_mem_size: usize,
) -> Result<BuiltInFreezeDefaults, PlatformError> {
    validate_non_overlapping_mappings(&discovered.mappings)?;
    let defaults =
        derive_built_in_freeze_defaults(is_system_mode, isa_name, discovered, default_mem_size)?;
    if let Some(platform) = defaults.platform {
        platform.validate_system_mappings(&discovered.mappings)?;
    }
    Ok(defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_system_platform_is_arm_virt_for_aarch64() {
        assert_eq!(
            default_system_platform_for_isa("aarch64"),
            Some(BuiltInPlatform::ArmVirt)
        );
    }

    #[test]
    fn default_system_platform_is_absent_for_unsupported_isa() {
        assert_eq!(default_system_platform_for_isa("riscv64"), None);
    }

    #[test]
    fn classify_builtin_mapped_device_handles_known_types() {
        assert_eq!(
            classify_builtin_mapped_device("Ram", None).unwrap(),
            BuiltInMappedDeviceKind::Ram
        );
        assert_eq!(
            classify_builtin_mapped_device("Pl011", None).unwrap(),
            BuiltInMappedDeviceKind::Pl011
        );
        assert_eq!(
            classify_builtin_mapped_device("GicV2", Some(128)).unwrap(),
            BuiltInMappedDeviceKind::GicV2 { num_irqs: 128 }
        );
    }

    #[test]
    fn classify_builtin_mapped_device_preserves_unknown_types() {
        assert_eq!(
            classify_builtin_mapped_device("CustomDevice", None).unwrap(),
            BuiltInMappedDeviceKind::Unknown {
                python_type: "CustomDevice".to_string()
            }
        );
    }

    #[test]
    fn classify_builtin_mapped_device_requires_gic_metadata() {
        assert!(classify_builtin_mapped_device("GicV2", None).is_err());
    }

    #[test]
    fn validate_non_overlapping_mappings_accepts_disjoint_ranges() {
        let mappings = vec![
            BuiltInMappedDevice {
                base: 0x1000,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Ram,
            },
            BuiltInMappedDevice {
                base: 0x3000,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Pl011,
            },
        ];
        assert!(validate_non_overlapping_mappings(&mappings).is_ok());
    }

    #[test]
    fn validate_non_overlapping_mappings_rejects_overlap() {
        let mappings = vec![
            BuiltInMappedDevice {
                base: 0x1000,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Ram,
            },
            BuiltInMappedDevice {
                base: 0x1800,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Pl011,
            },
        ];
        let err = validate_non_overlapping_mappings(&mappings).expect_err("overlap should fail");
        assert!(err.to_string().contains("overlapping memory mappings"));
    }

    #[test]
    fn derive_freeze_defaults_uses_system_platform_defaults() {
        let discovered = BuiltInDiscoveredConfig::default();
        let defaults =
            derive_built_in_freeze_defaults(true, "aarch64", &discovered, 512 * 1024 * 1024)
                .expect("defaults should resolve");

        assert_eq!(defaults.platform, Some(BuiltInPlatform::ArmVirt));
        assert_eq!(
            defaults.mem_base,
            BuiltInPlatform::ArmVirt.default_ram_base()
        );
        assert_eq!(defaults.mem_size, 512 * 1024 * 1024);
    }

    #[test]
    fn derive_freeze_defaults_prefers_discovered_ram_mapping() {
        let discovered = BuiltInDiscoveredConfig {
            mapped_ram: Some((0x4000_0000, 128 * 1024 * 1024)),
            direct_ram_size: Some(64 * 1024 * 1024),
            ..Default::default()
        };
        let defaults =
            derive_built_in_freeze_defaults(false, "riscv64", &discovered, 512 * 1024 * 1024)
                .expect("defaults should resolve");

        assert_eq!(defaults.platform, None);
        assert_eq!(defaults.mem_base, 0x4000_0000);
        assert_eq!(defaults.mem_size, 128 * 1024 * 1024);
    }

    #[test]
    fn derive_freeze_defaults_rejects_missing_system_platform() {
        let err = derive_built_in_freeze_defaults(
            true,
            "riscv64",
            &BuiltInDiscoveredConfig::default(),
            1,
        )
        .expect_err("system platform should be required");
        assert!(err.to_string().contains("no default system platform"));
    }

    #[test]
    fn freeze_built_in_discovered_config_rejects_overlap_before_defaults() {
        let discovered = BuiltInDiscoveredConfig {
            mappings: vec![
                BuiltInMappedDevice {
                    base: 0x1000,
                    size: 0x1000,
                    bank: 0,
                    kind: BuiltInMappedDeviceKind::Ram,
                },
                BuiltInMappedDevice {
                    base: 0x1800,
                    size: 0x1000,
                    bank: 0,
                    kind: BuiltInMappedDeviceKind::Pl011,
                },
            ],
            ..Default::default()
        };
        let err = freeze_built_in_discovered_config(false, "aarch64", &discovered, 1)
            .expect_err("overlap should fail");
        assert!(err.to_string().contains("overlapping memory mappings"));
    }

    #[test]
    fn freeze_built_in_discovered_config_validates_system_layout() {
        let discovered = BuiltInDiscoveredConfig {
            mappings: vec![BuiltInMappedDevice {
                base: 0x0900_1000,
                size: 0x1000,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Unknown {
                    python_type: "CustomDevice".to_string(),
                },
            }],
            ..Default::default()
        };
        let err = freeze_built_in_discovered_config(true, "aarch64", &discovered, 1)
            .expect_err("attachment-window validation should fail");
        assert!(err.to_string().contains("attachment window"));
    }
}
