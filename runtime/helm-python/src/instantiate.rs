#![allow(missing_docs)]

use helm_engine::platform::arm_virt::install_arm_virt_pci_bar_device;
use helm_engine::{
    build_simulator_from_request, ExecMode, FrozenSimulatorConfig, Isa, SimulatorBuildRequest,
    TimingChoice,
};
use helm_hw_pci::{build_pci_bar0_endpoint, build_pci_ram_bar_pair, Bdf, PciBus};
use helm_hw_virtio::blk::VirtioBlk;
use helm_hw_virtio::console::VirtioConsole;
use helm_hw_virtio::net::VirtioNet;
use helm_hw_virtio::pci::build_virtio_pci_rng_pair;
use helm_hw_virtio::proto::transport::VirtioMmioTransport;
use helm_hw_virtio::proto::virtqueue::RamBlockBackend;
use helm_hw_virtio::rng::VirtioRng;
use helm_platform::{
    freeze_built_in_discovered_config, BuiltInDiscoveredConfig, BuiltInMappedDevice,
    BuiltInMappedDeviceKind, BuiltInPlatform,
};
use pyo3::prelude::*;

use crate::discovery::{
    collect_port_refs, discover_children, discover_pci_ram_bars, discover_pci_virtio_blk,
    discover_pci_virtio_console, discover_pci_virtio_net, discover_pci_virtio_rng,
    discover_pci_virtio_rng_mmio, parse_ram_size, DiscoveredPciRamBar, DiscoveredPciVirtioBlk,
    DiscoveredPciVirtioConsole, DiscoveredPciVirtioNet, DiscoveredPciVirtioRng,
    DiscoveredPciVirtioRngMmio,
};
use crate::simobject::{SimObject, SimObjectState};
use crate::system::{parse_mode, parse_timing, HelmSystem};

const DEFAULT_MEM_SIZE: usize = 512 * 1024 * 1024;

struct FrozenPythonSystemConfig {
    frozen: FrozenSimulatorConfig,
    pci_ram_bars: Vec<DiscoveredPciRamBar>,
    pci_virtio_rng_mmio: Vec<DiscoveredPciVirtioRngMmio>,
    pci_virtio_rng: Vec<DiscoveredPciVirtioRng>,
    pci_virtio_blk: Vec<DiscoveredPciVirtioBlk>,
    pci_virtio_net: Vec<DiscoveredPciVirtioNet>,
    pci_virtio_console: Vec<DiscoveredPciVirtioConsole>,
}

pub(crate) fn instantiate_system(
    mut system: PyRefMut<'_, HelmSystem>,
    py: Python<'_>,
) -> PyResult<()> {
    let config = {
        let base: &SimObject = system.as_ref();
        base.require_pending()?;

        if system.sim.is_some() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "system is already instantiated",
            ));
        }

        let port_refs = collect_port_refs(py, base);
        if !port_refs.is_empty() {
            log::info!("Port references collected: {} wiring(s)", port_refs.len());
            for (device_name, attr_name, pref) in &port_refs {
                log::debug!("  {}.{} -> {:?}", device_name, attr_name, pref,);
            }
        }

        freeze_system_config(py, &system, base)?
    };

    let mut sim = build_simulator_from_request(config.frozen.request);
    install_pci_ram_bars(&mut sim, &config.pci_ram_bars)?;
    install_pci_virtio_rng_mmio(&mut sim, &config.pci_virtio_rng_mmio)?;
    install_pci_virtio_rng(&mut sim, &config.pci_virtio_rng)?;
    install_pci_virtio_blk(&mut sim, &config.pci_virtio_blk)?;
    install_pci_virtio_net(&mut sim, &config.pci_virtio_net)?;
    install_pci_virtio_console(&mut sim, &config.pci_virtio_console)?;
    system.sim = Some(sim);

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

pub(crate) fn freeze_explicit_system_config(
    isa: &str,
    mode: &str,
    timing: &str,
    mem_base: u64,
    mem_size: usize,
    ipc: f64,
) -> PyResult<FrozenSimulatorConfig> {
    Ok(FrozenSimulatorConfig {
        request: SimulatorBuildRequest::new(
            parse_isa(isa)?,
            parse_mode(mode)?,
            parse_timing(timing, ipc)?,
            mem_base,
            mem_size,
        ),
        mappings: Vec::new(),
    })
}

fn freeze_system_config(
    py: Python<'_>,
    system: &HelmSystem,
    base: &SimObject,
) -> PyResult<FrozenPythonSystemConfig> {
    let discovered = discover_children(py, base)?;
    let pci_ram_bars = discover_pci_ram_bars(py, base)?;
    let pci_virtio_rng_mmio = discover_pci_virtio_rng_mmio(py, base)?;
    let pci_virtio_rng = discover_pci_virtio_rng(py, base)?;
    let pci_virtio_blk = discover_pci_virtio_blk(py, base)?;
    let pci_virtio_net = discover_pci_virtio_net(py, base)?;
    let pci_virtio_console = discover_pci_virtio_console(py, base)?;
    Ok(FrozenPythonSystemConfig {
        frozen: build_from_discovered(system, discovered)?,
        pci_ram_bars,
        pci_virtio_rng_mmio,
        pci_virtio_rng,
        pci_virtio_blk,
        pci_virtio_net,
        pci_virtio_console,
    })
}

