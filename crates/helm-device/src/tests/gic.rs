use crate::arm::gic::common::*;
use crate::arm::gic::distributor::GicDistributor;
use crate::arm::gic::icc::{IccReg, IccState};
use crate::arm::gic::its::GicIts;
use crate::arm::gic::lpi::{LpiConfig, LpiConfigTable, LpiPendingTable};
use crate::arm::gic::redistributor::GicRedistributor;
use crate::arm::gic::v3::GicV3;
use crate::arm::gic::v4::{GicV4, GicV4Version};
use crate::arm::gic::GicVersion;
use crate::device::Device;
use crate::irq::InterruptController;
use crate::transaction::Transaction;

fn read_reg(dev: &mut dyn Device, offset: u64) -> u32 {
    let mut txn = Transaction::read(0, 4);
    txn.offset = offset;
    dev.transact(&mut txn).unwrap();
    txn.data_u32()
}

fn write_reg(dev: &mut dyn Device, offset: u64, value: u32) {
    let mut txn = Transaction::write(0, 4, value as u64);
    txn.offset = offset;
    dev.transact(&mut txn).unwrap();
}

// ── Common helpers ──────────────────────────────────────────────────────────

#[test]
fn bitmap_set_and_check() {
    let mut bmap = vec![0u32; 4];
    bitmap_set(&mut bmap, 33);
    assert!(bitmap_is_set(&bmap, 33));
    assert!(!bitmap_is_set(&bmap, 32));
}

#[test]
fn bitmap_clear_works() {
    let mut bmap = vec![0u32; 4];
    bitmap_set(&mut bmap, 10);
    bitmap_clear(&mut bmap, 10);
    assert!(!bitmap_is_set(&bmap, 10));
}

#[test]
fn bitmap_or_and_andnot() {
    let mut bmap = vec![0u32; 2];
    bitmap_or_word(&mut bmap, 0, 0xFF);
    assert_eq!(bitmap_read_word(&bmap, 0), 0xFF);
    bitmap_andnot_word(&mut bmap, 0, 0x0F);
    assert_eq!(bitmap_read_word(&bmap, 0), 0xF0);
}

#[test]
fn byte_array_roundtrip() {
    let mut arr = vec![0u8; 8];
    byte_array_write4(&mut arr, 2, 0xAABBCCDD);
    assert_eq!(byte_array_read4(&arr, 2), 0xAABBCCDD);
}

#[test]
fn highest_pending_respects_mask() {
    let pending = [0x06u32]; // bits 1 and 2
    let enabled = [0x06u32];
    let priority = [0xFFu8, 0x10, 0x20, 0xFF];
    assert_eq!(
        highest_pending_in_range(&pending, &enabled, &priority, 0xFF, 0..4),
        Some(1) // priority 0x10 < 0x20
    );
    assert_eq!(
        highest_pending_in_range(&pending, &enabled, &priority, 0x15, 0..4),
        Some(1) // 0x10 < 0x15
    );
    assert_eq!(
        highest_pending_in_range(&pending, &enabled, &priority, 0x10, 0..4),
        None // nothing below mask 0x10
    );
}

// ── GicVersion ──────────────────────────────────────────────────────────────

#[test]
fn gic_version_v3_check() {
    assert!(GicVersion::V3.is_v3_or_later());
    assert!(GicVersion::V4.is_v3_or_later());
    assert!(!GicVersion::V2.is_v3_or_later());
}

// ── Distributor ─────────────────────────────────────────────────────────────

#[test]
fn dist_v2_typer() {
    let dist = GicDistributor::new(GicVersion::V2, 96);
    assert_eq!(dist.read(0x004) & 0x1F, 2); // 96/32 - 1 = 2
}

#[test]
fn dist_v3_iidr() {
    let dist = GicDistributor::new(GicVersion::V3, 256);
    assert_eq!(dist.read(0x008), 0x0300_043B);
}

#[test]
fn dist_v3_pidr2() {
    let dist = GicDistributor::new(GicVersion::V3, 256);
    assert_eq!(dist.read(0xFFE8), 0x30); // ArchRev = 3
}

#[test]
fn dist_v3_irouter_readwrite() {
    let mut dist = GicDistributor::new(GicVersion::V3, 256);
    dist.ctrl = 0x37; // enable with ARE
    let offset = 0x6100; // IROUTER for SPI 32
    dist.write(offset, 0x0000_0003); // low word: Aff0 = 3
    dist.write(offset + 4, 0x0000_0001); // high word: Aff1 = 1
    assert_eq!(dist.read(offset), 0x0000_0003);
    assert_eq!(dist.read(offset + 4), 0x0000_0001);
    assert_eq!(dist.spi_target_pe(32), 3); // Aff0 bits
}

