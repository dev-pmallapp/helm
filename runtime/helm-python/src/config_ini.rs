//! Helper for emitting gem5-style `[system.<obj>]` parameter
//! sections in `config.ini`. Walks a `HelmSystem`'s child
//! `SimObject` tree and produces an ordered list of
//! `(section, type_name, params)` tuples that the helm-report
//! writer (`emit_config_ini_with_params`) folds into the
//! per-object INI sections alongside the registered metric leaves.
//!
//! Today the walker recognizes the pyclasses already shipped
//! (`Cpu`, `Ram`, `Pl011`, `GicV2`, `MemorySpace`, and the PCI
//! / VirtIO transport descriptors). Unknown subclasses fall back
//! to a generic `SimObject` row so they still appear in the file.
//!
//! The walker is intentionally read-only: parameter values are
//! pre-rendered to short ASCII strings here so the writer never
//! has to know about Rust types.

#![cfg(feature = "report")]

use crate::cache::Cache;
use crate::cpu::Cpu;
use crate::devices::{
    GicV2, PciRamBar, PciVirtioBlk, PciVirtioConsole, PciVirtioNet, PciVirtioRng,
    PciVirtioRngMmio, Pl011,
};
use crate::memory_space::MemorySpace;
use crate::ram::Ram;
use crate::simobject::SimObject;
use pyo3::prelude::*;

/// Each entry: `(section_path, type_name, [(leaf, value), ...])`.
pub(crate) type ConfigIniParams = Vec<(String, String, Vec<(String, String)>)>;

