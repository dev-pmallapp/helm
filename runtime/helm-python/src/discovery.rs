#![allow(missing_docs)]

use helm_devices::DeviceParams;
use helm_platform::{
    classify_builtin_mapped_device, BuiltInDiscoveredConfig, BuiltInMappedDevice,
    BuiltInMappedDeviceKind,
};
use pyo3::prelude::*;

use crate::cpu::Cpu;
use crate::devices::{
    GicV2, PciRamBar, PciVirtioBlk, PciVirtioConsole, PciVirtioNet, PciVirtioRng, PciVirtioRngMmio,
    Pl011,
};
use crate::errors::platform_error;
use crate::memory_space::{MapEntry, MemorySpace};
use crate::port::PortRef;
use crate::ram::Ram;
use crate::simobject::SimObject;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiscoveredPciRamBar {
    pub base: u64,
    pub size: usize,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiscoveredPciVirtioRngMmio {
    pub base: u64,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u32,
    pub seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiscoveredPciVirtioRng {
    pub base: u64,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiscoveredPciVirtioBlk {
    pub base: u64,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub capacity_bytes: usize,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredPciVirtioNet {
    pub base: u64,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub mac: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredPciVirtioConsole {
    pub base: u64,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub serial: String,
    pub cols: u16,
    pub rows: u16,
}

pub(crate) fn discover_children(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<BuiltInDiscoveredConfig> {
    let mut discovered = BuiltInDiscoveredConfig::default();

    for child in base.children.values() {
        let bound = child.bind(py);

        if let Ok(cpu) = bound.extract::<PyRef<'_, Cpu>>() {
            set_unique_string(&mut discovered.cpu_isa, &cpu.isa, "CPU ISA")?;
            continue;
        }

        if let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() {
            for entry in &mem.entries {
                let kind = classify_mapped_device(py, &entry.device)?;
                discovered.mappings.push(BuiltInMappedDevice {
                    base: entry.base,
                    size: entry.size as usize,
                    bank: entry.bank,
                    kind: kind.clone(),
                });

                if matches!(kind, BuiltInMappedDeviceKind::Ram) {
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

pub(crate) fn collect_port_refs(
    py: Python<'_>,
    base: &SimObject,
) -> Vec<(String, String, PortRef)> {
    let mut result = Vec::new();

    for (child_name, child_obj) in &base.children {
        let bound = child_obj.bind(py);

        if let Ok(pl011) = bound.extract::<PyRef<'_, Pl011>>() {
            if let Some(ref pref) = pl011.irq {
                result.push((child_name.clone(), "irq".to_string(), pref.clone()));
            }
        }

        if let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() {
            for entry in &mem.entries {
                let dev = entry.device.bind(py);
                if let Ok(pl011) = dev.extract::<PyRef<'_, Pl011>>() {
                    if let Some(ref pref) = pl011.irq {
                        let dev_name = format!("{child_name}.<mapped-pl011>");
                        result.push((dev_name, "irq".to_string(), pref.clone()));
                    }
                }
            }
        }
    }

    result
}

pub(crate) fn discover_pci_ram_bars(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<Vec<DiscoveredPciRamBar>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for child in base.children.values() {
        let bound = child.bind(py);
        let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() else {
            continue;
        };

        for entry in &mem.entries {
            let Some(bar) = extract_pci_ram_bar(py, entry) else {
                continue;
            };
            validate_pci_ram_bar_entry(entry, &bar)?;
            let key = (bar.bus, bar.slot, bar.function);
            if !seen.insert(key) {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "duplicate PCI function mapping for {}:{}:{}",
                    bar.bus, bar.slot, bar.function
                )));
            }
            result.push(DiscoveredPciRamBar {
                base: entry.base,
                size: entry.size as usize,
                bus: bar.bus,
                slot: bar.slot,
                function: bar.function,
                vendor_id: bar.vendor_id,
                device_id: bar.device_id,
                class_code: bar.class_code,
            });
        }
    }

    Ok(result)
}

pub(crate) fn discover_pci_virtio_rng_mmio(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<Vec<DiscoveredPciVirtioRngMmio>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for child in base.children.values() {
        let bound = child.bind(py);
        let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() else {
            continue;
        };

        for entry in &mem.entries {
            let Some(dev) = extract_pci_virtio_rng_mmio(py, entry) else {
                continue;
            };
            validate_pci_virtio_rng_mmio_entry(entry, &dev)?;
            let key = (dev.bus, dev.slot, dev.function);
            if !seen.insert(key) {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "duplicate PCI function mapping for {}:{}:{}",
                    dev.bus, dev.slot, dev.function
                )));
            }
            result.push(DiscoveredPciVirtioRngMmio {
                base: entry.base,
                bus: dev.bus,
                slot: dev.slot,
                function: dev.function,
                vendor_id: dev.vendor_id,
                device_id: dev.device_id,
                class_code: dev.class_code,
                seed: dev.seed,
            });
        }
    }

    Ok(result)
}

pub(crate) fn discover_pci_virtio_rng(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<Vec<DiscoveredPciVirtioRng>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for child in base.children.values() {
        let bound = child.bind(py);
        let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() else {
            continue;
        };

        for entry in &mem.entries {
            let Some(dev) = extract_pci_virtio_rng(py, entry) else {
                continue;
            };
            validate_pci_virtio_rng_entry(entry, &dev)?;
            let key = (dev.bus, dev.slot, dev.function);
            if !seen.insert(key) {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "duplicate PCI function mapping for {}:{}:{}",
                    dev.bus, dev.slot, dev.function
                )));
            }
            result.push(DiscoveredPciVirtioRng {
                base: entry.base,
                bus: dev.bus,
                slot: dev.slot,
                function: dev.function,
                seed: dev.seed,
            });
        }
    }

    Ok(result)
}

