//! Compatibility re-export for the canonical system memory composition type.

pub use helm_memory::HelmAddressSpace;

use helm_hw_pci::PciBus;
use helm_hw_virtio::pci::VirtioPciBar0Device;

/// Result of draining one mapped PCI bus's pending BAR remap commands.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PciRemapDrainResult {
    pub drained: usize,
    pub applied: usize,
}

/// Drain pending BAR remaps from a mapped [`PciBus`] and project them onto the
/// live [`HelmAddressSpace`] surface.
///
/// This is the current runtime-side caller path for BAR reconfiguration:
/// write ECAM config space first, then drain queued remaps after the device
/// write completes.
pub(crate) fn drain_pci_bus_remaps(
    sys_mem: &mut HelmAddressSpace,
    pci_bus_idx: usize,
) -> PciRemapDrainResult {
    let Some(remaps) = sys_mem.with_device_mut::<PciBus, _>(pci_bus_idx, |bus| bus.drain_remaps())
    else {
        return PciRemapDrainResult::default();
    };

    let drained = remaps.len();
    let mut applied = 0usize;
    for cmd in remaps {
        if sys_mem.apply_pci_bar_remap(
            cmd.bdf.bus,
            cmd.bdf.device,
            cmd.bdf.function,
            cmd.bar_idx,
            cmd.old_base,
            cmd.new_base,
        ) {
            applied += 1;
        }
    }

    PciRemapDrainResult { drained, applied }
}

/// Drain BAR remaps from every mapped [`PciBus`] device in the live address
/// space and project them onto [`HelmAddressSpace`].
pub(crate) fn drain_all_pci_bus_remaps(sys_mem: &mut HelmAddressSpace) -> PciRemapDrainResult {
    let mut total = PciRemapDrainResult::default();
    let len = sys_mem.devices.len();
    for idx in 0..len {
        let result = drain_pci_bus_remaps(sys_mem, idx);
        total.drained += result.drained;
        total.applied += result.applied;
    }
    total
}

/// Process pending standard `virtio-pci` queue work against the live system
/// memory surface.
pub(crate) fn process_all_virtio_pci_pending(sys_mem: &mut HelmAddressSpace) -> Vec<(u64, u32)> {
    let len = sys_mem.devices.len();
    let mut messages = Vec::new();
    for idx in 0..len {
        let processor = {
            let Some(dev) = sys_mem.device_as_mut::<VirtioPciBar0Device>(idx) else {
                continue;
            };
            dev.pending_processor()
        };
        let result = processor.process_pending(sys_mem);
        messages.extend(result.msix_messages);
    }
    messages
}