/// Walk the SimObject children of `base` and emit a parameter
/// block per child plus a top-level `[system]` block describing
/// the `HelmSystem` itself. Section names use `system` as the
/// root and child names verbatim, matching the producer-walker
/// naming convention (`system.<child_name>`).
pub(crate) fn collect_params(
    py: Python<'_>,
    base: &SimObject,
    timing: &str,
    mode: &str,
    ipc: f64,
    num_cpus: usize,
    gic_version: &str,
) -> ConfigIniParams {
    let mut out: ConfigIniParams = Vec::new();

    // Top-level [system] block. Mirrors gem5's root-object section
    // with the headline configuration knobs the user supplied.
    out.push((
        "system".to_string(),
        "System".to_string(),
        vec![
            ("timing".to_string(), timing.to_string()),
            ("mode".to_string(), mode.to_string()),
            ("ipc".to_string(), format!("{ipc}")),
            ("num_cpus".to_string(), format!("{num_cpus}")),
            ("gic_version".to_string(), gic_version.to_string()),
        ],
    ));

    for (child_name, child_obj) in &base.children {
        let path = format!("system.{child_name}");
        if let Ok(cell) = child_obj.downcast_bound::<Cpu>(py) {
            let cpu = cell.borrow();
            out.push((
                path,
                "Cpu".to_string(),
                vec![
                    ("isa".to_string(), cpu.isa.clone()),
                    ("model".to_string(), cpu.model.clone()),
                    ("width".to_string(), format!("{}", cpu.width)),
                    ("rob_size".to_string(), format!("{}", cpu.rob_size)),
                    ("iq_size".to_string(), format!("{}", cpu.iq_size)),
                    ("lq_size".to_string(), format!("{}", cpu.lq_size)),
                    ("sq_size".to_string(), format!("{}", cpu.sq_size)),
                ],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<Ram>(py) {
            let ram = cell.borrow();
            out.push((
                path,
                "Ram".to_string(),
                vec![("size".to_string(), ram.size.clone())],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<GicV2>(py) {
            let gic = cell.borrow();
            out.push((
                path,
                "GicV2".to_string(),
                vec![("num_irqs".to_string(), format!("{}", gic.num_irqs))],
            ));
        } else if child_obj.downcast_bound::<Pl011>(py).is_ok() {
            // Pl011 has no user-tunable knobs today; emit a stub
            // so the section still lists the device class.
            out.push((path, "Pl011".to_string(), Vec::new()));
        } else if let Ok(cell) = child_obj.downcast_bound::<PciRamBar>(py) {
            let dev = cell.borrow();
            out.push((
                path,
                "PciRamBar".to_string(),
                vec![
                    ("vendor_id".to_string(), format!("0x{:04x}", dev.vendor_id)),
                    ("device_id".to_string(), format!("0x{:04x}", dev.device_id)),
                    ("class_code".to_string(), format!("0x{:06x}", dev.class_code)),
                    ("bus".to_string(), format!("{}", dev.bus)),
                    ("slot".to_string(), format!("{}", dev.slot)),
                    ("function".to_string(), format!("{}", dev.function)),
                ],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<PciVirtioRngMmio>(py) {
            let dev = cell.borrow();
            out.push((
                path,
                "PciVirtioRngMmio".to_string(),
                vec![
                    ("vendor_id".to_string(), format!("0x{:04x}", dev.vendor_id)),
                    ("device_id".to_string(), format!("0x{:04x}", dev.device_id)),
                    ("class_code".to_string(), format!("0x{:06x}", dev.class_code)),
                    ("bus".to_string(), format!("{}", dev.bus)),
                    ("slot".to_string(), format!("{}", dev.slot)),
                    ("function".to_string(), format!("{}", dev.function)),
                    ("seed".to_string(), format!("{}", dev.seed)),
                ],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<PciVirtioRng>(py) {
            let dev = cell.borrow();
            out.push((
                path,
                "PciVirtioRng".to_string(),
                vec![
                    ("bus".to_string(), format!("{}", dev.bus)),
                    ("slot".to_string(), format!("{}", dev.slot)),
                    ("function".to_string(), format!("{}", dev.function)),
                    ("seed".to_string(), format!("{}", dev.seed)),
                ],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<PciVirtioBlk>(py) {
            let dev = cell.borrow();
            out.push((
                path,
                "PciVirtioBlk".to_string(),
                vec![
                    ("bus".to_string(), format!("{}", dev.bus)),
                    ("slot".to_string(), format!("{}", dev.slot)),
                    ("function".to_string(), format!("{}", dev.function)),
                    (
                        "capacity_bytes".to_string(),
                        format!("{}", dev.capacity_bytes),
                    ),
                    ("read_only".to_string(), format!("{}", dev.read_only)),
                ],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<PciVirtioNet>(py) {
            let dev = cell.borrow();
            out.push((
                path,
                "PciVirtioNet".to_string(),
                vec![
                    ("bus".to_string(), format!("{}", dev.bus)),
                    ("slot".to_string(), format!("{}", dev.slot)),
                    ("function".to_string(), format!("{}", dev.function)),
                    ("mac".to_string(), dev.mac.clone()),
                ],
            ));
        } else if let Ok(cell) = child_obj.downcast_bound::<PciVirtioConsole>(py) {
            let dev = cell.borrow();
            out.push((
                path,
                "PciVirtioConsole".to_string(),
                vec![
                    ("bus".to_string(), format!("{}", dev.bus)),
                    ("slot".to_string(), format!("{}", dev.slot)),
                    ("function".to_string(), format!("{}", dev.function)),
                    ("serial".to_string(), dev.serial.clone()),
                    ("cols".to_string(), format!("{}", dev.cols)),
                    ("rows".to_string(), format!("{}", dev.rows)),
                ],
            ));
        } else if child_obj.downcast_bound::<MemorySpace>(py).is_ok() {
            out.push((path, "MemorySpace".to_string(), Vec::new()));
        } else if child_obj.downcast_bound::<Cache>(py).is_ok() {
            out.push((path, "Cache".to_string(), Vec::new()));
        } else if let Ok(child_sim) = child_obj.downcast_bound::<SimObject>(py) {
            // Unknown SimObject subclass: still emit a stub so the
            // section appears in config.ini with the generic type
            // name. The subclass's own parameters can be added
            // later by extending the walker.
            let _ = child_sim;
            out.push((path, "SimObject".to_string(), Vec::new()));
        }
    }

    out
}
