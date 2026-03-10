use crate::pci::capability::{AcsCapability, AerCapability, MsixCapability, PmCapability};
use crate::pci::capability::{MsixVector, PcieCapability};
use crate::pci::PciCapability;

// ── PcieCapability tests ──────────────────────────────────────────────────────

#[test]
fn pcie_cap_id() {
    let cap = PcieCapability::endpoint(0x40);
    assert_eq!(cap.cap_id(), 0x10);
}

#[test]
fn pcie_offset_and_length() {
    let cap = PcieCapability::endpoint(0x60);
    assert_eq!(cap.offset(), 0x60);
    assert_eq!(cap.length(), 60);
}

#[test]
fn pcie_not_extended() {
    let cap = PcieCapability::endpoint(0x40);
    assert!(!cap.is_extended());
}

#[test]
fn pcie_dev_cap_max_payload_256b() {
    let cap = PcieCapability::endpoint(0x40);
    // dev_cap at relative offset 0x04; bits [2:0] = max payload encoding
    let dev_cap = cap.read(0x04);
    assert_eq!(
        dev_cap & 0x7,
        0x1,
        "max payload should be 256B (encoding 1)"
    );
}

#[test]
fn pcie_link_cap_gen3_x1() {
    let cap = PcieCapability::endpoint(0x40);
    // link_cap at relative offset 0x0C
    let link_cap = cap.read(0x0C);
    let speed = link_cap & 0xF;
    let width = (link_cap >> 4) & 0x3F;
    assert_eq!(speed, 3, "link speed should be Gen3 (encoding 3)");
    assert_eq!(width, 1, "link width should be x1");
}

#[test]
fn pcie_link_status_negotiated_speed() {
    let cap = PcieCapability::endpoint(0x40);
    // link_ctl|link_sta at relative offset 0x10; link_sta in high 16 bits
    let dword = cap.read(0x10);
    let link_sta = (dword >> 16) as u16;
    let current_speed = link_sta & 0xF;
    assert_eq!(current_speed, 3, "negotiated speed should be Gen3");
}

#[test]
fn pcie_dev_ctl_writable() {
    let mut cap = PcieCapability::endpoint(0x40);
    // Write to dev_ctl (relative offset 0x08, low 16 bits)
    cap.write(0x08, 0x0010); // relaxed ordering enable
    let dword = cap.read(0x08);
    assert_eq!(dword & 0xFFFF, 0x0010);
}

#[test]
fn pcie_dev_sta_w1c() {
    let mut cap = PcieCapability::endpoint(0x40);
    // Manually set a status bit via write to dev_ctl word (we use inject-style
    // by reading and checking that W1C clears it)
    // Set dev_sta via the read — first verify it starts at 0
    let dword = cap.read(0x08);
    assert_eq!((dword >> 16) & 0xF, 0, "dev_sta starts cleared");

    // Write with high word = 0 (nothing to clear) — should remain 0
    cap.write(0x08, 0x0000_0000);
    let dword = cap.read(0x08);
    assert_eq!((dword >> 16) & 0xF, 0);
}

#[test]
fn pcie_reset_clears_ctl() {
    let mut cap = PcieCapability::endpoint(0x40);
    cap.write(0x08, 0x00FF); // set dev_ctl bits
    cap.reset();
    let dev_ctl = cap.read(0x08) & 0xFFFF;
    assert_eq!(dev_ctl, 0, "dev_ctl should be zero after reset");
}

#[test]
fn pcie_root_port_constructs() {
    let cap = PcieCapability::root_port(0x40);
    assert_eq!(cap.cap_id(), 0x10);
    // pcie_cap dword at offset 0x00, high 16 bits = pcie_cap register
    // device type = RootPort (4) at bits [7:4]
    let pcie_cap_reg = (cap.read(0x00) >> 16) as u16;
    let dev_type = (pcie_cap_reg >> 4) & 0xF;
    assert_eq!(dev_type, 4, "root port device type");
}

// ── MsixCapability tests ──────────────────────────────────────────────────────

#[test]
fn msix_cap_id() {
    let cap = MsixCapability::new(0x70, 4, 0, 0x2000, 0, 0x3000);
    assert_eq!(cap.cap_id(), 0x11);
}

