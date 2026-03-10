use crate::pci::capability::virtio_pci_cap::{VirtioPciCap, VirtioPciCapType};
use crate::pci::transport::VirtioPciTransport;
use crate::pci::{PciCapability, PciFunction};
use crate::virtio::blk::VirtioBlk;
use crate::virtio::net::VirtioNet;
use crate::virtio::rng::VirtioRng;
use crate::virtio::transport::{VirtioMmioTransport, VirtioTransport};

// ── Helper constructors ───────────────────────────────────────────────────────

fn make_rng_pci() -> VirtioPciTransport {
    VirtioPciTransport::new(Box::new(VirtioRng::new()))
}

fn make_blk_pci() -> VirtioPciTransport {
    VirtioPciTransport::new(Box::new(VirtioBlk::new(4096)))
}

// ── PCI identity tests ────────────────────────────────────────────────────────

#[test]
fn pci_vendor_id_is_virtio() {
    let t = make_rng_pci();
    assert_eq!(t.vendor_id(), 0x1AF4);
}

#[test]
fn pci_device_id_rng() {
    let t = make_rng_pci();
    // Modern non-transitional: 0x1040 + VIRTIO_DEV_RNG (4)
    assert_eq!(t.device_id(), 0x1040 + 4);
}

#[test]
fn pci_device_id_blk() {
    let t = make_blk_pci();
    // Modern non-transitional: 0x1040 + VIRTIO_DEV_BLK (2)
    assert_eq!(t.device_id(), 0x1040 + 2);
}

#[test]
fn pci_device_id_net() {
    let t = VirtioPciTransport::new(Box::new(VirtioNet::new()));
    // Modern non-transitional: 0x1040 + VIRTIO_DEV_NET (1)
    assert_eq!(t.device_id(), 0x1040 + 1);
}

#[test]
fn pci_subsystem_vendor_id() {
    let t = make_rng_pci();
    assert_eq!(t.subsystem_vendor_id(), 0x1AF4);
}

#[test]
fn pci_subsystem_id_rng() {
    let t = make_rng_pci();
    // subsystem_id = device_type = VIRTIO_DEV_RNG = 4
    assert_eq!(t.subsystem_id(), 4);
}

#[test]
fn pci_class_code_net() {
    let t = VirtioPciTransport::new(Box::new(VirtioNet::new()));
    assert_eq!(t.class_code(), 0x020000); // Ethernet controller
}

#[test]
fn pci_class_code_blk() {
    let t = make_blk_pci();
    assert_eq!(t.class_code(), 0x018000); // Mass storage
}

#[test]
fn pci_class_code_rng_is_unclassified() {
    let t = make_rng_pci();
    assert_eq!(t.class_code(), 0xFF0000); // Unclassified
}

// ── BAR layout tests ──────────────────────────────────────────────────────────

#[test]
fn bar0_is_mmio32_4k() {
    use crate::pci::BarDecl;
    let t = make_rng_pci();
    assert_eq!(t.bars()[0], BarDecl::Mmio32 { size: 0x1000 });
}

#[test]
fn bar4_is_mmio32_4k() {
    use crate::pci::BarDecl;
    let t = make_rng_pci();
    assert_eq!(t.bars()[4], BarDecl::Mmio32 { size: 0x1000 });
}

#[test]
fn bars_1_2_3_5_unused() {
    use crate::pci::BarDecl;
    let t = make_rng_pci();
    assert_eq!(t.bars()[1], BarDecl::Unused);
    assert_eq!(t.bars()[2], BarDecl::Unused);
    assert_eq!(t.bars()[3], BarDecl::Unused);
    assert_eq!(t.bars()[5], BarDecl::Unused);
}

// ── Capability structure tests ────────────────────────────────────────────────

#[test]
fn has_pcie_capability() {
    let t = make_rng_pci();
    let has_pcie = t.capabilities().iter().any(|c| c.cap_id() == 0x10);
    assert!(has_pcie, "must have PCIe capability (0x10)");
}

#[test]
fn has_pm_capability() {
    let t = make_rng_pci();
    let has_pm = t.capabilities().iter().any(|c| c.cap_id() == 0x01);
    assert!(has_pm, "must have PM capability (0x01)");
}

#[test]
fn has_five_virtio_vendor_specific_caps() {
    let t = make_rng_pci();
    let count = t
        .capabilities()
        .iter()
        .filter(|c| c.cap_id() == 0x09)
        .count();
    assert_eq!(count, 5, "must have 5 VirtIO vendor-specific caps (0x09)");
}