#[test]
fn dist_v3_are_makes_itargetsr_raz() {
    let mut dist = GicDistributor::new(GicVersion::V3, 256);
    dist.ctrl = 0x37; // ARE enabled
    dist.write(0x800, 0xFF); // ITARGETSR write — should be ignored
    assert_eq!(dist.read(0x800), 0); // RAZ when ARE=1
}

#[test]
fn dist_enable_pending_active_cycle() {
    let mut dist = GicDistributor::new(GicVersion::V3, 256);
    dist.write(0x104, 1 << 1); // ISENABLER1 bit 1 → IRQ 33
    assert!(dist.is_irq_enabled(33));
    dist.set_pending(33);
    assert!(dist.is_irq_pending(33));
    dist.set_active(33);
    assert_eq!(dist.read(0x300 + 4) & (1 << 1), 1 << 1); // ISACTIVER1
    dist.clear_active(33);
    assert_eq!(dist.read(0x380 + 4) & (1 << 1), 0);
}

#[test]
fn dist_reset_clears_all() {
    let mut dist = GicDistributor::new(GicVersion::V3, 256);
    dist.ctrl = 0x37;
    dist.set_pending(40);
    dist.reset();
    assert_eq!(dist.ctrl, 0);
    assert!(!dist.is_irq_pending(40));
}

// ── Redistributor ───────────────────────────────────────────────────────────

#[test]
fn redist_typer_encodes_affinity_and_last() {
    let rd = GicRedistributor::new(5, true);
    let lo = rd.read_rd(0x008); // GICR_TYPER low word
    assert_eq!((lo >> 24) & 0xFF, 5); // Aff0
    assert_ne!(lo & (1 << 4), 0); // Last bit
}

#[test]
fn redist_waker_sleep_protocol() {
    let mut rd = GicRedistributor::new(0, false);
    assert!(!rd.is_awake()); // ProcessorSleep=1 at reset
    rd.write_rd(0x014, 0x00); // clear ProcessorSleep
    assert!(rd.is_awake());
    assert_eq!(rd.waker & 0x04, 0); // ChildrenAsleep also cleared
}

#[test]
fn redist_sgi_enable_pending() {
    let mut rd = GicRedistributor::new(0, false);
    rd.write_sgi(0x100, 1 << 5); // ISENABLER0 bit 5
    assert_eq!(rd.sgi_ppi_enabled & (1 << 5), 1 << 5);
    rd.set_pending(5);
    assert_eq!(rd.highest_pending_sgi_ppi(0xFF), Some(5));
}

#[test]
fn redist_sgi_priority() {
    let mut rd = GicRedistributor::new(0, false);
    rd.write_sgi(0x100, 0xFFFF_FFFF); // enable all
    rd.sgi_ppi_priority[3] = 0x10;
    rd.sgi_ppi_priority[7] = 0x20;
    rd.set_pending(3);
    rd.set_pending(7);
    assert_eq!(rd.highest_pending_sgi_ppi(0xFF), Some(3));
}

#[test]
fn redist_reset_clears_state() {
    let mut rd = GicRedistributor::new(0, false);
    rd.set_pending(10);
    rd.reset();
    assert_eq!(rd.sgi_ppi_pending, 0);
}

// ── ICC state ───────────────────────────────────────────────────────────────

#[test]
fn icc_sre_is_read_as_one() {
    let icc = IccState::new();
    assert_eq!(icc.read_simple(IccReg::Sre), 0x7);
}

#[test]
fn icc_pmr_readwrite() {
    let mut icc = IccState::new();
    icc.write_simple(IccReg::Pmr, 0xF0);
    assert_eq!(icc.read_simple(IccReg::Pmr), 0xF0);
}

#[test]
fn icc_priority_drop_and_deactivate() {
    let mut icc = IccState::new();
    assert_eq!(icc.running_priority, 0xFF);
    icc.priority_drop(0x30);
    assert_eq!(icc.running_priority, 0x30);
    icc.priority_drop(0x10);
    assert_eq!(icc.running_priority, 0x10);
    icc.deactivate();
    assert_eq!(icc.running_priority, 0x30);
    icc.deactivate();
    assert_eq!(icc.running_priority, 0xFF);
}