pub(crate) fn discover_pci_virtio_blk(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<Vec<DiscoveredPciVirtioBlk>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for child in base.children.values() {
        let bound = child.bind(py);
        let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() else {
            continue;
        };

        for entry in &mem.entries {
            let Some(dev) = extract_pci_virtio_blk(py, entry) else {
                continue;
            };
            validate_standard_pci_virtio_entry(
                entry,
                dev.bus,
                dev.slot,
                dev.function,
                "PciVirtioBlk",
            )?;
            let key = (dev.bus, dev.slot, dev.function);
            if !seen.insert(key) {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "duplicate PCI function mapping for {}:{}:{}",
                    dev.bus, dev.slot, dev.function
                )));
            }
            result.push(DiscoveredPciVirtioBlk {
                base: entry.base,
                bus: dev.bus,
                slot: dev.slot,
                function: dev.function,
                capacity_bytes: dev.capacity_bytes,
                read_only: dev.read_only,
            });
        }
    }

    Ok(result)
}

pub(crate) fn discover_pci_virtio_net(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<Vec<DiscoveredPciVirtioNet>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for child in base.children.values() {
        let bound = child.bind(py);
        let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() else {
            continue;
        };

        for entry in &mem.entries {
            let Some(dev) = extract_pci_virtio_net(py, entry) else {
                continue;
            };
            validate_standard_pci_virtio_entry(
                entry,
                dev.bus,
                dev.slot,
                dev.function,
                "PciVirtioNet",
            )?;
            let key = (dev.bus, dev.slot, dev.function);
            if !seen.insert(key) {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "duplicate PCI function mapping for {}:{}:{}",
                    dev.bus, dev.slot, dev.function
                )));
            }
            result.push(DiscoveredPciVirtioNet {
                base: entry.base,
                bus: dev.bus,
                slot: dev.slot,
                function: dev.function,
                mac: dev.mac.clone(),
            });
        }
    }

    Ok(result)
}

pub(crate) fn discover_pci_virtio_console(
    py: Python<'_>,
    base: &SimObject,
) -> PyResult<Vec<DiscoveredPciVirtioConsole>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for child in base.children.values() {
        let bound = child.bind(py);
        let Ok(mem) = bound.extract::<PyRef<'_, MemorySpace>>() else {
            continue;
        };

        for entry in &mem.entries {
            let Some(dev) = extract_pci_virtio_console(py, entry) else {
                continue;
            };
            validate_standard_pci_virtio_entry(
                entry,
                dev.bus,
                dev.slot,
                dev.function,
                "PciVirtioConsole",
            )?;
            let key = (dev.bus, dev.slot, dev.function);
            if !seen.insert(key) {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "duplicate PCI function mapping for {}:{}:{}",
                    dev.bus, dev.slot, dev.function
                )));
            }
            result.push(DiscoveredPciVirtioConsole {
                base: entry.base,
                bus: dev.bus,
                slot: dev.slot,
                function: dev.function,
                serial: dev.serial.clone(),
                cols: dev.cols,
                rows: dev.rows,
            });
        }
    }

    Ok(result)
}