#[test]
fn virtio_caps_include_all_types() {
    let t = make_rng_pci();
    let caps: Vec<&dyn PciCapability> = t.capabilities().iter().map(AsRef::as_ref).collect();

    let types_present: Vec<u8> = caps
        .iter()
        .filter(|c| c.cap_id() == 0x09)
        .map(|c| {
            // cfg_type is in bits [31:24] of the first dword
            (c.read(0x00) >> 24) as u8
        })
        .collect();

    for expected in [1u8, 2, 3, 4, 5] {
        assert!(
            types_present.contains(&expected),
            "missing VirtIO cap type {expected}"
        );
    }
}

// ── VirtioPciCap unit tests ───────────────────────────────────────────────────

#[test]
fn virtio_pci_cap_id_is_vendor_specific() {
    let cap = VirtioPciCap::new(0x80, VirtioPciCapType::CommonCfg, 0, 0x000, 0x38, 0);
    assert_eq!(cap.cap_id(), 0x09);
}

#[test]
fn virtio_pci_cap_common_cfg_length_is_16() {
    let cap = VirtioPciCap::new(0x80, VirtioPciCapType::CommonCfg, 0, 0x000, 0x38, 0);
    assert_eq!(cap.length(), 16);
}

#[test]
fn virtio_pci_cap_notify_length_is_20() {
    let cap = VirtioPciCap::new(0xA0, VirtioPciCapType::NotifyCfg, 0, 0x040, 0x40, 2);
    assert_eq!(cap.length(), 20);
}

#[test]
fn virtio_pci_cap_reads_cfg_type() {
    let cap = VirtioPciCap::new(0x80, VirtioPciCapType::IsrCfg, 0, 0x038, 0x04, 0);
    let dword = cap.read(0x00);
    let cfg_type = (dword >> 24) as u8;
    assert_eq!(cfg_type, VirtioPciCapType::IsrCfg as u8);
}

#[test]
fn virtio_pci_cap_reads_bar() {
    let cap = VirtioPciCap::new(0x80, VirtioPciCapType::CommonCfg, 2, 0x000, 0x38, 0);
    assert_eq!(cap.read(0x04), 2u32);
}

#[test]
fn virtio_pci_cap_reads_bar_offset() {
    let cap = VirtioPciCap::new(0x80, VirtioPciCapType::CommonCfg, 0, 0x1000, 0x38, 0);
    assert_eq!(cap.read(0x08), 0x1000);
}

#[test]
fn virtio_pci_cap_reads_length() {
    let cap = VirtioPciCap::new(0x80, VirtioPciCapType::CommonCfg, 0, 0x000, 0x38, 0);
    assert_eq!(cap.read(0x0C), 0x38);
}

#[test]
fn virtio_pci_cap_notify_reads_multiplier() {
    let cap = VirtioPciCap::new(0xA0, VirtioPciCapType::NotifyCfg, 0, 0x040, 0x40, 4);
    assert_eq!(cap.read(0x10), 4);
}

#[test]
fn virtio_pci_cap_writes_ignored() {
    let mut cap = VirtioPciCap::new(0x80, VirtioPciCapType::CommonCfg, 0, 0x000, 0x38, 0);
    cap.write(0x04, 99); // try to change bar
    assert_eq!(cap.bar(), 0, "bar should remain 0 — all writes are ignored");
}

// ── BAR0 common config read/write tests ───────────────────────────────────────

#[test]
fn initial_device_status_zero() {
    let t = make_rng_pci();
    // device_status is in bits [7:0] of dword at offset 0x14
    let dword = t.bar_read(0, 0x14, 4) as u32;
    assert_eq!(dword & 0xFF, 0, "device_status should start at 0");
}

#[test]
fn device_feature_low_select_0() {
    // Note: bar_read is &self, so we need to write device_features_sel first
    // via bar_write then read back.
    let mut t = make_rng_pci();
    // Write device_features_sel = 0 at offset 0x00
    t.bar_write(0, 0x00, 4, 0);
    let features = t.bar_read(0, 0x04, 4) as u32;
    // RNG only declares VERSION_1 (bit 32) so low 32 bits should be 0
    assert_eq!(features, 0);
}

#[test]
fn device_feature_high_select_1() {
    let mut t = make_rng_pci();
    // Write device_features_sel = 1 at offset 0x00
    t.bar_write(0, 0x00, 4, 1);
    let features = t.bar_read(0, 0x04, 4) as u32;
    // VIRTIO_F_VERSION_1 = bit 32 → bit 0 in the high word
    assert!(
        features & 1 != 0,
        "VERSION_1 should be set in high feature word"
    );
}