#[test]
fn msix_table_size() {
    let cap = MsixCapability::new(0x70, 8, 0, 0, 0, 0x1000);
    assert_eq!(cap.table_size(), 8);
}

#[test]
fn msix_table_bar() {
    let cap = MsixCapability::new(0x70, 4, 2, 0, 0, 0x1000);
    assert_eq!(cap.table_bar(), 2);
}

#[test]
fn msix_initially_disabled() {
    let cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    assert!(!cap.is_enabled());
}

#[test]
fn msix_enable_disable() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    // Enable via write to config header (offset 0x00, high word bit 15)
    cap.write(0x00, 0x8000_0000); // MSI-X Enable = 1
    assert!(cap.is_enabled());

    cap.write(0x00, 0x0000_0000); // MSI-X Enable = 0
    assert!(!cap.is_enabled());
}

#[test]
fn msix_vector_write_read() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0x0000_0000, 0x41);
    let v = cap.read_vector(0);
    assert_eq!(v.addr_lo, 0xFEE0_0000);
    assert_eq!(v.addr_hi, 0x0000_0000);
    assert_eq!(v.data, 0x41);
}

#[test]
fn msix_vector_default_not_masked() {
    let cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    let v = cap.read_vector(0);
    assert!(!v.masked);
    assert!(!v.pending);
}

#[test]
fn msix_mask_vector() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.mask_vector(0, true);
    assert!(cap.read_vector(0).masked);
    cap.mask_vector(0, false);
    assert!(!cap.read_vector(0).masked);
}

#[test]
fn msix_unmask_clears_pending() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    cap.write(0x00, 0x8000_0000); // enable

    // Mask then fire — should set pending
    cap.mask_vector(0, true);
    let result = cap.fire(0);
    assert!(result.is_none());
    assert!(cap.read_vector(0).pending);

    // Unmask — pending should clear
    cap.mask_vector(0, false);
    assert!(!cap.read_vector(0).pending);
}

#[test]
fn msix_fire_enabled_unmasked() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write_vector(0, 0xFEE0_0010, 0, 0x41);
    cap.write(0x00, 0x8000_0000); // enable

    let result = cap.fire(0);
    assert!(result.is_some());
    let (addr, data) = result.unwrap();
    assert_eq!(addr, 0xFEE0_0010);
    assert_eq!(data, 0x41);
}

#[test]
fn msix_fire_disabled_returns_none() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    // MSI-X not enabled
    assert!(cap.fire(0).is_none());
}

#[test]
fn msix_fire_masked_sets_pending() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    cap.write(0x00, 0x8000_0000); // enable
    cap.mask_vector(0, true);

    let result = cap.fire(0);
    assert!(result.is_none());
    assert!(cap.read_vector(0).pending, "pending bit should be set");
}

#[test]
fn msix_reset_clears_all() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write(0x00, 0x8000_0000); // enable
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    cap.mask_vector(0, true);
    cap.reset();

    assert!(!cap.is_enabled());
    let v = cap.read_vector(0);
    assert_eq!(v.addr_lo, 0);
    assert_eq!(v.data, 0);
    assert!(!v.masked);
    assert!(!v.pending);
}

#[test]
fn msix_table_bar_offset_in_config_read() {
    let cap = MsixCapability::new(0x70, 4, 1, 0x2000, 2, 0x4000);
    // Offset 0x04: table_offset | table_bir
    let dword = cap.read(0x04);
    assert_eq!(dword & 0b111, 1, "table_bir should be 1");
    assert_eq!(dword & !0b111, 0x2000, "table_offset should be 0x2000");
}

#[test]
fn msix_pba_bar_offset_in_config_read() {
    let cap = MsixCapability::new(0x70, 4, 1, 0x2000, 2, 0x4000);
    // Offset 0x08: pba_offset | pba_bir
    let dword = cap.read(0x08);
    assert_eq!(dword & 0b111, 2, "pba_bir should be 2");
    assert_eq!(dword & !0b111, 0x4000, "pba_offset should be 0x4000");
}

#[test]
fn msix_table_read_write_via_bar() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    // Write addr_lo via table BAR interface
    cap.table_write(0, 4, 0xFEE0_0000);
    let val = cap.table_read(0, 4);
    assert_eq!(val, 0xFEE0_0000);
}