#[test]
fn icc_eoi_mode_flag() {
    let mut icc = IccState::new();
    assert!(!icc.eoi_mode());
    icc.write_simple(IccReg::Ctlr, 0x2);
    assert!(icc.eoi_mode());
}

#[test]
fn icc_reg_sysreg_decode() {
    assert_eq!(IccReg::from_sysreg(3, 0, 12, 12, 0), Some(IccReg::Iar1));
    assert_eq!(IccReg::from_sysreg(3, 0, 12, 11, 5), Some(IccReg::Sgi1r));
    assert_eq!(IccReg::from_sysreg(3, 7, 0, 0, 0), None);
}

// ── LPI ─────────────────────────────────────────────────────────────────────

#[test]
fn lpi_pending_set_clear() {
    let mut table = LpiPendingTable::new(256);
    table.set_pending(8192);
    assert!(table.is_pending(8192));
    table.clear_pending(8192);
    assert!(!table.is_pending(8192));
}

#[test]
fn lpi_pending_out_of_range() {
    let mut table = LpiPendingTable::new(64);
    table.set_pending(8192 + 100); // beyond capacity
    assert!(!table.is_pending(8192 + 100));
}

#[test]
fn lpi_config_roundtrip() {
    let config = LpiConfig {
        priority: 0xA0,
        enabled: true,
    };
    let byte = config.to_byte();
    let decoded = LpiConfig::from_byte(byte);
    assert_eq!(decoded.priority, 0xA0);
    assert!(decoded.enabled);
}

#[test]
fn lpi_config_table_get_set() {
    let mut table = LpiConfigTable::new(128);
    table.set(
        8200,
        LpiConfig {
            priority: 0x40,
            enabled: true,
        },
    );
    let cfg = table.get(8200).unwrap();
    assert_eq!(cfg.priority, 0x40);
    assert!(cfg.enabled);
}

// ── ITS ─────────────────────────────────────────────────────────────────────

#[test]
fn its_mapd_mapti_int() {
    let mut its = GicIts::new();
    its.cmd_mapd(1, true); // device 1
    its.cmd_mapc(0, 2, true); // collection 0 → PE 2
    its.cmd_mapti(1, 100, 8200, 0); // dev 1, event 100 → LPI 8200, coll 0

    let result = its.cmd_int(1, 100);
    assert_eq!(result, Some((2, 8200)));

    let pending = its.drain_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0], (2, 8200));
}

#[test]
fn its_mapi_shorthand() {
    let mut its = GicIts::new();
    its.cmd_mapd(5, true);
    its.cmd_mapc(1, 0, true);
    its.cmd_mapi(5, 8300, 1); // INTID = EventID = 8300

    let result = its.cmd_int(5, 8300);
    assert_eq!(result, Some((0, 8300)));
}

#[test]
fn its_discard_removes_mapping() {
    let mut its = GicIts::new();
    its.cmd_mapd(1, true);
    its.cmd_mapc(0, 0, true);
    its.cmd_mapti(1, 50, 9000, 0);
    its.cmd_discard(1, 50);
    assert_eq!(its.cmd_int(1, 50), None);
}

#[test]
fn its_register_readwrite() {
    let mut its = GicIts::new();
    its.write(0x000, 1); // GITS_CTLR = enabled
    assert_eq!(its.read(0x000), 1);
    assert!(its.is_enabled());
}

#[test]
fn its_reset_clears_all() {
    let mut its = GicIts::new();
    its.cmd_mapd(1, true);
    its.reset();
    assert!(its.devices.is_empty());
    assert!(!its.is_enabled());
}

// ── GicV3 ───────────────────────────────────────────────────────────────────

#[test]
fn gicv3_gicd_typer_via_mmio() {
    let mut gic = GicV3::new("gic", 256, 4);
    let typer = read_reg(&mut gic, 0x004);
    assert_eq!(typer & 0x1F, 7); // 256/32 - 1 = 7
}

#[test]
fn gicv3_gicd_pidr2() {
    let mut gic = GicV3::new("gic", 256, 1);
    let pidr2 = read_reg(&mut gic, 0xFFE8);
    assert_eq!(pidr2 >> 4, 3); // ArchRev = 3
}

#[test]
fn gicv3_redist_typer_last_bit() {
    let mut gic = GicV3::new("gic", 256, 3);
    let rd0_offset = 0xA_0000;
    let typer_lo_0 = read_reg(&mut gic, rd0_offset + 0x008);
    assert_eq!(typer_lo_0 & (1 << 4), 0); // not last

    let rd2_offset = 0xA_0000 + 2 * 0x20000;
    let typer_lo_2 = read_reg(&mut gic, rd2_offset + 0x008);
    assert_ne!(typer_lo_2 & (1 << 4), 0); // last
}

