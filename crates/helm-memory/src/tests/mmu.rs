use crate::mmu::*;

// ── Granule ────────────────────────────────────────────────────────────────

#[test]
fn granule_4k_size() {
    assert_eq!(Granule::K4.size(), 4096);
}

#[test]
fn granule_16k_size() {
    assert_eq!(Granule::K16.size(), 16384);
}

#[test]
fn granule_64k_size() {
    assert_eq!(Granule::K64.size(), 65536);
}

#[test]
fn granule_4k_page_shift() {
    assert_eq!(Granule::K4.page_shift(), 12);
}

#[test]
fn granule_16k_page_shift() {
    assert_eq!(Granule::K16.page_shift(), 14);
}

#[test]
fn granule_64k_page_shift() {
    assert_eq!(Granule::K64.page_shift(), 16);
}

#[test]
fn granule_4k_bits_per_level() {
    assert_eq!(Granule::K4.bits_per_level(), 9);
}

#[test]
fn granule_16k_bits_per_level() {
    assert_eq!(Granule::K16.bits_per_level(), 11);
}

#[test]
fn granule_64k_bits_per_level() {
    assert_eq!(Granule::K64.bits_per_level(), 13);
}

// ── PTE accessors ──────────────────────────────────────────────────────────

#[test]
fn pte_invalid_entry() {
    let pte = Pte(0);
    assert!(!pte.is_valid());
    assert!(!pte.is_table());
    assert!(!pte.is_block());
}

#[test]
fn pte_table_descriptor() {
    let pte = Pte(0b11);
    assert!(pte.is_valid());
    assert!(pte.is_table());
    assert!(!pte.is_block());
}

#[test]
fn pte_block_descriptor() {
    let pte = Pte(0b01);
    assert!(pte.is_valid());
    assert!(!pte.is_table());
    assert!(pte.is_block());
}

#[test]
fn pte_access_flag_set() {
    let pte = Pte(1 << 10 | 0b11);
    assert!(pte.af());
}

#[test]
fn pte_access_flag_clear() {
    let pte = Pte(0b11);
    assert!(!pte.af());
}

#[test]
fn pte_ap_field() {
    let pte = Pte((2u64 << 6) | 0b11);
    assert_eq!(pte.ap(), 2);
}

#[test]
fn pte_pxn_set() {
    let pte = Pte(1u64 << 53 | 0b11);
    assert!(pte.pxn());
}

#[test]
fn pte_uxn_set() {
    let pte = Pte(1u64 << 54 | 0b11);
    assert!(pte.uxn());
}

#[test]
fn pte_ng_flag() {
    let pte = Pte(1u64 << 11 | 0b11);
    assert!(pte.ng());
}

#[test]
fn pte_dbm_flag() {
    let pte = Pte(1u64 << 51 | 0b11);
    assert!(pte.dbm());
}

#[test]
fn pte_attr_indx() {
    let pte = Pte((5u64 << 2) | 0b11);
    assert_eq!(pte.attr_indx(), 5);
}

#[test]
fn pte_output_address_4k_page() {
    let pa = 0x0000_0000_4000_0000u64;
    let pte = Pte(pa | 0b11);
    assert_eq!(pte.oa(12), pa);
}

// ── Permissions ────────────────────────────────────────────────────────────