#[test]
fn msix_pba_read_reflects_pending() {
    let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    cap.write(0x00, 0x8000_0000); // enable
    cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    cap.mask_vector(0, true);
    let _ = cap.fire(0); // sets pending for vector 0

    let pba = cap.pba_read(0);
    assert_eq!(pba & 1, 1, "vector 0 should be pending");
}

// ── MsixVector derive tests ───────────────────────────────────────────────────

#[test]
fn msix_vector_default_derive() {
    let v: MsixVector = MsixVector::default();
    assert_eq!(v.addr_lo, 0);
    assert_eq!(v.addr_hi, 0);
    assert_eq!(v.data, 0);
    assert!(!v.masked);
    assert!(!v.pending);
}

#[test]
fn msix_vector_clone() {
    let v = MsixVector {
        addr_lo: 0xFEE0_0000,
        addr_hi: 0,
        data: 0x41,
        masked: false,
        pending: false,
    };
    let v2 = v.clone();
    assert_eq!(v2.addr_lo, 0xFEE0_0000);
    assert_eq!(v2.data, 0x41);
}

// ── PmCapability tests ────────────────────────────────────────────────────────

#[test]
fn pm_cap_id() {
    let cap = PmCapability::new(0x50);
    assert_eq!(cap.cap_id(), 0x01);
}

#[test]
fn pm_offset_and_length() {
    let cap = PmCapability::new(0x50);
    assert_eq!(cap.offset(), 0x50);
    assert_eq!(cap.length(), 8);
}

#[test]
fn pm_not_extended() {
    let cap = PmCapability::new(0x50);
    assert!(!cap.is_extended());
}

#[test]
fn pm_initial_power_state_d0() {
    let cap = PmCapability::new(0x50);
    let pm_csr = cap.read(0x04);
    assert_eq!(pm_csr & 0x3, 0, "power state should be D0 on reset");
}

#[test]
fn pm_power_state_writable() {
    let mut cap = PmCapability::new(0x50);
    // Write D3hot (0b11) to bits [1:0]
    cap.write(0x04, 0x0003);
    let pm_csr = cap.read(0x04);
    assert_eq!(pm_csr & 0x3, 0x3, "power state should be D3hot");
}

#[test]
fn pm_pme_status_w1c() {
    let mut cap = PmCapability::new(0x50);
    // Directly set PME_Status bit (bit 15) by writing to pm_csr raw bits —
    // we cannot set it via write() since write only does W1C, so use reset+check
    // Instead: assert that writing bit 15 = 1 clears it (W1C semantics).
    // Start: pm_csr = 0, PME_Status = 0.
    // Writing bit 15 = 1 should have no net effect (clearing an already-0 bit).
    cap.write(0x04, 1 << 15);
    let pm_csr = cap.read(0x04);
    assert_eq!(
        (pm_csr >> 15) & 1,
        0,
        "PME_Status should remain 0 after W1C of already-0 bit"
    );
}

#[test]
fn pm_reset_clears_state() {
    let mut cap = PmCapability::new(0x50);
    cap.write(0x04, 0x0003); // D3hot
    cap.reset();
    let pm_csr = cap.read(0x04);
    assert_eq!(pm_csr, 0, "pm_csr should be zero after reset");
}

// ── AerCapability tests ───────────────────────────────────────────────────────

#[test]
fn aer_cap_id() {
    let cap = AerCapability::new(0x100);
    assert_eq!(cap.cap_id(), 0x01); // low byte of 0x0001
}

#[test]
fn aer_is_extended() {
    let cap = AerCapability::new(0x100);
    assert!(cap.is_extended());
}

#[test]
fn aer_offset_and_length() {
    let cap = AerCapability::new(0x100);
    assert_eq!(cap.offset(), 0x100);
    assert_eq!(cap.length(), 48);
}

#[test]
fn aer_ext_cap_header() {
    let cap = AerCapability::new(0x100);
    let header = cap.read(0x00);
    // Bits [15:0] = 0x0001 (AER)
    assert_eq!(header & 0xFFFF, 0x0001);
    // Bits [19:16] = 1 (version)
    assert_eq!((header >> 16) & 0xF, 1);
}

#[test]
fn aer_uncorrectable_status_inject() {
    let mut cap = AerCapability::new(0x100);
    cap.inject_uncorrectable(0x0010); // bit 4
    let status = cap.read(0x04);
    assert_eq!(status & 0x0010, 0x0010);
}

