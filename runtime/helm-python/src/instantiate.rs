#![allow(missing_docs)]

use helm_devices::DeviceParams;
use helm_engine::{build_simulator, ExecMode, HelmSim, Isa, TimingChoice};
use helm_platform::aarch64::virt::ArmVirtPlatform;
use helm_platform::aarch64::virt::RAM_BASE as ARM_VIRT_RAM_BASE;
use helm_platform::{Platform, PlatformBuildPlan, RegionKind};
use pyo3::prelude::*;

use crate::cpu::Cpu;
use crate::devices::{GicV2, Pl011};
use crate::memory_space::MemorySpace;
use crate::ram::Ram;
use crate::simobject::{SimObject, SimObjectState};
use crate::system::{parse_mode, parse_timing, System};

const DEFAULT_MEM_SIZE: usize = 512 * 1024 * 1024;

pub(crate) struct FrozenSystemConfig {
    pub(crate) isa: Isa,
    pub(crate) mode: ExecMode,
    pub(crate) timing: TimingChoice,
    pub(crate) mem_base: u64,
    pub(crate) mem_size: usize,
    pub(crate) mappings: Vec<FrozenMapEntry>,
}

#[derive(Debug, Default)]
struct DiscoveredConfig {
    cpu_isa: Option<String>,
    direct_ram_size: Option<usize>,
    mapped_ram: Option<(u64, usize)>,
    mappings: Vec<FrozenMapEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrozenDeviceKind {
    Ram,
    GicV2 { num_irqs: u32 },
    Pl011,
    Unknown { python_type: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrozenMapEntry {
    pub(crate) base: u64,
    pub(crate) size: usize,
    pub(crate) bank: u32,
    pub(crate) kind: FrozenDeviceKind,
}

impl FrozenSystemConfig {
    pub(crate) fn from_explicit(
        isa: &str,
        mode: &str,
        timing: &str,
        mem_base: u64,
        mem_size: usize,
        ipc: f64,
    ) -> PyResult<Self> {
        Ok(Self {
            isa: parse_isa(isa)?,
            mode: parse_mode(mode)?,
            timing: parse_timing(timing, ipc)?,
            mem_base,
            mem_size,
            mappings: Vec::new(),
        })
    }

    pub(crate) fn build(self) -> HelmSim {
        build_simulator(
            self.isa,
            self.mode,
            self.timing,
            self.mem_base,
            self.mem_size,
        )
    }
}

pub(crate) fn instantiate_system(mut system: PyRefMut<'_, System>, py: Python<'_>) -> PyResult<()> {
    let config = {
        let base: &SimObject = system.as_ref();
        base.require_pending()?;

        if system.sim.is_some() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "system is already instantiated",
            ));
        }

        freeze_system_config(py, &system, base)?
    };

    system.sim = Some(config.build());

    let base: &mut SimObject = system.as_mut();
    base.state = SimObjectState::Instantiated;
    Ok(())
}

pub(crate) fn parse_isa(s: &str) -> PyResult<Isa> {
    match s {
        "aarch64" | "arm64" => Ok(Isa::AArch64),
        "riscv" | "riscv64" | "rv64" => Ok(Isa::RiscV),
        "aarch32" | "arm32" => Ok(Isa::AArch32),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unknown ISA '{other}'"
        ))),
    }
}

fn freeze_system_config(
    py: Python<'_>,
    system: &System,
    base: &SimObject,
) -> PyResult<FrozenSystemConfig> {
    let discovered = discover_children(py, base)?;
    build_from_discovered(system, discovered)
}