fn build_from_discovered(
    system: &HelmSystem,
    mut discovered: BuiltInDiscoveredConfig,
) -> PyResult<FrozenSimulatorConfig> {
    let mode = parse_mode(&system.mode)?;
    let timing = parse_timing(&system.timing, system.ipc)?;
    let isa = discovered
        .cpu_isa
        .as_deref()
        .map(parse_isa)
        .transpose()?
        .unwrap_or(Isa::AArch64);
    let isa_name = match isa {
        Isa::AArch64 => "aarch64",
        Isa::RiscV => "riscv64",
        Isa::AArch32 => "aarch32",
    };
    let defaults = freeze_built_in_discovered_config(
        mode == ExecMode::System,
        isa_name,
        &discovered,
        DEFAULT_MEM_SIZE,
    )
    .map_err(pyo3::exceptions::PyValueError::new_err)?;

    let mut request =
        SimulatorBuildRequest::new(isa, mode, timing, defaults.mem_base, defaults.mem_size);
    if let Some(platform) = defaults.platform {
        request = request.with_platform(platform);
    }

    Ok(FrozenSimulatorConfig {
        request,
        mappings: std::mem::take(&mut discovered.mappings),
    })
}

fn install_pci_ram_bars(
    sim: &mut helm_engine::HelmSim,
    bars: &[DiscoveredPciRamBar],
) -> PyResult<()> {
    if bars.is_empty() {
        return Ok(());
    }

    let result = sim
        .with_system_memory_mut(|sys_mem| install_pci_ram_bars_on_system_memory(sys_mem, bars))
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "PciRamBar requires an instantiated AArch64 system board",
            )
        })?;
    result.map_err(pyo3::exceptions::PyValueError::new_err)
}

fn install_pci_virtio_rng_mmio(
    sim: &mut helm_engine::HelmSim,
    devices: &[DiscoveredPciVirtioRngMmio],
) -> PyResult<()> {
    if devices.is_empty() {
        return Ok(());
    }

    let result = sim
        .with_system_memory_mut(|sys_mem| {
            install_pci_virtio_rng_mmio_on_system_memory(sys_mem, devices)
        })
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "PciVirtioRngMmio requires an instantiated AArch64 system board",
            )
        })?;
    result.map_err(pyo3::exceptions::PyValueError::new_err)
}

fn install_pci_virtio_rng(
    sim: &mut helm_engine::HelmSim,
    devices: &[DiscoveredPciVirtioRng],
) -> PyResult<()> {
    if devices.is_empty() {
        return Ok(());
    }

    let result = sim
        .with_system_memory_mut(|sys_mem| install_pci_virtio_rng_on_system_memory(sys_mem, devices))
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "PciVirtioRng requires an instantiated AArch64 system board",
            )
        })?;
    result.map_err(pyo3::exceptions::PyValueError::new_err)
}

fn install_pci_virtio_blk(
    sim: &mut helm_engine::HelmSim,
    devices: &[DiscoveredPciVirtioBlk],
) -> PyResult<()> {
    if devices.is_empty() {
        return Ok(());
    }

    let result = sim
        .with_system_memory_mut(|sys_mem| install_pci_virtio_blk_on_system_memory(sys_mem, devices))
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "PciVirtioBlk requires an instantiated AArch64 system board",
            )
        })?;
    result.map_err(pyo3::exceptions::PyValueError::new_err)
}

fn install_pci_virtio_net(
    sim: &mut helm_engine::HelmSim,
    devices: &[DiscoveredPciVirtioNet],
) -> PyResult<()> {
    if devices.is_empty() {
        return Ok(());
    }

    let result = sim
        .with_system_memory_mut(|sys_mem| install_pci_virtio_net_on_system_memory(sys_mem, devices))
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "PciVirtioNet requires an instantiated AArch64 system board",
            )
        })?;
    result.map_err(pyo3::exceptions::PyValueError::new_err)
}

fn install_pci_virtio_console(
    sim: &mut helm_engine::HelmSim,
    devices: &[DiscoveredPciVirtioConsole],
) -> PyResult<()> {
    if devices.is_empty() {
        return Ok(());
    }

    let result = sim
        .with_system_memory_mut(|sys_mem| {
            install_pci_virtio_console_on_system_memory(sys_mem, devices)
        })
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "PciVirtioConsole requires an instantiated AArch64 system board",
            )
        })?;
    result.map_err(pyo3::exceptions::PyValueError::new_err)
}