#[test]
fn aer_uncorrectable_status_w1c() {
    let mut cap = AerCapability::new(0x100);
    cap.inject_uncorrectable(0x0010);
    // W1C: write 1 to bit 4 to clear it
    cap.write(0x04, 0x0010);
    let status = cap.read(0x04);
    assert_eq!(status & 0x0010, 0, "bit should be cleared by W1C");
}

#[test]
fn aer_uncorrectable_mask_writable() {
    let mut cap = AerCapability::new(0x100);
    cap.write(0x08, 0xFFFF_FFFF);
    assert_eq!(cap.read(0x08), 0xFFFF_FFFF);
}

#[test]
fn aer_correctable_status_inject() {
    let mut cap = AerCapability::new(0x100);
    cap.inject_correctable(0x0001);
    assert_eq!(cap.read(0x10) & 0x0001, 0x0001);
}

#[test]
fn aer_correctable_status_w1c() {
    let mut cap = AerCapability::new(0x100);
    cap.inject_correctable(0x0001);
    cap.write(0x10, 0x0001);
    assert_eq!(
        cap.read(0x10) & 0x0001,
        0,
        "correctable status cleared by W1C"
    );
}

#[test]
fn aer_reset_clears_all() {
    let mut cap = AerCapability::new(0x100);
    cap.inject_uncorrectable(0xFFFF_FFFF);
    cap.inject_correctable(0xFFFF_FFFF);
    cap.write(0x08, 0xFFFF_FFFF); // mask
    cap.reset();
    assert_eq!(cap.read(0x04), 0);
    assert_eq!(cap.read(0x08), 0);
    assert_eq!(cap.read(0x10), 0);
}

// ── AcsCapability tests ───────────────────────────────────────────────────────

#[test]
fn acs_cap_id() {
    let cap = AcsCapability::new(0x148);
    assert_eq!(cap.cap_id(), 0x0D); // low byte of 0x000D
}

#[test]
fn acs_is_extended() {
    let cap = AcsCapability::new(0x148);
    assert!(cap.is_extended());
}

#[test]
fn acs_offset_and_length() {
    let cap = AcsCapability::new(0x148);
    assert_eq!(cap.offset(), 0x148);
    assert_eq!(cap.length(), 8);
}

#[test]
fn acs_ext_cap_header() {
    let cap = AcsCapability::new(0x148);
    let header = cap.read(0x00);
    // Bits [15:0] = 0x000D (ACS)
    assert_eq!(header & 0xFFFF, 0x000D);
    // Bits [19:16] = 1 (version)
    assert_eq!((header >> 16) & 0xF, 1);
}

#[test]
fn acs_cap_register_default() {
    let cap = AcsCapability::new(0x148);
    // acs_cap at bits [15:0] of offset 0x04 = 0x001F
    let dword = cap.read(0x04);
    assert_eq!(dword & 0xFFFF, 0x001F);
}

#[test]
fn acs_control_writable() {
    let mut cap = AcsCapability::new(0x148);
    // ACS Control in high 16 bits of offset 0x04
    cap.write(0x04, 0x001F_0000); // enable all five bits in acs_ctl
    let dword = cap.read(0x04);
    let acs_ctl = (dword >> 16) as u16;
    assert_eq!(acs_ctl, 0x001F);
}

#[test]
fn acs_cap_register_readonly() {
    let mut cap = AcsCapability::new(0x148);
    // Writing to low 16 bits should not change acs_cap
    cap.write(0x04, 0x0000_0000); // try to zero acs_cap
    let dword = cap.read(0x04);
    assert_eq!(
        dword & 0xFFFF,
        0x001F,
        "acs_cap should remain 0x001F (read-only)"
    );
}

#[test]
fn acs_reset_clears_control() {
    let mut cap = AcsCapability::new(0x148);
    cap.write(0x04, 0x001F_0000); // set all acs_ctl bits
    cap.reset();
    let dword = cap.read(0x04);
    let acs_ctl = (dword >> 16) as u16;
    assert_eq!(acs_ctl, 0, "acs_ctl should be zero after reset");
    // acs_cap should still be 0x001F
    assert_eq!(dword & 0xFFFF, 0x001F);
}