#[test]
fn gicv3_redist_waker_via_mmio() {
    let mut gic = GicV3::new("gic", 256, 1);
    let rd_offset = 0xA_0000;
    let waker = read_reg(&mut gic, rd_offset + 0x014);
    assert_ne!(waker & 0x02, 0); // ProcessorSleep set at reset
    write_reg(&mut gic, rd_offset + 0x014, 0x00);
    let waker2 = read_reg(&mut gic, rd_offset + 0x014);
    assert_eq!(waker2 & 0x02, 0); // cleared
}

#[test]
fn gicv3_sgi_enable_via_redist() {
    let mut gic = GicV3::new("gic", 256, 1);
    let sgi_offset = 0xA_0000 + 0x10000;
    write_reg(&mut gic, sgi_offset + 0x100, 1 << 5); // ISENABLER0 bit 5
    let val = read_reg(&mut gic, sgi_offset + 0x100);
    assert_ne!(val & (1 << 5), 0);
}

#[test]
fn gicv3_inject_spi_pending_ack() {
    let mut gic = GicV3::new("gic", 256, 1);
    write_reg(&mut gic, 0x000, 0x37); // GICD_CTLR: enable + ARE
    write_reg(&mut gic, 0x104, 1 << 1); // ISENABLER1 bit 1 → IRQ 33
    gic.icc_states[0].pmr = 0xFF;
    gic.icc_states[0].igrpen1 = 1;

    gic.inject(33, true);
    assert!(gic.pending_for_cpu(0));

    let irq = gic.ack(0);
    assert_eq!(irq, Some(33));
    assert!(!gic.pending_for_cpu(0));
}

#[test]
fn gicv3_inject_sgi_via_trait() {
    let mut gic = GicV3::new("gic", 256, 2);
    gic.redistributors[1].sgi_ppi_enabled = 0xFFFF;
    gic.icc_states[1].pmr = 0xFF;
    gic.icc_states[1].igrpen1 = 1;
    write_reg(&mut gic, 0x000, 0x37);

    gic.redistributors[1].set_pending(3);
    assert!(gic.pending_for_cpu(1));
    assert!(!gic.pending_for_cpu(0));
}

#[test]
fn gicv3_sysreg_iar_eoir_cycle() {
    let mut gic = GicV3::new("gic", 256, 1);
    write_reg(&mut gic, 0x000, 0x37);
    write_reg(&mut gic, 0x104, 1 << 1);
    gic.icc_states[0].pmr = 0xFF;
    gic.icc_states[0].igrpen1 = 1;

    gic.inject(33, true);
    let irq = gic.sysreg_read(0, IccReg::Iar1);
    assert_eq!(irq, 33);

    assert!(!gic.pending_for_cpu(0));

    gic.sysreg_write(0, IccReg::Eoir1, 33);
    assert_eq!(gic.icc_states[0].running_priority, 0xFF);
}

#[test]
fn gicv3_sysreg_sgi_generation() {
    let mut gic = GicV3::new("gic", 256, 4);
    gic.redistributors[2].sgi_ppi_enabled = 0xFFFF;
    // SGI 5 to PE with Aff0=2: TargetList bit 2, INTID=5
    let sgi_val: u64 = (5 << 24) | (1 << 2);
    gic.sysreg_write(0, IccReg::Sgi1r, sgi_val);

    assert_ne!(gic.redistributors[2].sgi_ppi_pending & (1 << 5), 0);
    assert_eq!(gic.redistributors[0].sgi_ppi_pending & (1 << 5), 0);
}

#[test]
fn gicv3_sysreg_sre_readonly() {
    let mut gic = GicV3::new("gic", 256, 1);
    let sre = gic.sysreg_read(0, IccReg::Sre);
    assert_eq!(sre, 0x7);
}

#[test]
fn gicv3_spurious_when_nothing_pending() {
    let mut gic = GicV3::new("gic", 256, 1);
    gic.icc_states[0].pmr = 0xFF;
    let irq = gic.sysreg_read(0, IccReg::Iar1);
    assert_eq!(irq, SPURIOUS_IRQ as u64);
}