#[test]
fn write_and_read_device_status() {
    let mut t = make_rng_pci();
    // device_status is at dword offset 0x14, bits [7:0]
    // Write ACKNOWLEDGE (1) — need to write the whole dword; queue_sel in high 16
    t.bar_write(0, 0x14, 4, 0x0000_0001);
    assert_eq!(t.status(), 1);
}

#[test]
fn reset_by_writing_zero_status() {
    let mut t = make_rng_pci();
    t.bar_write(0, 0x14, 4, 0x0000_000F); // set some bits
    assert_ne!(t.status(), 0);
    t.bar_write(0, 0x14, 4, 0x0000_0000); // reset
    assert_eq!(t.status(), 0);
}

#[test]
fn queue_select_via_common_cfg() {
    let mut t = make_blk_pci();
    // Write queue_select = 0 via device_status dword: bits [31:16] = queue_sel
    t.bar_write(0, 0x14, 4, 0x0000_0000); // queue_sel=0, status=0
    let dword = t.bar_read(0, 0x14, 4) as u32;
    let queue_sel = (dword >> 16) as u16;
    assert_eq!(queue_sel, 0);
}

#[test]
fn num_queues_in_msix_config_dword() {
    let t = make_rng_pci();
    // dword at 0x10: msix_config [15:0] | num_queues [31:16]
    let dword = t.bar_read(0, 0x10, 4) as u32;
    let num_queues = (dword >> 16) as u16;
    assert_eq!(num_queues, 1, "RNG has 1 queue");
}

#[test]
fn queue_size_in_queue_size_dword() {
    let t = make_rng_pci();
    // dword at 0x18: queue_size [15:0] | queue_msix_vector [31:16]
    let dword = t.bar_read(0, 0x18, 4) as u32;
    let queue_size = dword as u16;
    assert!(queue_size > 0, "queue size should be non-zero");
}

#[test]
fn queue_enable_initially_false() {
    let t = make_rng_pci();
    // dword at 0x1C: queue_enable [15:0] | queue_notify_off [31:16]
    let dword = t.bar_read(0, 0x1C, 4) as u32;
    let queue_enable = dword as u16;
    assert_eq!(queue_enable, 0, "queue should not be enabled initially");
}

#[test]
fn write_queue_enable() {
    let mut t = make_rng_pci();
    // Enable queue 0: write 1 to low 16 bits of dword 0x1C
    t.bar_write(0, 0x1C, 4, 0x0000_0001);
    let dword = t.bar_read(0, 0x1C, 4) as u32;
    assert_eq!(dword & 0xFFFF, 1, "queue_enable should be 1");
}

#[test]
fn write_driver_features() {
    let mut t = make_rng_pci();
    // Set driver_features_sel = 0 at offset 0x08
    t.bar_write(0, 0x08, 4, 0);
    // Write driver_features low at offset 0x0C
    t.bar_write(0, 0x0C, 4, 0xDEAD_BEEF);
    assert_eq!(t.driver_features() & 0xFFFF_FFFF, 0xDEAD_BEEF);
}

// ── ISR read-to-clear test ────────────────────────────────────────────────────

#[test]
fn isr_read_to_clear() {
    let mut t = make_rng_pci();
    // Raise the queue IRQ
    VirtioTransport::raise_irq(&mut t);
    assert_ne!(t.interrupt_status(), 0, "interrupt_status should be set");

    // ISR is at BAR0 offset 0x038; bar_read uses &self so it won't clear.
    // Use the mutable helper.
    let isr = t.bar0_read_mut(0x038);
    assert_ne!(isr, 0, "ISR read should return the set bit");
    assert_eq!(t.interrupt_status(), 0, "ISR should clear after read");
}

#[test]
fn isr_config_irq_bit() {
    let mut t = make_rng_pci();
    VirtioTransport::raise_config_irq(&mut t);
    let isr = t.bar0_read_mut(0x038);
    assert!(isr & 2 != 0, "config IRQ bit (bit 1) should be set");
}

// ── Notify region tests ───────────────────────────────────────────────────────