fn install_pci_ram_bars_on_system_memory(
    sys_mem: &mut helm_engine::address_space::HelmAddressSpace,
    bars: &[DiscoveredPciRamBar],
) -> Result<(), String> {
    let pci_idx = find_pci_bus_index(sys_mem)
        .ok_or_else(|| "built-in platform does not expose a live PCI bus".to_string())?;

    for bar in bars {
        let (endpoint, device) = build_pci_ram_bar_pair(
            bar.vendor_id,
            bar.device_id,
            bar.class_code,
            bar.base,
            bar.size as u64,
        )?;
        let bdf = Bdf::new(bar.bus, bar.slot, bar.function);
        let attach = sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(bdf, Box::new(endpoint))
            })
            .ok_or_else(|| "built-in PCI bus is not available".to_string())?;
        attach.map_err(|e| format!("failed to attach PCI function at {:?}: {e}", bdf))?;

        install_arm_virt_pci_bar_device(sys_mem, bdf, 0, bar.base, 0, Box::new(device))
            .ok_or_else(|| format!("failed to register PCI BAR0 MMIO window at {:#x}", bar.base))?;
    }

    Ok(())
}

fn install_pci_virtio_rng_mmio_on_system_memory(
    sys_mem: &mut helm_engine::address_space::HelmAddressSpace,
    devices: &[DiscoveredPciVirtioRngMmio],
) -> Result<(), String> {
    let pci_idx = find_pci_bus_index(sys_mem)
        .ok_or_else(|| "built-in platform does not expose a live PCI bus".to_string())?;

    for dev in devices {
        let endpoint = build_pci_bar0_endpoint(
            dev.vendor_id,
            dev.device_id,
            dev.class_code,
            dev.base,
            0x200,
        )?;
        let bdf = Bdf::new(dev.bus, dev.slot, dev.function);
        let attach = sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(bdf, Box::new(endpoint))
            })
            .ok_or_else(|| "built-in PCI bus is not available".to_string())?;
        attach.map_err(|e| format!("failed to attach PCI function at {:?}: {e}", bdf))?;

        let transport = VirtioMmioTransport::new(Box::new(VirtioRng::with_seed(dev.seed)));
        install_arm_virt_pci_bar_device(sys_mem, bdf, 0, dev.base, 0, Box::new(transport))
            .ok_or_else(|| {
                format!(
                    "failed to register PciVirtioRngMmio BAR0 window at {:#x}",
                    dev.base
                )
            })?;
    }

    Ok(())
}

fn install_pci_virtio_rng_on_system_memory(
    sys_mem: &mut helm_engine::address_space::HelmAddressSpace,
    devices: &[DiscoveredPciVirtioRng],
) -> Result<(), String> {
    let pci_idx = find_pci_bus_index(sys_mem)
        .ok_or_else(|| "built-in platform does not expose a live PCI bus".to_string())?;

    for dev in devices {
        let (endpoint, bar0, bar4) = build_virtio_pci_rng_pair(dev.base, dev.seed)?;
        let bdf = Bdf::new(dev.bus, dev.slot, dev.function);
        let attach = sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(bdf, Box::new(endpoint))
            })
            .ok_or_else(|| "built-in PCI bus is not available".to_string())?;
        attach.map_err(|e| format!("failed to attach PCI function at {:?}: {e}", bdf))?;

        install_arm_virt_pci_bar_device(sys_mem, bdf, 0, dev.base, 0, Box::new(bar0)).ok_or_else(
            || format!("failed to register PciVirtioRng BAR0 window at {:#x}", dev.base),
        )?;
        install_arm_virt_pci_bar_device(sys_mem, bdf, 4, dev.base + 0x1000, 0, Box::new(bar4))
            .ok_or_else(|| {
                format!(
                    "failed to register PciVirtioRng BAR4 MSI-X window at {:#x}",
                    dev.base + 0x1000
                )
            })?;
    }

    Ok(())
}

fn install_pci_virtio_blk_on_system_memory(
    sys_mem: &mut helm_engine::address_space::HelmAddressSpace,
    devices: &[DiscoveredPciVirtioBlk],
) -> Result<(), String> {
    let pci_idx = find_pci_bus_index(sys_mem)
        .ok_or_else(|| "built-in platform does not expose a live PCI bus".to_string())?;

    for dev in devices {
        let disk = RamBlockBackend::zeroed(dev.capacity_bytes);
        let (endpoint, bar0, bar4) = helm_hw_virtio::pci::build_virtio_pci_pair(
            Box::new(VirtioBlk::new(Box::new(disk), dev.read_only)),
            dev.base,
        )?;
        let bdf = Bdf::new(dev.bus, dev.slot, dev.function);
        let attach = sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(bdf, Box::new(endpoint))
            })
            .ok_or_else(|| "built-in PCI bus is not available".to_string())?;
        attach.map_err(|e| format!("failed to attach PCI function at {:?}: {e}", bdf))?;

        install_arm_virt_pci_bar_device(sys_mem, bdf, 0, dev.base, 0, Box::new(bar0)).ok_or_else(
            || format!("failed to register PciVirtioBlk BAR0 window at {:#x}", dev.base),
        )?;
        install_arm_virt_pci_bar_device(sys_mem, bdf, 4, dev.base + 0x1000, 0, Box::new(bar4))
            .ok_or_else(|| {
                format!(
                    "failed to register PciVirtioBlk BAR4 MSI-X window at {:#x}",
                    dev.base + 0x1000
                )
            })?;
    }

    Ok(())
}