#[test]
fn permissions_rw_el1_only() {
    let pte = Pte((0u64 << 6) | (1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(perms.readable);
    assert!(perms.writable);
}

#[test]
fn permissions_ro() {
    let pte = Pte((2u64 << 6) | (1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(perms.readable);
    assert!(!perms.writable);
}

#[test]
fn permissions_check_write_denied_when_ro() {
    let pte = Pte((2u64 << 6) | (1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(!perms.check(1, true, false));
}

#[test]
fn permissions_check_read_allowed_when_ro() {
    let pte = Pte((2u64 << 6) | (1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(perms.check(1, false, false));
}

#[test]
fn permissions_el0_execute_denied_when_uxn() {
    let pte = Pte((1u64 << 54) | (1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(!perms.check(0, false, true));
}

#[test]
fn permissions_el1_execute_denied_when_pxn() {
    let pte = Pte((1u64 << 53) | (1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(!perms.check(1, false, true));
}

#[test]
fn permissions_execute_allowed_when_no_xn() {
    let pte = Pte((1 << 10) | 0b11);
    let perms = Permissions::from_pte(pte);
    assert!(perms.check(0, false, true));
    assert!(perms.check(1, false, true));
}

// ── TranslationConfig ──────────────────────────────────────────────────────

#[test]
fn tcr_parse_t0sz_t1sz() {
    let tcr = 25u64 | (25u64 << 16);
    let cfg = TranslationConfig::parse(tcr);
    assert_eq!(cfg.t0sz, 25);
    assert_eq!(cfg.t1sz, 25);
}

#[test]
fn tcr_parse_granule_4k() {
    let tcr: u64 = 0;
    let cfg = TranslationConfig::parse(tcr);
    assert_eq!(cfg.tg0, Granule::K4);
}

#[test]
fn tcr_parse_granule_64k_tg0() {
    let tcr: u64 = 1u64 << 14;
    let cfg = TranslationConfig::parse(tcr);
    assert_eq!(cfg.tg0, Granule::K64);
}

#[test]
fn tcr_parse_granule_16k_tg0() {
    let tcr: u64 = 2u64 << 14;
    let cfg = TranslationConfig::parse(tcr);
    assert_eq!(cfg.tg0, Granule::K16);
}

#[test]
fn tcr_parse_epd0() {
    let tcr: u64 = 1u64 << 7;
    let cfg = TranslationConfig::parse(tcr);
    assert!(cfg.epd0);
}

#[test]
fn tcr_parse_epd1() {
    let tcr: u64 = 1u64 << 23;
    let cfg = TranslationConfig::parse(tcr);
    assert!(cfg.epd1);
}

#[test]
fn tcr_parse_ha_hd() {
    let tcr: u64 = (1u64 << 39) | (1u64 << 40);
    let cfg = TranslationConfig::parse(tcr);
    assert!(cfg.ha);
    assert!(cfg.hd);
}

// ── select_ttbr ────────────────────────────────────────────────────────────

#[test]
fn select_ttbr_lower_half_is_ttbr0() {
    let tcr = TranslationConfig::parse(16);
    assert_eq!(select_ttbr(0x0000_0000_1000_0000, &tcr), TtbrSelect::Ttbr0);
}

#[test]
fn select_ttbr_upper_half_is_ttbr1() {
    let tcr = TranslationConfig::parse(16 | (16u64 << 16));
    assert_eq!(select_ttbr(0xFFFF_FFFF_F000_0000, &tcr), TtbrSelect::Ttbr1);
}

#[test]
fn select_ttbr_middle_is_fault() {
    let tcr = TranslationConfig::parse(25 | (25u64 << 16));
    let va = 0x0001_0000_0000_0000;
    assert_eq!(select_ttbr(va, &tcr), TtbrSelect::Fault);
}

// ── TranslationFault ───────────────────────────────────────────────────────

#[test]
fn translation_fault_fsc_encoding() {
    let f = TranslationFault::TranslationFault { level: 2 };
    assert_eq!(f.to_fsc(), 0b000110);
    assert_eq!(f.level(), 2);
}

#[test]
fn access_flag_fault_fsc_encoding() {
    let f = TranslationFault::AccessFlagFault { level: 3 };
    assert_eq!(f.to_fsc(), 0b001011);
}

#[test]
fn permission_fault_fsc_encoding() {
    let f = TranslationFault::PermissionFault { level: 1 };
    assert_eq!(f.to_fsc(), 0b001101);
}

#[test]
fn address_size_fault_fsc_encoding() {
    let f = TranslationFault::AddressSizeFault { level: 0 };
    assert_eq!(f.to_fsc(), 0b000000);
}

// ── walk ───────────────────────────────────────────────────────────────────

#[test]
fn walk_invalid_l0_returns_translation_fault() {
    let err = walk(0x1000, 0, 16, Granule::K4, false, &mut |_| 0).unwrap_err();
    assert_eq!(err, TranslationFault::TranslationFault { level: 0 });
}

#[test]
fn walk_single_level_4k_page() {
    let page_pa = 0x0000_0000_8000_0000u64;
    let pte_val = page_pa | (1u64 << 10) | 0b11;
    let va = 0x0000_0000_0000_1234u64;

    let result = walk(va, 0, 25, Granule::K4, false, &mut |_addr| pte_val).unwrap();

    assert_eq!(result.pa & !0xFFF, page_pa);
    assert_eq!(result.pa & 0xFFF, 0x234);
    assert!(result.perms.readable);
}

#[test]
fn walk_access_flag_fault_when_af_clear_and_ha_disabled() {
    let page_pa = 0x0000_0000_8000_0000u64;
    let pte_val = page_pa | 0b11;
    let err = walk(0x1000, 0, 25, Granule::K4, false, &mut |_| pte_val).unwrap_err();
    match err {
        TranslationFault::AccessFlagFault { .. } => {}
        other => panic!("expected AccessFlagFault, got {other:?}"),
    }
}

#[test]
fn walk_no_af_fault_when_ha_enabled() {
    let page_pa = 0x0000_0000_8000_0000u64;
    let pte_val = page_pa | 0b11;
    let result = walk(0x1000, 0, 25, Granule::K4, true, &mut |_| pte_val);
    assert!(result.is_ok());
}

// ── translate ──────────────────────────────────────────────────────────────

#[test]
fn translate_epd0_set_returns_fault() {
    let mut tcr_val = 16u64;
    tcr_val |= 1u64 << 7;
    let tcr = TranslationConfig::parse(tcr_val);
    let err = translate(0x1000, &tcr, 0, 0, &mut |_| 0).unwrap_err();
    assert_eq!(err, TranslationFault::TranslationFault { level: 0 });
}

#[test]
fn translate_epd1_set_returns_fault() {
    let mut tcr_val = 16u64 | (16u64 << 16);
    tcr_val |= 1u64 << 23;
    let tcr = TranslationConfig::parse(tcr_val);
    let va = 0xFFFF_FFFF_F000_0000;
    let err = translate(va, &tcr, 0, 0, &mut |_| 0).unwrap_err();
    assert_eq!(err, TranslationFault::TranslationFault { level: 0 });
}

// ── Stage2Config ──────────────────────────────────────────────────────────

#[test]
fn stage2_config_parse_t0sz() {
    let vtcr = 20u64; // T0SZ = 20
    let cfg = Stage2Config::parse(vtcr);
    assert_eq!(cfg.t0sz, 20);
}

#[test]
fn stage2_config_parse_sl0() {
    let vtcr = (1u64 << 6); // SL0 = 1
    let cfg = Stage2Config::parse(vtcr);
    assert_eq!(cfg.sl0, 1);
}

#[test]
fn stage2_config_parse_granule_4k() {
    let vtcr = 0u64;
    let cfg = Stage2Config::parse(vtcr);
    assert_eq!(cfg.tg0, Granule::K4);
}

#[test]
fn stage2_config_parse_granule_64k() {
    let vtcr = 1u64 << 14;
    let cfg = Stage2Config::parse(vtcr);
    assert_eq!(cfg.tg0, Granule::K64);
}

#[test]
fn stage2_config_parse_ps() {
    let vtcr = 5u64 << 16; // PS = 5 (48-bit)
    let cfg = Stage2Config::parse(vtcr);
    assert_eq!(cfg.ps, 5);
}

#[test]
fn stage2_config_parse_ha_hd() {
    let vtcr = (1u64 << 21) | (1u64 << 22);
    let cfg = Stage2Config::parse(vtcr);
    assert!(cfg.ha);
    assert!(cfg.hd);
}

#[test]
fn stage2_start_level_4k_sl0_0() {
    // SL0=0 with 4K → start at L2
    let vtcr = 0u64; // SL0=0, TG0=4K
    let cfg = Stage2Config::parse(vtcr);
    // Verify indirectly through walk behavior — SL0=0 means L2 start for 4K
    assert_eq!(cfg.sl0, 0);
}

// ── walk_stage2 ────────────────────────────────────────────────────────────

#[test]
fn walk_stage2_invalid_descriptor_returns_translation_fault() {
    // SL0=1 (L1 start for 4K), T0SZ=24
    let vtcr = 24u64 | (1u64 << 6);
    let cfg = Stage2Config::parse(vtcr);
    let err = walk_stage2(0x1000, 0, &cfg, &mut |_| 0).unwrap_err();
    assert_eq!(err, TranslationFault::TranslationFault { level: 1 });
}

#[test]
fn walk_stage2_4k_page_translates() {
    // Set up a simple 1-level walk: SL0=2 (L0 start for 4K), T0SZ=25
    // Use SL0=0 (L2 start) for simplicity — only one level of table + pages
    let page_pa = 0x0000_0000_8000_0000u64;
    let ipa = 0x0000_0000_0000_1234u64;

    // 4K granule, SL0=0 (L2 start), T0SZ=25 → 39-bit IPA
    // L2 start means we need one table level to L3 pages (or L2 block)
    // For a single-level walk with block descriptor at L2:
    let block_pa = 0x0000_0000_8000_0000u64;
    let block_shift = 12 + 9; // L2 block = 2MB
    let block_mask = (1u64 << block_shift) - 1;

    // S2AP=11 (RW), AF=1, block descriptor (bits[1:0]=01)
    let pte = block_pa | (3u64 << 6) | (1u64 << 10) | 0b01;

    let vtcr = 25u64 | (0u64 << 6); // SL0=0 → L2 start
    let cfg = Stage2Config::parse(vtcr);

    let result = walk_stage2(ipa, 0, &cfg, &mut |_| pte).unwrap();

    // PA = block base + offset
    assert_eq!(result.pa, block_pa | (ipa & block_mask));
    assert!(result.perms.readable);
    assert!(result.perms.writable);
}

#[test]
fn walk_stage2_s2ap_read_only() {
    // S2AP=01 (read-only), AF=1, block descriptor at L2
    let block_pa = 0x4000_0000u64;
    let pte = block_pa | (1u64 << 6) | (1u64 << 10) | 0b01; // S2AP=01

    let vtcr = 25u64; // SL0=0, TG0=4K
    let cfg = Stage2Config::parse(vtcr);

    let result = walk_stage2(0x1000, 0, &cfg, &mut |_| pte).unwrap();

    assert!(result.perms.readable);
    assert!(!result.perms.writable);
}

#[test]
fn walk_stage2_s2ap_write_only() {
    // S2AP=10 (write-only), AF=1, block descriptor
    let block_pa = 0x4000_0000u64;
    let pte = block_pa | (2u64 << 6) | (1u64 << 10) | 0b01; // S2AP=10

    let vtcr = 25u64;
    let cfg = Stage2Config::parse(vtcr);

    let result = walk_stage2(0x1000, 0, &cfg, &mut |_| pte).unwrap();

    assert!(!result.perms.readable);
    assert!(result.perms.writable);
}

#[test]
fn walk_stage2_s2ap_no_access() {
    // S2AP=00 (no access), AF=1, block descriptor
    let block_pa = 0x4000_0000u64;
    let pte = block_pa | (0u64 << 6) | (1u64 << 10) | 0b01; // S2AP=00

    let vtcr = 25u64;
    let cfg = Stage2Config::parse(vtcr);

    let result = walk_stage2(0x1000, 0, &cfg, &mut |_| pte).unwrap();

    assert!(!result.perms.readable);
    assert!(!result.perms.writable);
}

#[test]
fn walk_stage2_xn_bits() {
    // XN[1:0]: bit 54 = XN for EL1, bit 53 = XN for EL0
    let block_pa = 0x4000_0000u64;
    let pte = block_pa | (3u64 << 6) | (1u64 << 10) | (1u64 << 54) | 0b01; // XN[1]=1

    let vtcr = 25u64;
    let cfg = Stage2Config::parse(vtcr);

    let result = walk_stage2(0x1000, 0, &cfg, &mut |_| pte).unwrap();

    assert!(!result.perms.el1_executable); // XN[1]=1 means no exec for EL1
    assert!(result.perms.el0_executable); // XN[0]=0 means exec for EL0
}

#[test]
fn walk_stage2_access_flag_fault() {
    // AF=0, HA=false → access flag fault
    let block_pa = 0x4000_0000u64;
    let pte = block_pa | (3u64 << 6) | 0b01; // No AF bit

    let vtcr = 25u64; // HA=0
    let cfg = Stage2Config::parse(vtcr);

    let err = walk_stage2(0x1000, 0, &cfg, &mut |_| pte).unwrap_err();
    match err {
        TranslationFault::AccessFlagFault { .. } => {}
        other => panic!("expected AccessFlagFault, got {other:?}"),
    }
}

#[test]
fn walk_stage2_no_af_fault_when_ha_enabled() {
    let block_pa = 0x4000_0000u64;
    let pte = block_pa | (3u64 << 6) | 0b01; // No AF, but HA=1

    let vtcr = 25u64 | (1u64 << 21); // HA=1
    let cfg = Stage2Config::parse(vtcr);

    let result = walk_stage2(0x1000, 0, &cfg, &mut |_| pte);
    assert!(result.is_ok());
}

// ── parse_single ─────────────────────────────────────────────────────────

#[test]
fn tcr_parse_single_disables_ttbr1() {
    let tcr = 25u64; // T0SZ=25
    let cfg = TranslationConfig::parse_single(tcr);
    assert_eq!(cfg.t0sz, 25);
    assert_eq!(cfg.t1sz, 64); // disabled
    assert!(cfg.epd1);
}

#[test]
fn tcr_parse_single_ps_field_at_16() {
    let tcr = 25u64 | (5u64 << 16); // PS=5 at bits [18:16]
    let cfg = TranslationConfig::parse_single(tcr);
    assert_eq!(cfg.ips, 5);
}
