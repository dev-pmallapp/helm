//! Harness-backed SMMUv3 requester and translation tests.

mod common;

use common::io_harness::DummyDmaRequester;
use helm_hw_iommu::smmu::{SmmuState, SmmuTranslateResult};
use helm_memory::{FlatMem, HelmAddressSpace};

const STRTAB_BASE: u64 = 0x10_000;
const CD_BASE: u64 = 0x20_000;
const S1_L1_64K_TABLE: u64 = 0x30_000;
const S1_L2_64K_TABLE: u64 = 0x40_000;
const S2_L1_64K_TABLE: u64 = 0x50_000;
const S2_L2_64K_TABLE: u64 = 0x60_000;
const SRC_PA: u64 = 0x90_000;
const DST_PA: u64 = 0xA0_000;

fn build_smmu_harness() -> SmmuState<HelmAddressSpace> {
    let mut mem = HelmAddressSpace::new(FlatMem::new(0, 0x20_0000));

    let ste_dw0: u64 = 0x1 | (0b111 << 1) | (CD_BASE & 0x000F_FFFF_FFFF_FFC0);
    let ste_dw2: u64 =
        (28u64 & 0x3F) | ((1u64 & 0x3) << 56) | (S2_L1_64K_TABLE & 0x000F_FFFF_FFFF_FFF0);
    mem.write_bytes(STRTAB_BASE, &ste_dw0.to_le_bytes())
        .unwrap();
    mem.write_bytes(STRTAB_BASE + 8, &0u64.to_le_bytes())
        .unwrap();
    mem.write_bytes(STRTAB_BASE + 16, &ste_dw2.to_le_bytes())
        .unwrap();
    mem.write_bytes(STRTAB_BASE + 24, &0u64.to_le_bytes())
        .unwrap();

    let cd_dw0: u64 = (1u64 << 31) | (28u64 << 32) | (1u64 << 46);
    let cd_dw1: u64 = (42u64 << 48) | (S1_L1_64K_TABLE & 0x0000_FFFF_FFFF_0000);
    mem.write_bytes(CD_BASE, &cd_dw0.to_le_bytes()).unwrap();
    mem.write_bytes(CD_BASE + 8, &cd_dw1.to_le_bytes()).unwrap();

    mem.write_bytes(S1_L1_64K_TABLE, &(S1_L2_64K_TABLE | 0x3).to_le_bytes())
        .unwrap();
    mem.write_bytes(
        S1_L2_64K_TABLE + 5 * 8,
        &(0x50_000u64 | 0x3 | (0b01 << 6) | (1 << 10)).to_le_bytes(),
    )
    .unwrap();
    mem.write_bytes(
        S1_L2_64K_TABLE + 6 * 8,
        &(0x60_000u64 | 0x3 | (0b01 << 6) | (1 << 10)).to_le_bytes(),
    )
    .unwrap();

    mem.write_bytes(S2_L1_64K_TABLE, &(S2_L2_64K_TABLE | 0x3).to_le_bytes())
        .unwrap();
    mem.write_bytes(
        S2_L2_64K_TABLE + 5 * 8,
        &(SRC_PA | 0x3 | (0b01 << 6) | (1 << 10)).to_le_bytes(),
    )
    .unwrap();
    mem.write_bytes(
        S2_L2_64K_TABLE + 6 * 8,
        &(DST_PA | 0x3 | (0b01 << 6) | (1 << 10)).to_le_bytes(),
    )
    .unwrap();

    let mut smmu = SmmuState::new(mem);
    smmu.strtab_base = STRTAB_BASE;
    smmu.strtab_log2size = 1;
    smmu.cr0 = 0x7;
    smmu.cr0ack = smmu.cr0;
    smmu
}

#[test]
fn generic_requester_can_drive_smmuv3_s1s2_translation() {
    let mut smmu = build_smmu_harness();
    let req = DummyDmaRequester::new(0);

    smmu.mem.write_bytes(SRC_PA, b"payload").unwrap();
    smmu.dma_copy(req.requester_id(), 0x50_000, 0x60_000, 7)
        .unwrap();

    let mut buf = [0u8; 7];
    smmu.mem.read_bytes(DST_PA, &mut buf).unwrap();
    assert_eq!(&buf, b"payload");
}

#[test]
fn generic_requester_sees_explicit_translation_faults() {
    let mut smmu = build_smmu_harness();
    let req = DummyDmaRequester::new(0);

    let err = smmu
        .dma_read(req.requester_id(), 0x70_000, &mut [0u8; 8])
        .unwrap_err();
    assert_eq!(err.input_addr, 0x70_000);
    assert_eq!(err.stream_id, req.requester_id());
}

#[test]
fn s2_only_no_longer_bypasses() {
    let mut mem = HelmAddressSpace::new(FlatMem::new(0, 0x10_0000));
    let ste_dw0: u64 = 0x1 | (0b110 << 1);
    let ste_dw2: u64 = (25u64 & 0x3F) | (0x30_000u64 & 0x000F_FFFF_FFFF_FFF0);
    mem.write_bytes(STRTAB_BASE, &ste_dw0.to_le_bytes())
        .unwrap();
    mem.write_bytes(STRTAB_BASE + 16, &ste_dw2.to_le_bytes())
        .unwrap();
    mem.write_bytes(0x30_000, &(0x31_000u64 | 0x3).to_le_bytes())
        .unwrap();
    mem.write_bytes(0x31_000, &(0x32_000u64 | 0x3).to_le_bytes())
        .unwrap();
    mem.write_bytes(
        0x32_000 + 8,
        &(0x40_000u64 | 0x3 | (0b01 << 6) | (1 << 10)).to_le_bytes(),
    )
    .unwrap();

    let mut smmu = SmmuState::new(mem);
    smmu.strtab_base = STRTAB_BASE;
    smmu.strtab_log2size = 1;
    smmu.cr0 = 0x7;
    smmu.cr0ack = smmu.cr0;

    match smmu.translate(0, 0x1000, false) {
        SmmuTranslateResult::Ok(pa) => assert_eq!(pa, 0x40_000),
        other => panic!("expected S2 translation, got {other:?}"),
    }
}