fn install_pci_virtio_net_on_system_memory(
    sys_mem: &mut helm_engine::address_space::HelmAddressSpace,
    devices: &[DiscoveredPciVirtioNet],
) -> Result<(), String> {
    let pci_idx = find_pci_bus_index(sys_mem)
        .ok_or_else(|| "built-in platform does not expose a live PCI bus".to_string())?;

    for dev in devices {
        let mac = parse_mac(&dev.mac)?;
        let (endpoint, bar0, bar4) =
            helm_hw_virtio::pci::build_virtio_pci_pair(Box::new(VirtioNet::new(mac)), dev.base)?;
        let bdf = Bdf::new(dev.bus, dev.slot, dev.function);
        let attach = sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(bdf, Box::new(endpoint))
            })
            .ok_or_else(|| "built-in PCI bus is not available".to_string())?;
        attach.map_err(|e| format!("failed to attach PCI function at {:?}: {e}", bdf))?;

        install_arm_virt_pci_bar_device(sys_mem, bdf, 0, dev.base, 0, Box::new(bar0)).ok_or_else(
            || format!("failed to register PciVirtioNet BAR0 window at {:#x}", dev.base),
        )?;
        install_arm_virt_pci_bar_device(sys_mem, bdf, 4, dev.base + 0x1000, 0, Box::new(bar4))
            .ok_or_else(|| {
                format!(
                    "failed to register PciVirtioNet BAR4 MSI-X window at {:#x}",
                    dev.base + 0x1000
                )
            })?;
    }

    Ok(())
}

fn install_pci_virtio_console_on_system_memory(
    sys_mem: &mut helm_engine::address_space::HelmAddressSpace,
    devices: &[DiscoveredPciVirtioConsole],
) -> Result<(), String> {
    let pci_idx = find_pci_bus_index(sys_mem)
        .ok_or_else(|| "built-in platform does not expose a live PCI bus".to_string())?;

    for dev in devices {
        let backend = make_console_backend(&dev.serial)?;
        let console = VirtioConsole::with_size(backend, dev.cols, dev.rows);
        let (endpoint, bar0, bar4) =
            helm_hw_virtio::pci::build_virtio_pci_pair(Box::new(console), dev.base)?;
        let bdf = Bdf::new(dev.bus, dev.slot, dev.function);
        let attach = sys_mem
            .with_device_mut::<PciBus, _>(pci_idx, |bus| {
                bus.attach_endpoint(bdf, Box::new(endpoint))
            })
            .ok_or_else(|| "built-in PCI bus is not available".to_string())?;
        attach.map_err(|e| format!("failed to attach PCI function at {:?}: {e}", bdf))?;

        install_arm_virt_pci_bar_device(sys_mem, bdf, 0, dev.base, 0, Box::new(bar0)).ok_or_else(
            || format!("failed to register PciVirtioConsole BAR0 window at {:#x}", dev.base),
        )?;
        install_arm_virt_pci_bar_device(sys_mem, bdf, 4, dev.base + 0x1000, 0, Box::new(bar4))
            .ok_or_else(|| {
                format!(
                    "failed to register PciVirtioConsole BAR4 MSI-X window at {:#x}",
                    dev.base + 0x1000
                )
            })?;
    }

    Ok(())
}

fn parse_mac(mac: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 {
        return Err(format!("invalid MAC '{mac}': expected 6 octets"));
    }

    let mut bytes = [0u8; 6];
    for (idx, part) in parts.iter().enumerate() {
        bytes[idx] = u8::from_str_radix(part, 16)
            .map_err(|e| format!("invalid MAC '{mac}' octet '{}': {e}", part))?;
    }
    Ok(bytes)
}

fn make_console_backend(serial: &str) -> Result<Box<dyn helm_devices::CharBackend>, String> {
    match serial {
        "null" => Ok(Box::new(helm_devices::NullCharBackend)),
        "stdio" => Ok(Box::new(helm_engine::platform::arm_virt::StdioCharBackend)),
        other => Err(format!(
            "unknown PciVirtioConsole serial backend '{other}' (expected 'null' or 'stdio')"
        )),
    }
}