fn discover_children(py: Python<'_>, base: &SimObject) -> PyResult<DiscoveredConfig> {
    let mut discovered = DiscoveredConfig::default();

    for child in base.children.values() {
        let bound = child.bind(py);

        if let Ok(cpu) = bound.extract::<PyRef<'_, Cpu>>() {
            set_unique_string(&mut discovered.cpu_isa, &cpu.isa, "CPU ISA")?;
            continue;
        }

        if let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() {
            for entry in &mem.entries {
                let kind = classify_mapped_device(py, &entry.device)?;
                discovered.mappings.push(FrozenMapEntry {
                    base: entry.base,
                    size: entry.size as usize,
                    bank: entry.bank,
                    kind: kind.clone(),
                });

                if matches!(kind, FrozenDeviceKind::Ram) {
                    set_unique_ram_mapping(
                        &mut discovered.mapped_ram,
                        (entry.base, entry.size as usize),
                    )?;
                }
            }
            continue;
        }

        if let Ok(ram) = bound.extract::<PyRef<'_, Ram>>() {
            let size = parse_ram_size(&ram.size)?;
            set_unique_usize(&mut discovered.direct_ram_size, size, "RAM size")?;
        }
    }

    Ok(discovered)
}

fn build_from_discovered(
    system: &System,
    mut discovered: DiscoveredConfig,
) -> PyResult<FrozenSystemConfig> {
    validate_no_overlaps(&discovered.mappings)?;
    let mode = parse_mode(&system.mode)?;
    validate_platform_constraints(mode, &discovered.mappings)?;
    let timing = parse_timing(&system.timing, system.ipc)?;
    let isa = discovered
        .cpu_isa
        .as_deref()
        .map(parse_isa)
        .transpose()?
        .unwrap_or(Isa::AArch64);

    let (mem_base, mem_size) = discovered.mapped_ram.unwrap_or_else(|| {
        (
            default_mem_base(mode),
            discovered.direct_ram_size.unwrap_or(DEFAULT_MEM_SIZE),
        )
    });

    Ok(FrozenSystemConfig {
        isa,
        mode,
        timing,
        mem_base,
        mem_size,
        mappings: std::mem::take(&mut discovered.mappings),
    })
}

fn default_mem_base(mode: ExecMode) -> u64 {
    if mode == ExecMode::System {
        ARM_VIRT_RAM_BASE
    } else {
        0
    }
}

fn parse_ram_size(size: &str) -> PyResult<usize> {
    let bytes = DeviceParams::parse_memory_size(size).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("invalid RAM size '{size}': {e}"))
    })?;
    usize::try_from(bytes).map_err(|_| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "RAM size '{size}' exceeds host usize capacity"
        ))
    })
}

fn set_unique_string(slot: &mut Option<String>, value: &str, label: &str) -> PyResult<()> {
    match slot {
        Some(existing) if existing != value => Err(pyo3::exceptions::PyValueError::new_err(
            format!("conflicting {label}: '{existing}' vs '{value}'"),
        )),
        Some(_) => Ok(()),
        None => {
            *slot = Some(value.to_string());
            Ok(())
        }
    }
}

fn set_unique_usize(slot: &mut Option<usize>, value: usize, label: &str) -> PyResult<()> {
    match slot {
        Some(existing) if *existing != value => Err(pyo3::exceptions::PyValueError::new_err(
            format!("conflicting {label}: {existing} vs {value}"),
        )),
        Some(_) => Ok(()),
        None => {
            *slot = Some(value);
            Ok(())
        }
    }
}

fn set_unique_ram_mapping(slot: &mut Option<(u64, usize)>, value: (u64, usize)) -> PyResult<()> {
    match slot {
        Some(existing) if *existing != value => {
            Err(pyo3::exceptions::PyValueError::new_err(format!(
                "multiple RAM mappings are not yet supported: ({:#x}, {}) vs ({:#x}, {})",
                existing.0, existing.1, value.0, value.1
            )))
        }
        Some(_) => Ok(()),
        None => {
            *slot = Some(value);
            Ok(())
        }
    }
}

fn classify_mapped_device(py: Python<'_>, device: &PyObject) -> PyResult<FrozenDeviceKind> {
    let bound = device.bind(py);
    if bound.extract::<PyRef<'_, Ram>>().is_ok() {
        return Ok(FrozenDeviceKind::Ram);
    }
    if let Ok(gic) = bound.extract::<PyRef<'_, GicV2>>() {
        return Ok(FrozenDeviceKind::GicV2 {
            num_irqs: gic.num_irqs,
        });
    }
    if bound.extract::<PyRef<'_, Pl011>>().is_ok() {
        return Ok(FrozenDeviceKind::Pl011);
    }

    let ty_name = bound
        .get_type()
        .name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(FrozenDeviceKind::Unknown {
        python_type: ty_name,
    })
}