pub(crate) fn parse_ram_size(size: &str) -> PyResult<usize> {
    let bytes = DeviceParams::parse_memory_size(size).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("invalid RAM size '{size}': {e}"))
    })?;
    usize::try_from(bytes).map_err(|_| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "RAM size '{size}' exceeds host usize capacity"
        ))
    })
}

fn classify_mapped_device(py: Python<'_>, device: &PyObject) -> PyResult<BuiltInMappedDeviceKind> {
    let bound = device.bind(py);
    let ty_name = bound
        .get_type()
        .name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());
    let gic_num_irqs = bound
        .extract::<PyRef<'_, GicV2>>()
        .ok()
        .map(|gic| gic.num_irqs);

    classify_builtin_mapped_device(&ty_name, gic_num_irqs).map_err(platform_error)
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

fn extract_pci_ram_bar<'py>(
    py: Python<'py>,
    entry: &'py MapEntry,
) -> Option<PyRef<'py, PciRamBar>> {
    entry.device.bind(py).extract::<PyRef<'_, PciRamBar>>().ok()
}

fn validate_pci_ram_bar_entry(entry: &MapEntry, bar: &PciRamBar) -> PyResult<()> {
    let size = entry.size as usize;
    if size < 16 || !size.is_power_of_two() {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "PciRamBar {}:{}:{} requires a power-of-two BAR size >= 16 bytes, got {:#x}",
            bar.bus, bar.slot, bar.function, size,
        )));
    }
    if entry.bank != 0 {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "PciRamBar '{}:{}:{}' must use bank=0 for BAR0, got {}",
            bar.bus, bar.slot, bar.function, entry.bank
        )));
    }
    Ok(())
}

fn extract_pci_virtio_rng_mmio<'py>(
    py: Python<'py>,
    entry: &'py MapEntry,
) -> Option<PyRef<'py, PciVirtioRngMmio>> {
    entry
        .device
        .bind(py)
        .extract::<PyRef<'_, PciVirtioRngMmio>>()
        .ok()
}

fn validate_pci_virtio_rng_mmio_entry(entry: &MapEntry, dev: &PciVirtioRngMmio) -> PyResult<()> {
    if entry.size != 0x200 {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "PciVirtioRngMmio {}:{}:{} requires size 0x200, got {:#x}",
            dev.bus, dev.slot, dev.function, entry.size,
        )));
    }
    if entry.bank != 0 {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "PciVirtioRngMmio {}:{}:{} must use bank=0 for BAR0, got {}",
            dev.bus, dev.slot, dev.function, entry.bank
        )));
    }
    Ok(())
}

fn extract_pci_virtio_rng<'py>(
    py: Python<'py>,
    entry: &'py MapEntry,
) -> Option<PyRef<'py, PciVirtioRng>> {
    entry
        .device
        .bind(py)
        .extract::<PyRef<'_, PciVirtioRng>>()
        .ok()
}

fn validate_pci_virtio_rng_entry(entry: &MapEntry, dev: &PciVirtioRng) -> PyResult<()> {
    validate_standard_pci_virtio_entry(entry, dev.bus, dev.slot, dev.function, "PciVirtioRng")
}

fn extract_pci_virtio_blk<'py>(
    py: Python<'py>,
    entry: &'py MapEntry,
) -> Option<PyRef<'py, PciVirtioBlk>> {
    entry
        .device
        .bind(py)
        .extract::<PyRef<'_, PciVirtioBlk>>()
        .ok()
}

fn extract_pci_virtio_net<'py>(
    py: Python<'py>,
    entry: &'py MapEntry,
) -> Option<PyRef<'py, PciVirtioNet>> {
    entry
        .device
        .bind(py)
        .extract::<PyRef<'_, PciVirtioNet>>()
        .ok()
}

fn extract_pci_virtio_console<'py>(
    py: Python<'py>,
    entry: &'py MapEntry,
) -> Option<PyRef<'py, PciVirtioConsole>> {
    entry
        .device
        .bind(py)
        .extract::<PyRef<'_, PciVirtioConsole>>()
        .ok()
}

fn validate_standard_pci_virtio_entry(
    entry: &MapEntry,
    bus: u8,
    slot: u8,
    function: u8,
    label: &str,
) -> PyResult<()> {
    if entry.size != 0x2000 {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "{label} {bus}:{slot}:{function} requires size 0x2000, got {:#x}",
            entry.size,
        )));
    }
    if entry.bank != 0 {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "{label} {bus}:{slot}:{function} must use bank=0 for BAR0, got {}",
            entry.bank,
        )));
    }
    Ok(())
}