fn find_pci_bus_index(sys_mem: &mut helm_engine::address_space::HelmAddressSpace) -> Option<usize> {
    (0..sys_mem.devices.len()).find(|&idx| sys_mem.device_as_mut::<PciBus>(idx).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{
        PciRamBar, PciVirtioBlk, PciVirtioConsole, PciVirtioNet, PciVirtioRng, PciVirtioRngMmio,
    };
    use crate::memory_space::{MapEntry, MemorySpace};
    use crate::simobject::SimObject;
    use helm_core::{AccessType, MemInterface};
    use helm_platform::aarch64::virt::{MMIO_BASE, PCIE_ECAM_BASE};
    use indexmap::IndexMap;

    fn system(mode: &str) -> HelmSystem {
        HelmSystem {
            timing: "virtual".into(),
            mode: mode.into(),
            ipc: 4.0,
            sim: None,
            exited: false,
            exit_code_val: 0,
            plugins: Vec::new(),
        }
    }

    fn base_with_pci_ram_bar(py: Python<'_>) -> (HelmSystem, SimObject) {
        let pci = Py::new(
            py,
            (
                PciRamBar {
                    vendor_id: 0xCAFE,
                    device_id: 0x0001,
                    class_code: 0xFF0000,
                    bus: 0,
                    slot: 1,
                    function: 0,
                },
                SimObject::new("pci-bar0"),
            ),
        )
        .unwrap();

        let mem = Py::new(
            py,
            (
                MemorySpace {
                    entries: vec![MapEntry {
                        base: MMIO_BASE,
                        device: pci.to_object(py),
                        size: 0x1000,
                        bank: 0,
                    }],
                },
                SimObject::new("phys_mem"),
            ),
        )
        .unwrap();

        let mut base = SimObject::new("sys");
        base.children = IndexMap::from([("mem".to_string(), mem.to_object(py))]);
        (system("fs"), base)
    }

    fn base_with_pci_virtio_rng_mmio(py: Python<'_>) -> (HelmSystem, SimObject) {
        let rng = Py::new(
            py,
            (
                PciVirtioRngMmio {
                    vendor_id: 0xCAFE,
                    device_id: 0x1004,
                    class_code: 0xFF0000,
                    bus: 0,
                    slot: 2,
                    function: 0,
                    seed: 0x1234_5678,
                },
                SimObject::new("pci-rng0"),
            ),
        )
        .unwrap();

        let mem = Py::new(
            py,
            (
                MemorySpace {
                    entries: vec![MapEntry {
                        base: MMIO_BASE + 0x1000,
                        device: rng.to_object(py),
                        size: 0x200,
                        bank: 0,
                    }],
                },
                SimObject::new("phys_mem"),
            ),
        )
        .unwrap();

        let mut base = SimObject::new("sys");
        base.children = IndexMap::from([("mem".to_string(), mem.to_object(py))]);
        (system("fs"), base)
    }

    fn base_with_pci_virtio_rng(py: Python<'_>) -> (HelmSystem, SimObject) {
        let rng = Py::new(
            py,
            (
                PciVirtioRng {
                    bus: 0,
                    slot: 3,
                    function: 0,
                    seed: 0x1234_5678,
                },
                SimObject::new("pci-rng1"),
            ),
        )
        .unwrap();

        let mem = Py::new(
            py,
            (
                MemorySpace {
                    entries: vec![MapEntry {
                        base: MMIO_BASE + 0x3000,
                        device: rng.to_object(py),
                        size: 0x2000,
                        bank: 0,
                    }],
                },
                SimObject::new("phys_mem"),
            ),
        )
        .unwrap();

        let mut base = SimObject::new("sys");
        base.children = IndexMap::from([("mem".to_string(), mem.to_object(py))]);
        (system("fs"), base)
    }

    fn base_with_pci_virtio_standard_devices(py: Python<'_>) -> (HelmSystem, SimObject) {
        let blk = Py::new(
            py,
            (
                PciVirtioBlk {
                    bus: 0,
                    slot: 4,
                    function: 0,
                    capacity_bytes: 4096,
                    read_only: false,
                },
                SimObject::new("pci-blk0"),
            ),
        )
        .unwrap();
        let net = Py::new(
            py,
            (
                PciVirtioNet {
                    bus: 0,
                    slot: 5,
                    function: 0,
                    mac: "52:54:00:12:34:56".to_string(),
                },
                SimObject::new("pci-net0"),
            ),
        )
        .unwrap();
        let console = Py::new(
            py,
            (
                PciVirtioConsole {
                    bus: 0,
                    slot: 6,
                    function: 0,
                    serial: "null".to_string(),
                    cols: 100,
                    rows: 40,
                },
                SimObject::new("pci-console0"),
            ),
        )
        .unwrap();

        let mem = Py::new(
            py,
            (
                MemorySpace {
                    entries: vec![
                        MapEntry {
                            base: MMIO_BASE + 0x5000,
                            device: blk.to_object(py),
                            size: 0x2000,
                            bank: 0,
                        },
                        MapEntry {
                            base: MMIO_BASE + 0x8000,
                            device: net.to_object(py),
                            size: 0x2000,
                            bank: 0,
                        },
                        MapEntry {
                            base: MMIO_BASE + 0xB000,
                            device: console.to_object(py),
                            size: 0x2000,
                            bank: 0,
                        },
                    ],
                },
                SimObject::new("phys_mem"),
            ),
        )
        .unwrap();

        let mut base = SimObject::new("sys");
        base.children = IndexMap::from([("mem".to_string(), mem.to_object(py))]);
        (system("fs"), base)
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
        let cfg = build_from_discovered(&system("se"), BuiltInDiscoveredConfig::default()).unwrap();
        assert!(matches!(cfg.request.isa, Isa::AArch64));
        assert!(cfg.request.platform.is_none());
        assert_eq!(cfg.request.mem_base, 0);
        assert_eq!(cfg.request.mem_size, DEFAULT_MEM_SIZE);
        assert!(matches!(
            cfg.request.timing,
            TimingChoice::VirtualTiming { .. }
        ));
    }

    #[test]
    fn system_mode_uses_platform_ram_base_by_default() {
        let cfg = build_from_discovered(&system("fs"), BuiltInDiscoveredConfig::default()).unwrap();
        assert_eq!(cfg.request.platform, Some(BuiltInPlatform::ArmVirt));
        assert_eq!(
            cfg.request.mem_base,
            BuiltInPlatform::ArmVirt.default_ram_base()
        );
    }

    #[test]
    fn memory_space_mapping_wins_over_direct_ram_size() {
        let discovered = BuiltInDiscoveredConfig {
            cpu_isa: Some("riscv64".into()),
            direct_ram_size: Some(64 * 1024 * 1024),
            mapped_ram: Some((0x4000_0000, 128 * 1024 * 1024)),
            mappings: vec![BuiltInMappedDevice {
                base: 0x4000_0000,
                size: 128 * 1024 * 1024,
                bank: 0,
                kind: BuiltInMappedDeviceKind::Ram,
            }],
        };
        let cfg = build_from_discovered(&system("se"), discovered).unwrap();
        assert!(matches!(cfg.request.isa, Isa::RiscV));
        assert_eq!(cfg.request.mem_base, 0x4000_0000);
        assert_eq!(cfg.request.mem_size, 128 * 1024 * 1024);
        assert_eq!(cfg.mappings.len(), 1);
    }

    #[test]
    fn overlapping_mappings_are_rejected() {
        pyo3::prepare_freethreaded_python();
        let discovered = BuiltInDiscoveredConfig {
            cpu_isa: None,
            direct_ram_size: None,
            mapped_ram: Some((0x1000, 0x1000)),
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
        };
        let err = build_from_discovered(&system("se"), discovered)
            .err()
            .expect("overlap should fail");
        assert!(err.to_string().contains("overlapping memory mappings"));
    }

    #[test]
    fn system_mode_gic_mapping_must_match_platform_layout() {
        pyo3::prepare_freethreaded_python();
        let discovered = BuiltInDiscoveredConfig {
            cpu_isa: Some("aarch64".into()),
            direct_ram_size: None,
            mapped_ram: Some((
                BuiltInPlatform::ArmVirt.default_ram_base(),
                128 * 1024 * 1024,
            )),
            mappings: vec![
                BuiltInMappedDevice {
                    base: BuiltInPlatform::ArmVirt.default_ram_base(),
                    size: 128 * 1024 * 1024,
                    bank: 0,
                    kind: BuiltInMappedDeviceKind::Ram,
                },
                BuiltInMappedDevice {
                    base: 0x0900_0000,
                    size: 0x1000,
                    bank: 0,
                    kind: BuiltInMappedDeviceKind::GicV2 { num_irqs: 96 },
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
        let discovered = BuiltInDiscoveredConfig {
            cpu_isa: Some("aarch64".into()),
            direct_ram_size: None,
            mapped_ram: Some((
                BuiltInPlatform::ArmVirt.default_ram_base(),
                128 * 1024 * 1024,
            )),
            mappings: vec![
                BuiltInMappedDevice {
                    base: BuiltInPlatform::ArmVirt.default_ram_base(),
                    size: 128 * 1024 * 1024,
                    bank: 0,
                    kind: BuiltInMappedDeviceKind::Ram,
                },
                BuiltInMappedDevice {
                    base: 0x0900_1000,
                    size: 0x1000,
                    bank: 0,
                    kind: BuiltInMappedDeviceKind::Unknown {
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

    #[test]
    fn freeze_system_config_collects_pci_ram_bar_plan() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (system, base) = base_with_pci_ram_bar(py);
            let cfg = freeze_system_config(py, &system, &base).unwrap();
            assert_eq!(cfg.pci_ram_bars.len(), 1);
            assert_eq!(cfg.pci_ram_bars[0].base, MMIO_BASE);
            assert_eq!(cfg.frozen.request.platform, Some(BuiltInPlatform::ArmVirt));
        });
    }

    #[test]
    fn freeze_system_config_collects_pci_virtio_rng_mmio_plan() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (system, base) = base_with_pci_virtio_rng_mmio(py);
            let cfg = freeze_system_config(py, &system, &base).unwrap();
            assert_eq!(cfg.pci_virtio_rng_mmio.len(), 1);
            assert_eq!(cfg.pci_virtio_rng_mmio[0].base, MMIO_BASE + 0x1000);
            assert_eq!(cfg.frozen.request.platform, Some(BuiltInPlatform::ArmVirt));
        });
    }

    #[test]
    fn freeze_system_config_collects_pci_virtio_rng_plan() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (system, base) = base_with_pci_virtio_rng(py);
            let cfg = freeze_system_config(py, &system, &base).unwrap();
            assert_eq!(cfg.pci_virtio_rng.len(), 1);
            assert_eq!(cfg.pci_virtio_rng[0].base, MMIO_BASE + 0x3000);
            assert_eq!(cfg.frozen.request.platform, Some(BuiltInPlatform::ArmVirt));
        });
    }

    #[test]
    fn freeze_system_config_collects_standard_virtio_pci_plans() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let (system, base) = base_with_pci_virtio_standard_devices(py);
            let cfg = freeze_system_config(py, &system, &base).unwrap();
            assert_eq!(cfg.pci_virtio_blk.len(), 1);
            assert_eq!(cfg.pci_virtio_net.len(), 1);
            assert_eq!(cfg.pci_virtio_console.len(), 1);
            assert_eq!(cfg.pci_virtio_blk[0].base, MMIO_BASE + 0x5000);
            assert_eq!(cfg.pci_virtio_net[0].base, MMIO_BASE + 0x8000);
            assert_eq!(cfg.pci_virtio_console[0].base, MMIO_BASE + 0xB000);
        });
    }

    #[test]
    fn install_pci_ram_bars_attaches_live_function_and_bar() {
        let mut sim = build_simulator_from_request(
            SimulatorBuildRequest::new(
                Isa::AArch64,
                ExecMode::System,
                TimingChoice::VirtualTiming { ipc: 1.0 },
                BuiltInPlatform::ArmVirt.default_ram_base(),
                0x20_0000,
            )
            .with_platform(BuiltInPlatform::ArmVirt),
        );

        install_pci_ram_bars(
            &mut sim,
            &[DiscoveredPciRamBar {
                base: MMIO_BASE,
                size: 0x1000,
                bus: 0,
                slot: 1,
                function: 0,
                vendor_id: 0xCAFE,
                device_id: 0x0001,
                class_code: 0xFF0000,
            }],
        )
        .unwrap();

        sim.with_system_memory_mut(|sys| {
            let vendor_device = sys
                .read(PCIE_ECAM_BASE + (1u64 << 15), 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(vendor_device, 0x0001_CAFE);
            sys.write(MMIO_BASE + 0x20, 4, 0x1122_3344, AccessType::Store)
                .unwrap();
            assert_eq!(
                sys.read(MMIO_BASE + 0x20, 4, AccessType::Load).unwrap(),
                0x1122_3344
            );
        })
        .expect("system memory should be available");
    }

    #[test]
    fn install_pci_virtio_rng_mmio_attaches_live_function_and_transport_bar() {
        let mut sim = build_simulator_from_request(
            SimulatorBuildRequest::new(
                Isa::AArch64,
                ExecMode::System,
                TimingChoice::VirtualTiming { ipc: 1.0 },
                BuiltInPlatform::ArmVirt.default_ram_base(),
                0x20_0000,
            )
            .with_platform(BuiltInPlatform::ArmVirt),
        );

        install_pci_virtio_rng_mmio(
            &mut sim,
            &[DiscoveredPciVirtioRngMmio {
                base: MMIO_BASE + 0x1000,
                bus: 0,
                slot: 2,
                function: 0,
                vendor_id: 0xCAFE,
                device_id: 0x1004,
                class_code: 0xFF0000,
                seed: 0x1234_5678,
            }],
        )
        .unwrap();

        sim.with_system_memory_mut(|sys| {
            let vendor_device = sys
                .read(PCIE_ECAM_BASE + (2u64 << 15), 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(vendor_device, 0x1004_CAFE);
            let magic = sys.read(MMIO_BASE + 0x1000, 4, AccessType::Load).unwrap() as u32;
            assert_eq!(magic, 0x7472_6976);
        })
        .expect("system memory should be available");
    }

    #[test]
    fn install_pci_virtio_rng_attaches_live_standard_transport() {
        let mut sim = build_simulator_from_request(
            SimulatorBuildRequest::new(
                Isa::AArch64,
                ExecMode::System,
                TimingChoice::VirtualTiming { ipc: 1.0 },
                BuiltInPlatform::ArmVirt.default_ram_base(),
                0x20_0000,
            )
            .with_platform(BuiltInPlatform::ArmVirt),
        );

        install_pci_virtio_rng(
            &mut sim,
            &[DiscoveredPciVirtioRng {
                base: MMIO_BASE + 0x3000,
                bus: 0,
                slot: 3,
                function: 0,
                seed: 0x1234_5678,
            }],
        )
        .unwrap();

        sim.with_system_memory_mut(|sys| {
            let vendor_device = sys
                .read(PCIE_ECAM_BASE + (3u64 << 15), 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(vendor_device, 0x1044_1AF4);
            let cap_ptr = sys
                .read(PCIE_ECAM_BASE + (3u64 << 15) + 0x34, 1, AccessType::Load)
                .unwrap();
            assert_eq!(cap_ptr, 0x40);
            sys.write(MMIO_BASE + 0x3000, 4, 1, AccessType::Store)
                .unwrap();
            let features = sys
                .read(MMIO_BASE + 0x3000 + 0x04, 4, AccessType::Load)
                .unwrap();
            assert_eq!(features, 1);
            let msix_cap = sys
                .read(PCIE_ECAM_BASE + (3u64 << 15) + 0x90, 1, AccessType::Load)
                .unwrap();
            assert_eq!(msix_cap, 0x11);
            sys.write(MMIO_BASE + 0x3000 + 0x1010, 4, 0xFEE0_0000, AccessType::Store)
                .unwrap();
            let msix_addr = sys
                .read(MMIO_BASE + 0x3000 + 0x1010, 4, AccessType::Load)
                .unwrap();
            assert_eq!(msix_addr, 0xFEE0_0000);
        })
        .expect("system memory should be available");
    }

    #[test]
    fn install_standard_virtio_pci_devices_attach_live_transports() {
        let mut sim = build_simulator_from_request(
            SimulatorBuildRequest::new(
                Isa::AArch64,
                ExecMode::System,
                TimingChoice::VirtualTiming { ipc: 1.0 },
                BuiltInPlatform::ArmVirt.default_ram_base(),
                0x20_0000,
            )
            .with_platform(BuiltInPlatform::ArmVirt),
        );

        install_pci_virtio_blk(
            &mut sim,
            &[DiscoveredPciVirtioBlk {
                base: MMIO_BASE + 0x5000,
                bus: 0,
                slot: 4,
                function: 0,
                capacity_bytes: 4096,
                read_only: false,
            }],
        )
        .unwrap();
        install_pci_virtio_net(
            &mut sim,
            &[DiscoveredPciVirtioNet {
                base: MMIO_BASE + 0x8000,
                bus: 0,
                slot: 5,
                function: 0,
                mac: "52:54:00:12:34:56".to_string(),
            }],
        )
        .unwrap();
        install_pci_virtio_console(
            &mut sim,
            &[DiscoveredPciVirtioConsole {
                base: MMIO_BASE + 0xB000,
                bus: 0,
                slot: 6,
                function: 0,
                serial: "null".to_string(),
                cols: 100,
                rows: 40,
            }],
        )
        .unwrap();

        sim.with_system_memory_mut(|sys| {
            let blk_vendor_device = sys
                .read(PCIE_ECAM_BASE + (4u64 << 15), 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(blk_vendor_device, 0x1042_1AF4);
            let blk_capacity = sys
                .read(MMIO_BASE + 0x5000 + 0x100, 4, AccessType::Load)
                .unwrap();
            assert_eq!(blk_capacity, 8);
            let blk_msix_cap = sys
                .read(PCIE_ECAM_BASE + (4u64 << 15) + 0x90, 1, AccessType::Load)
                .unwrap();
            assert_eq!(blk_msix_cap, 0x11);
            sys.write(MMIO_BASE + 0x5000 + 0x1010, 4, 0xFEE0_0000, AccessType::Store)
                .unwrap();
            let blk_msix_addr = sys
                .read(MMIO_BASE + 0x5000 + 0x1010, 4, AccessType::Load)
                .unwrap();
            assert_eq!(blk_msix_addr, 0xFEE0_0000);

            let net_vendor_device = sys
                .read(PCIE_ECAM_BASE + (5u64 << 15), 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(net_vendor_device, 0x1041_1AF4);
            let net_mac = sys
                .read(MMIO_BASE + 0x8000 + 0x100, 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(net_mac, u32::from_le_bytes([0x52, 0x54, 0x00, 0x12]));

            let console_vendor_device = sys
                .read(PCIE_ECAM_BASE + (6u64 << 15), 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(console_vendor_device, 0x1043_1AF4);
            let console_cfg = sys
                .read(MMIO_BASE + 0xB000 + 0x100, 4, AccessType::Load)
                .unwrap() as u32;
            assert_eq!(console_cfg & 0xFFFF, 100);
            assert_eq!(console_cfg >> 16, 40);
        })
        .expect("system memory should be available");
    }
}