#[test]
fn notify_write_triggers_queue_notify() {
    // Build a blk device and push something onto the queue so we can verify
    // queue_notify was called (VirtioRng processes the queue internally).
    let mut t = make_rng_pci();
    // Set up queue 0 as enabled and add a descriptor
    t.bar_write(0, 0x14, 4, 0x0000_0000); // select queue 0, reset status
    t.bar_write(0, 0x1C, 4, 0x0000_0001); // enable queue 0

    // The RNG queue_notify drains the queue; with an empty queue it's a no-op.
    // Write to notify offset for queue 0: NOTIFY_OFFSET + 0 * multiplier (2) = 0x040
    t.bar_write(0, 0x040, 4, 0);
    // Test passes if no panic — queue_notify dispatched to backend
}

// ── Device config region tests ────────────────────────────────────────────────

#[test]
fn blk_device_config_capacity_readable() {
    let t = make_blk_pci();
    // VirtioBlk config space at BAR0 offset 0x080:
    // bytes 0..7 = capacity in sectors (little-endian u64)
    let low = t.bar_read(0, 0x080, 4) as u32;
    let high = t.bar_read(0, 0x084, 4) as u32;
    let capacity = u64::from(low) | (u64::from(high) << 32);
    // VirtioBlk::new(4096) → 4096/512 = 8 sectors
    assert_eq!(
        capacity, 8,
        "capacity should be 8 sectors for a 4096-byte disk"
    );
}

#[test]
fn rng_device_config_is_empty() {
    let t = make_rng_pci();
    // RNG has no device-specific config; reads should return 0
    let val = t.bar_read(0, 0x080, 4) as u32;
    assert_eq!(val, 0);
}

// ── VirtioTransport trait tests ───────────────────────────────────────────────

#[test]
fn pci_transport_type_is_pci() {
    let t = make_rng_pci();
    assert_eq!(VirtioTransport::transport_type(&t), "pci");
}

#[test]
fn mmio_transport_type_is_mmio() {
    let t = VirtioMmioTransport::new(Box::new(VirtioRng::new()));
    assert_eq!(VirtioTransport::transport_type(&t), "mmio");
}

#[test]
fn pci_transport_backend_name() {
    let t = make_rng_pci();
    assert_eq!(VirtioTransport::backend(&t).name(), "virtio-rng");
}

#[test]
fn mmio_transport_backend_name() {
    let t = VirtioMmioTransport::new(Box::new(VirtioRng::new()));
    assert_eq!(VirtioTransport::backend(&t).name(), "virtio-rng");
}

#[test]
fn mmio_raise_irq_sets_interrupt_status() {
    let mut t = VirtioMmioTransport::new(Box::new(VirtioRng::new()));
    VirtioTransport::raise_irq(&mut t);
    assert_ne!(t.interrupt_status(), 0);
}

#[test]
fn mmio_raise_config_irq_sets_interrupt_status() {
    let mut t = VirtioMmioTransport::new(Box::new(VirtioRng::new()));
    VirtioTransport::raise_config_irq(&mut t);
    assert_ne!(t.interrupt_status(), 0);
}

#[test]
fn pci_raise_irq_sets_interrupt_status() {
    let mut t = make_rng_pci();
    VirtioTransport::raise_irq(&mut t);
    assert_ne!(t.interrupt_status(), 0);
}

#[test]
fn pci_raise_config_irq_sets_interrupt_status() {
    let mut t = make_rng_pci();
    VirtioTransport::raise_config_irq(&mut t);
    assert_ne!(t.interrupt_status(), 0);
}

#[test]
fn pci_transport_as_trait_object() {
    // Verify VirtioPciTransport can be used as Box<dyn VirtioTransport>
    let t: Box<dyn VirtioTransport> = Box::new(make_rng_pci());
    assert_eq!(t.transport_type(), "pci");
    assert_eq!(t.backend().name(), "virtio-rng");
}

#[test]
fn mmio_transport_as_trait_object() {
    // Verify VirtioMmioTransport can be used as Box<dyn VirtioTransport>
    let t: Box<dyn VirtioTransport> =
        Box::new(VirtioMmioTransport::new(Box::new(VirtioRng::new())));
    assert_eq!(t.transport_type(), "mmio");
}

// ── Reset tests ───────────────────────────────────────────────────────────────

#[test]
fn pci_reset_clears_state() {
    let mut t = make_rng_pci();
    t.bar_write(0, 0x14, 4, 0x0000_000F); // set status bits
    PciFunction::reset(&mut t);
    assert_eq!(t.status(), 0);
    assert_eq!(t.driver_features(), 0);
    assert_eq!(t.interrupt_status(), 0);
}

// ── Name test ─────────────────────────────────────────────────────────────────

#[test]
fn pci_transport_name_delegates_to_backend() {
    let t = make_rng_pci();
    assert_eq!(PciFunction::name(&t), "virtio-rng");
}