fn validate_no_overlaps(mappings: &[FrozenMapEntry]) -> PyResult<()> {
    let mut sorted: Vec<&FrozenMapEntry> = mappings.iter().collect();
    sorted.sort_by_key(|entry| entry.base);

    for pair in sorted.windows(2) {
        let left = pair[0];
        let right = pair[1];
        let left_end = u128::from(left.base) + u128::from(left.size as u64);
        if left_end > u128::from(right.base) {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
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

fn validate_platform_constraints(mode: ExecMode, mappings: &[FrozenMapEntry]) -> PyResult<()> {
    if mode != ExecMode::System || mappings.is_empty() {
        return Ok(());
    }

    let plan = ArmVirtPlatform.build_plan();
    for mapping in mappings {
        validate_system_mapping(&plan, mapping)?;
    }
    Ok(())
}

fn validate_system_mapping(plan: &PlatformBuildPlan, mapping: &FrozenMapEntry) -> PyResult<()> {
    match &mapping.kind {
        FrozenDeviceKind::Ram => {
            let ram = plan
                .region_named("ram")
                .expect("arm-virt RAM region missing");
            if mapping.base != ram.base {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "system-mode RAM must start at {:#x}, got {:#x}",
                    ram.base, mapping.base
                )));
            }
        }
        FrozenDeviceKind::GicV2 { .. } => {
            let region_name = match mapping.bank {
                0 => "gic-dist",
                1 => "gic-cpu",
                other => {
                    return Err(pyo3::exceptions::PyValueError::new_err(format!(
                        "system-mode GicV2 bank must be 0 or 1, got {other}"
                    )))
                }
            };
            validate_exact_region(plan, region_name, mapping)?;
        }
        FrozenDeviceKind::Pl011 => {
            if mapping.bank != 0 {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "system-mode Pl011 bank must be 0, got {}",
                    mapping.bank
                )));
            }
            validate_exact_region(plan, "uart0", mapping)?;
        }
        FrozenDeviceKind::Unknown { python_type } => {
            if plan
                .attachment_window_for(mapping.base, mapping.size as u64)
                .is_none()
            {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "system-mode mapping for unknown device type '{python_type}' must fit an attachment window"
                )));
            }
        }
    }

    Ok(())
}