#[test]
fn gicv3_reset_clears_everything() {
    let mut gic = GicV3::new("gic", 256, 2);
    gic.inject(40, true);
    gic.redistributors[1].set_pending(3);
    gic.reset().unwrap();
    assert!(!gic.distributor.is_irq_pending(40));
    assert_eq!(gic.redistributors[1].sgi_ppi_pending, 0);
}

// ── GicV4 ───────────────────────────────────────────────────────────────────

#[test]
fn gicv4_vmapp_schedule() {
    let mut gic = GicV4::new("gic", 256, 2, GicV4Version::V4);
    gic.cmd_vmapp(10, 1, true);
    assert!(gic.vpe_table.contains_key(&10));
    gic.schedule_vpe(10);
    assert!(gic.vpe_table[&10].resident);
    gic.deschedule_vpe(10);
    assert!(!gic.vpe_table[&10].resident);
}

#[test]
fn gicv4_vmapti_inject_resident() {
    let mut gic = GicV4::new("gic", 256, 2, GicV4Version::V4);
    gic.cmd_vmapp(1, 0, true);
    gic.schedule_vpe(1);
    gic.cmd_vmapti(100, 200, 9000, 1); // dev 100, event 200 → vINTID 9000, vPE 1

    let result = gic.inject_vlpi(100, 200);
    assert_eq!(result, Some((1, 9000)));
    let pending = gic.drain_pending_vlpis();
    assert_eq!(pending, vec![(1, 9000)]);
}

#[test]
fn gicv4_vmovi_moves_mapping() {
    let mut gic = GicV4::new("gic", 256, 2, GicV4Version::V4);
    gic.cmd_vmapp(1, 0, true);
    gic.cmd_vmapp(2, 1, true);
    gic.cmd_vmapti(10, 20, 9100, 1);
    gic.cmd_vmovi(10, 20, 2);

    let mapping = gic
        .vlpi_mappings
        .iter()
        .find(|m| m.device_id == 10 && m.event_id == 20)
        .unwrap();
    assert_eq!(mapping.vpe_id, 2);
}

#[test]
fn gicv4_inject_nonresident_doorbell() {
    let mut gic = GicV4::new("gic", 256, 2, GicV4Version::V4);
    gic.cmd_vmapp(1, 0, true);
    gic.vpe_table.get_mut(&1).unwrap().doorbell_intid = Some(40);
    gic.cmd_vmapti(10, 20, 9000, 1);
    gic.inner.distributor.ctrl = 0x37;
    gic.inner.distributor.write(0x104, 1 << (40 - 32)); // enable IRQ 40

    gic.inject_vlpi(10, 20);
    // vPE not resident → doorbell IRQ 40 injected to physical GIC
    assert!(gic.inner.distributor.is_irq_pending(40));
}

#[test]
fn gicv4_1_vsgi() {
    let mut gic = GicV4::new("gic", 256, 2, GicV4Version::V4_1);
    gic.cmd_vmapp(1, 0, true);
    gic.schedule_vpe(1);
    assert!(gic.inject_vsgi(1, 5));
    let pending = gic.drain_pending_vlpis();
    assert_eq!(pending, vec![(1, 5)]);
}

#[test]
fn gicv4_vsgi_rejected_on_v4_0() {
    let mut gic = GicV4::new("gic", 256, 2, GicV4Version::V4);
    gic.cmd_vmapp(1, 0, true);
    gic.schedule_vpe(1);
    assert!(!gic.inject_vsgi(1, 5)); // not v4.1
}

#[test]
fn gicv4_delegates_device_trait() {
    let mut gic = GicV4::new("gic", 256, 1, GicV4Version::V4);
    let typer = read_reg(&mut gic, 0x004);
    assert_eq!(typer & 0x1F, 7);
}

#[test]
fn gicv4_delegates_interrupt_controller() {
    let mut gic = GicV4::new("gic", 256, 1, GicV4Version::V4);
    gic.inner.distributor.ctrl = 0x37;
    gic.inner.distributor.write(0x104, 1 << 1); // enable IRQ 33
    gic.inner.icc_states[0].pmr = 0xFF;
    gic.inner.icc_states[0].igrpen1 = 1;

    gic.inject(33, true);
    assert!(gic.pending_for_cpu(0));
    assert_eq!(gic.ack(0), Some(33));
}

#[test]
fn gicv4_reset_clears_vpe_table() {
    let mut gic = GicV4::new("gic", 256, 1, GicV4Version::V4);
    gic.cmd_vmapp(1, 0, true);
    gic.reset().unwrap();
    assert!(gic.vpe_table.is_empty());
}