fn validate_exact_region(
    plan: &PlatformBuildPlan,
    region_name: &str,
    mapping: &FrozenMapEntry,
) -> PyResult<()> {
    let region = plan.region_named(region_name).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(format!("missing platform region '{region_name}'"))
    })?;
    debug_assert_eq!(region.kind, RegionKind::Mmio);

    if mapping.base != region.base || mapping.size as u64 != region.size {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "system-mode mapping for '{region_name}' must be [{:#x}, {:#x}), got [{:#x}, {:#x})",
            region.base,
            region.base.saturating_add(region.size),
            mapping.base,
            mapping.base.saturating_add(mapping.size as u64),
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(mode: &str) -> System {
        System {
            timing: "virtual".into(),
            mode: mode.into(),
            ipc: 4.0,
            sim: None,
            exited: false,
            exit_code_val: 0,
            plugins: Vec::new(),
        }
    }

    #[test]
    fn parse_isa_aliases() {
        assert!(matches!(parse_isa("aarch64").unwrap(), Isa::AArch64));
        assert!(matches!(parse_isa("rv64").unwrap(), Isa::RiscV));
        assert!(matches!(parse_isa("arm32").unwrap(), Isa::AArch32));
    }

    #[test]
    fn parse_ram_size_strings() {
        assert_eq!(parse_ram_size("512MiB").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_ram_size("1GiB").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn discovered_defaults_are_frozen() {
        let cfg = build_from_discovered(&system("se"), DiscoveredConfig::default()).unwrap();
        assert!(matches!(cfg.isa, Isa::AArch64));
        assert_eq!(cfg.mem_base, 0);
        assert_eq!(cfg.mem_size, DEFAULT_MEM_SIZE);
        assert!(matches!(cfg.timing, TimingChoice::Virtual { .. }));
    }

    #[test]
    fn system_mode_uses_platform_ram_base_by_default() {
        let cfg = build_from_discovered(&system("fs"), DiscoveredConfig::default()).unwrap();
        assert_eq!(cfg.mem_base, ARM_VIRT_RAM_BASE);
    }

    #[test]
    fn memory_space_mapping_wins_over_direct_ram_size() {
        let discovered = DiscoveredConfig {
            cpu_isa: Some("riscv64".into()),
            direct_ram_size: Some(64 * 1024 * 1024),
            mapped_ram: Some((0x4000_0000, 128 * 1024 * 1024)),
            mappings: vec![FrozenMapEntry {
                base: 0x4000_0000,
                size: 128 * 1024 * 1024,
                bank: 0,
                kind: FrozenDeviceKind::Ram,
            }],
        };
        let cfg = build_from_discovered(&system("se"), discovered).unwrap();
        assert!(matches!(cfg.isa, Isa::RiscV));
        assert_eq!(cfg.mem_base, 0x4000_0000);
        assert_eq!(cfg.mem_size, 128 * 1024 * 1024);
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn overlapping_mappings_are_rejected() {
        pyo3::prepare_freethreaded_python();
        let discovered = DiscoveredConfig {
            cpu_isa: None,
            direct_ram_size: None,
            mapped_ram: Some((0x1000, 0x1000)),
            mappings: vec![
                FrozenMapEntry {
                    base: 0x1000,
                    size: 0x1000,
                    bank: 0,
                    kind: FrozenDeviceKind::Ram,
                },
                FrozenMapEntry {
                    base: 0x1800,
                    size: 0x1000,
                    bank: 0,
                    kind: FrozenDeviceKind::Pl011,
                },
            ],
        };
        let err = build_from_discovered(&system("se"), discovered)
            .err()
            .expect("overlap should fail");
        assert!(err.to_string().contains("overlapping memory mappings"));
    }

    #[test]
    fn system_mode_gic_mapping_must_match_platform_layout() {
        pyo3::prepare_freethreaded_python();
        let discovered = DiscoveredConfig {
            cpu_isa: Some("aarch64".into()),
            direct_ram_size: None,
            mapped_ram: Some((ARM_VIRT_RAM_BASE, 128 * 1024 * 1024)),
            mappings: vec![
                FrozenMapEntry {
                    base: ARM_VIRT_RAM_BASE,
                    size: 128 * 1024 * 1024,
                    bank: 0,
                    kind: FrozenDeviceKind::Ram,
                },
                FrozenMapEntry {
                    base: 0x0900_0000,
                    size: 0x1000,
                    bank: 0,
                    kind: FrozenDeviceKind::GicV2 { num_irqs: 96 },
                },
            ],
        };
        let err = build_from_discovered(&system("fs"), discovered)
            .err()
            .expect("mismatched gic mapping should fail");
        assert!(err
            .to_string()
            .contains("system-mode mapping for 'gic-dist'"));
    }

    #[test]
    fn system_mode_unknown_device_must_fit_attachment_window() {
        pyo3::prepare_freethreaded_python();
        let discovered = DiscoveredConfig {
            cpu_isa: Some("aarch64".into()),
            direct_ram_size: None,
            mapped_ram: Some((ARM_VIRT_RAM_BASE, 128 * 1024 * 1024)),
            mappings: vec![
                FrozenMapEntry {
                    base: ARM_VIRT_RAM_BASE,
                    size: 128 * 1024 * 1024,
                    bank: 0,
                    kind: FrozenDeviceKind::Ram,
                },
                FrozenMapEntry {
                    base: 0x0900_1000,
                    size: 0x1000,
                    bank: 0,
                    kind: FrozenDeviceKind::Unknown {
                        python_type: "CustomDevice".into(),
                    },
                },
            ],
        };
        let err = build_from_discovered(&system("fs"), discovered)
            .err()
            .expect("unknown device outside attachment window should fail");
        assert!(err.to_string().contains("attachment window"));
    }
}
