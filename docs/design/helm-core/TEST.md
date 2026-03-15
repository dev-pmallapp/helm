# helm-core — Test Plan

> Version: 0.1.0
> Status: Draft
> Cross-references: [HLD.md](HLD.md) · [LLD-arch-state.md](LLD-arch-state.md) · [LLD-interfaces.md](LLD-interfaces.md)

---

## Table of Contents

1. [Test Strategy and Location](#1-test-strategy-and-location)
2. [Unit Tests: RiscvArchState](#2-unit-tests-riscvarchstate)
3. [Unit Tests: Aarch64ArchState](#3-unit-tests-aarch64archstate)
4. [Property Tests: ArchState Roundtrip](#4-property-tests-archstate-roundtrip)
5. [Unit Tests: MemFault Propagation](#5-unit-tests-memfault-propagation)
6. [Unit Tests: HartException](#6-unit-tests-hartexception)
7. [Functional Separation Tests: ExecContext vs ThreadContext](#7-functional-separation-tests-execcontext-vs-threadcontext)
8. [Mock Implementations](#8-mock-implementations)
9. [Test Coverage Targets](#9-test-coverage-targets)

---

## 1. Test Strategy and Location

All tests in this section live in `helm-core`. They test only what `helm-core` exports — no `helm-arch`, `helm-engine`, or `helm-memory` code is imported.

| Test type | Location | Crate |
|---|---|---|
| Unit tests for `ArchState` | `helm-core/src/arch_state/riscv.rs` (inline `#[cfg(test)]`) | `helm-core` |
| Unit tests for `Aarch64ArchState` | `helm-core/src/arch_state/aarch64.rs` | `helm-core` |
| Property tests (`proptest`) | `helm-core/tests/proptest_arch_state.rs` | `helm-core` (integration test) |
| MemFault tests | `helm-core/src/mem/fault.rs` | `helm-core` |
| HartException tests | `helm-core/src/error.rs` | `helm-core` |
| ExecContext/ThreadContext split tests | `helm-core/tests/interface_split.rs` | `helm-core` (integration test) |
| Mock implementations | `helm-core/src/testing.rs` (feature-gated) | `helm-core` |

`Cargo.toml` dev-dependencies:

```toml
[dev-dependencies]
proptest = "1"
```

The `testing` module is gated behind `#[cfg(any(test, feature = "testing"))]`. Other crates (`helm-arch`, `helm-engine`) enable the `testing` feature to access the mocks without shipping test code in release builds.

---

## 2. Unit Tests: RiscvArchState

```rust
// helm-core/src/arch_state/riscv.rs

#[cfg(test)]
mod tests {
    use super::*;

    // ── IntRegs ────────────────────────────────────────────────────────────

    #[test]
    fn x0_always_reads_zero() {
        let mut state = RiscvArchState::new();
        // Write a nonzero value to x0 — must be silently discarded.
        state.write_x(0, 0xDEAD_BEEF_CAFE_0000);
        assert_eq!(state.read_x(0), 0, "x0 must always be 0");
    }

    #[test]
    fn int_reg_write_then_read() {
        let mut state = RiscvArchState::new();
        // Test all general-purpose registers x1–x31
        for idx in 1u8..32 {
            let val = (idx as u64) * 0x1_0000_0001;
            state.write_x(idx, val);
            assert_eq!(
                state.read_x(idx), val,
                "x{} write-then-read failed", idx
            );
        }
    }

    #[test]
    fn int_reg_overwrite() {
        let mut state = RiscvArchState::new();
        state.write_x(5, 0xAAAA_AAAA_AAAA_AAAA);
        state.write_x(5, 0x5555_5555_5555_5555);
        assert_eq!(state.read_x(5), 0x5555_5555_5555_5555);
    }

    #[test]
    fn int_regs_independent() {
        let mut state = RiscvArchState::new();
        // Write all registers
        for i in 1u8..32 { state.write_x(i, i as u64 * 1000); }
        // Verify none interfere with each other
        for i in 1u8..32 {
            assert_eq!(state.read_x(i), i as u64 * 1000,
                "x{} corrupted by writes to other registers", i);
        }
    }

    #[test]
    fn max_value_survives_roundtrip() {
        let mut state = RiscvArchState::new();
        state.write_x(1, u64::MAX);
        assert_eq!(state.read_x(1), u64::MAX);
    }

    // ── FloatRegs ──────────────────────────────────────────────────────────

    #[test]
    fn float_reg_write_read_bits() {
        let mut state = RiscvArchState::new();
        for idx in 0u8..32 {
            let bits = idx as u64 * 0x0001_0000_0000_0001;
            state.write_f_bits(idx, bits);
            assert_eq!(state.read_f_bits(idx), bits,
                "f{} raw bits write-then-read failed", idx);
        }
    }

    #[test]
    fn float_reg_f32_nan_boxing() {
        let mut state = RiscvArchState::new();
        let val: f32 = 1.5;
        state.write_f32(0, val);
        // Upper 32 bits must be all-ones (NaN-boxed)
        let bits = state.read_f_bits(0);
        assert_eq!(bits >> 32, 0xFFFF_FFFF,
            "NaN-box upper bits not set: {:#018x}", bits);
        // Lower 32 bits must match the f32 encoding
        assert_eq!(bits as u32, val.to_bits(),
            "NaN-box lower bits corrupted");
    }

    #[test]
    fn float_reg_f32_roundtrip() {
        let mut state = RiscvArchState::new();
        let values: &[f32] = &[0.0, 1.0, -1.0, f32::MAX, f32::MIN_POSITIVE, f32::NAN];
        for &v in values {
            state.write_f32(1, v);
            let back = state.read_f32(1);
            if v.is_nan() {
                assert!(back.is_nan(), "NaN did not survive roundtrip for f1");
            } else {
                assert_eq!(back.to_bits(), v.to_bits(),
                    "f32 roundtrip failed for {:?}", v);
            }
        }
    }

    #[test]
    fn float_reg_f64_roundtrip() {
        let mut state = RiscvArchState::new();
        let values: &[f64] = &[0.0, 1.0, -1.0, f64::MAX, f64::MIN_POSITIVE, f64::INFINITY];
        for &v in values {
            state.write_f64(2, v);
            let back = state.read_f64(2);
            assert_eq!(back.to_bits(), v.to_bits(),
                "f64 roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn float_regs_independent() {
        let mut state = RiscvArchState::new();
        for i in 0u8..32 { state.write_f_bits(i, i as u64 * 0xABCD_0000_0000 + 1); }
        for i in 0u8..32 {
            assert_eq!(state.read_f_bits(i), i as u64 * 0xABCD_0000_0000 + 1,
                "f{} corrupted", i);
        }
    }

    // ── PC ─────────────────────────────────────────────────────────────────

    #[test]
    fn pc_read_write() {
        let mut state = RiscvArchState::new();
        state.write_pc(0xFFFF_FFFF_8000_0000);
        assert_eq!(state.read_pc(), 0xFFFF_FFFF_8000_0000);
    }

    #[test]
    fn pc_initial_value() {
        let state = RiscvArchState::new();
        assert_eq!(state.read_pc(), 0x8000_0000,
            "RISC-V PC should start at conventional boot address");
    }

    // ── CSR ────────────────────────────────────────────────────────────────

    #[test]
    fn csr_read_write_mstatus() {
        let mut state = RiscvArchState::new();
        let mstatus_addr: u16 = 0x300;
        state.write_csr_raw(mstatus_addr, 0x0000_0000_0000_1800); // MPP=11 (M-mode)
        assert_eq!(state.read_csr_raw(mstatus_addr), 0x0000_0000_0000_1800);
    }

    #[test]
    fn csr_read_write_mepc() {
        let mut state = RiscvArchState::new();
        let mepc: u16 = 0x341;
        state.write_csr_raw(mepc, 0x8000_1000);
        assert_eq!(state.read_csr_raw(mepc), 0x8000_1000);
    }

    #[test]
    fn csr_undefined_reads_zero() {
        let state = RiscvArchState::new();
        // CSR 0x100 is sstatus (defined), but e.g. 0x080 is undefined
        // Undefined CSRs return 0; ISA layer raises illegal instruction.
        let undefined_csr: u16 = 0x080;
        assert_eq!(state.read_csr_raw(undefined_csr), 0,
            "Undefined CSR should read as 0");
    }

    #[test]
    fn csr_kind_for_standard_csrs() {
        let state = RiscvArchState::new();
        assert_ne!(state.csr.kind(0x300), CsrKind::Undefined, "mstatus should be defined");
        assert_ne!(state.csr.kind(0x341), CsrKind::Undefined, "mepc should be defined");
        assert_ne!(state.csr.kind(0xF14), CsrKind::Undefined, "mhartid should be defined");
    }

    // ── reset() ────────────────────────────────────────────────────────────

    #[test]
    fn reset_clears_int_regs() {
        let mut state = RiscvArchState::new();
        for i in 1u8..32 { state.write_x(i, 0xDEAD_BEEF); }
        state.reset();
        for i in 1u8..32 {
            assert_eq!(state.read_x(i), 0, "x{} not cleared by reset", i);
        }
    }

    #[test]
    fn reset_restores_pc() {
        let mut state = RiscvArchState::new();
        state.write_pc(0x1234_5678);
        state.reset();
        assert_eq!(state.read_pc(), 0x8000_0000, "PC not restored by reset");
    }

    #[test]
    fn reset_clears_csrs() {
        let mut state = RiscvArchState::new();
        state.write_csr_raw(0x300, 0xFFFF_FFFF_FFFF_FFFF);
        state.reset();
        assert_eq!(state.read_csr_raw(0x300), 0, "mstatus not cleared by reset");
    }

    #[test]
    fn reset_is_idempotent() {
        let mut state = RiscvArchState::new();
        for i in 1u8..32 { state.write_x(i, i as u64 * 99); }
        state.reset();
        state.reset(); // second reset must produce same result
        for i in 1u8..32 {
            assert_eq!(state.read_x(i), 0, "x{} not 0 after double reset", i);
        }
    }
}
```

---

## 3. Unit Tests: Aarch64ArchState

```rust
// helm-core/src/arch_state/aarch64.rs

#[cfg(test)]
mod tests {
    use super::*;

    // ── GprFile ────────────────────────────────────────────────────────────

    #[test]
    fn x31_reads_xzr_zero() {
        let state = Aarch64ArchState::new();
        assert_eq!(state.read_x(31), 0, "X31 (XZR) must always read as 0");
    }

    #[test]
    fn x31_write_discarded() {
        let mut state = Aarch64ArchState::new();
        state.write_x(31, 0xDEAD_DEAD_DEAD_DEAD);
        assert_eq!(state.read_x(31), 0, "Write to X31 (XZR) must be discarded");
    }

    #[test]
    fn gpr_write_read_all_regs() {
        let mut state = Aarch64ArchState::new();
        for i in 0u8..31 {
            let val = (i as u64 + 1) * 0x1111_1111_1111;
            state.write_x(i, val);
            assert_eq!(state.read_x(i), val, "X{} write-then-read failed", i);
        }
    }

    #[test]
    fn w_reg_write_zero_extends() {
        let mut state = Aarch64ArchState::new();
        // Write a value with bit 31 set (sign bit for i32)
        state.write_w(5, 0x8000_0001u32);
        // The X register must contain the zero-extended value, NOT sign-extended
        assert_eq!(state.read_x(5), 0x0000_0000_8000_0001u64,
            "W register write must zero-extend to X register");
    }

    #[test]
    fn w_reg_read_lower_32_bits() {
        let mut state = Aarch64ArchState::new();
        state.write_x(3, 0xFFFF_FFFF_1234_5678u64);
        assert_eq!(state.read_w(3), 0x1234_5678u32,
            "W register read must return lower 32 bits only");
    }

    // ── VRegFile ───────────────────────────────────────────────────────────

    #[test]
    fn vreg_q_write_read() {
        let mut state = Aarch64ArchState::new();
        let val: u128 = 0xDEAD_BEEF_CAFE_1234_5678_9ABC_DEF0_0000;
        state.vreg.write_q(0, val);
        assert_eq!(state.vreg.read_q(0), val);
    }

    #[test]
    fn vreg_d_write_zeroes_upper_half() {
        let mut state = Aarch64ArchState::new();
        // First, set the whole Q register to all-ones
        state.vreg.write_q(0, u128::MAX);
        // Now write the D register — upper 64 bits must be cleared
        state.vreg.write_d(0, 0xABCD_EF01_2345_6789u64);
        let q = state.vreg.read_q(0);
        assert_eq!(q >> 64, 0, "D write must zero the upper 64 bits of Q register");
        assert_eq!(q as u64, 0xABCD_EF01_2345_6789u64);
    }

    #[test]
    fn vreg_s_write_zeroes_upper_96_bits() {
        let mut state = Aarch64ArchState::new();
        state.vreg.write_q(1, u128::MAX);
        state.vreg.write_s(1, 0x1234_5678u32);
        let q = state.vreg.read_q(1);
        assert_eq!(q >> 32, 0, "S write must zero upper 96 bits");
        assert_eq!(q as u32, 0x1234_5678u32);
    }

    #[test]
    fn vreg_regs_independent() {
        let mut state = Aarch64ArchState::new();
        for i in 0u8..32 {
            state.vreg.write_q(i, (i as u128 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101);
        }
        for i in 0u8..32 {
            let expected = (i as u128 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101;
            assert_eq!(state.vreg.read_q(i), expected, "V{} corrupted", i);
        }
    }

    // ── PSTATE / NZCV ──────────────────────────────────────────────────────

    #[test]
    fn pstate_nzcv_individual_flags() {
        let mut state = Aarch64ArchState::new();
        let p = state.pstate_mut();

        p.set_nzcv(true, false, true, false);
        assert!(p.n(), "N flag should be set");
        assert!(!p.z(), "Z flag should be clear");
        assert!(p.c(), "C flag should be set");
        assert!(!p.v(), "V flag should be clear");
    }

    #[test]
    fn pstate_nzcv_all_clear() {
        let mut state = Aarch64ArchState::new();
        state.pstate_mut().set_nzcv(false, false, false, false);
        let p = state.pstate();
        assert!(!p.n() && !p.z() && !p.c() && !p.v(),
            "All NZCV flags should be clear");
    }

    #[test]
    fn pstate_nzcv_all_set() {
        let mut state = Aarch64ArchState::new();
        state.pstate_mut().set_nzcv(true, true, true, true);
        let p = state.pstate();
        assert!(p.n() && p.z() && p.c() && p.v(),
            "All NZCV flags should be set");
    }

    #[test]
    fn pstate_nzcv_bits_layout() {
        let mut state = Aarch64ArchState::new();
        // N=1, Z=0, C=0, V=0: bits[31:28] = 0b1000_0000_0000_0000_0000_0000_0000_0000
        state.pstate_mut().set_nzcv(true, false, false, false);
        let bits = state.pstate().nzcv_bits();
        assert_eq!(bits, 1 << 31, "N flag must map to bit 31");

        state.pstate_mut().set_nzcv(false, true, false, false);
        assert_eq!(state.pstate().nzcv_bits(), 1 << 30, "Z flag must map to bit 30");

        state.pstate_mut().set_nzcv(false, false, true, false);
        assert_eq!(state.pstate().nzcv_bits(), 1 << 29, "C flag must map to bit 29");

        state.pstate_mut().set_nzcv(false, false, false, true);
        assert_eq!(state.pstate().nzcv_bits(), 1 << 28, "V flag must map to bit 28");
    }

    // ── PC ─────────────────────────────────────────────────────────────────

    #[test]
    fn aarch64_pc_read_write() {
        let mut state = Aarch64ArchState::new();
        state.write_pc(0x0000_0000_4010_0000);
        assert_eq!(state.read_pc(), 0x0000_0000_4010_0000);
    }

    // ── SysRegFile ─────────────────────────────────────────────────────────

    #[test]
    fn sysreg_read_write() {
        let mut state = Aarch64ArchState::new();
        let key = sysreg_key(3, 0, 2, 0, 0); // TTBR0_EL1
        state.sysreg.write(key, 0x0000_0000_8000_1000);
        assert_eq!(state.sysreg.read(key), 0x0000_0000_8000_1000);
    }

    #[test]
    fn sysreg_undefined_reads_zero() {
        let state = Aarch64ArchState::new();
        // Some random key that is not pre-populated
        let key = sysreg_key(3, 7, 15, 15, 7);
        assert_eq!(state.sysreg.read(key), 0,
            "Undefined sysreg must read as 0");
    }

    // ── reset() ────────────────────────────────────────────────────────────

    #[test]
    fn aarch64_reset_clears_gprs() {
        let mut state = Aarch64ArchState::new();
        for i in 0u8..31 { state.write_x(i, 0xDEAD_DEAD); }
        state.reset();
        for i in 0u8..31 {
            assert_eq!(state.read_x(i), 0, "X{} not cleared by reset", i);
        }
    }

    #[test]
    fn aarch64_reset_clears_vregs() {
        let mut state = Aarch64ArchState::new();
        for i in 0u8..32 { state.vreg.write_q(i, u128::MAX); }
        state.reset();
        for i in 0u8..32 {
            assert_eq!(state.vreg.read_q(i), 0, "V{} not cleared by reset", i);
        }
    }
}
```

---

## 4. Property Tests: ArchState Roundtrip

Property tests use `proptest` to verify that write-then-read returns the same value for arbitrary inputs, with no interference between registers.

```rust
// helm-core/tests/proptest_arch_state.rs

use helm_core::arch_state::{RiscvArchState, Aarch64ArchState};
use proptest::prelude::*;

// ── RISC-V integer register roundtrip ─────────────────────────────────────

proptest! {
    /// For any register index (1–31) and any u64 value,
    /// write then read must return the same value.
    #[test]
    fn riscv_int_reg_roundtrip(
        idx in 1u8..32u8,
        val in any::<u64>(),
    ) {
        let mut state = RiscvArchState::new();
        state.write_x(idx, val);
        prop_assert_eq!(state.read_x(idx), val);
    }

    /// x0 always reads as zero regardless of what is written.
    #[test]
    fn riscv_x0_always_zero(val in any::<u64>()) {
        let mut state = RiscvArchState::new();
        state.write_x(0, val);
        prop_assert_eq!(state.read_x(0), 0u64);
    }

    /// Writing to register A does not affect register B.
    #[test]
    fn riscv_int_regs_no_aliasing(
        idx_a in 1u8..32u8,
        idx_b in 1u8..32u8,
        val_a in any::<u64>(),
        val_b in any::<u64>(),
    ) {
        prop_assume!(idx_a != idx_b);
        let mut state = RiscvArchState::new();
        state.write_x(idx_a, val_a);
        state.write_x(idx_b, val_b);
        prop_assert_eq!(state.read_x(idx_a), val_a,
            "x{} was corrupted by write to x{}", idx_a, idx_b);
    }
}

// ── RISC-V float register roundtrip ───────────────────────────────────────

proptest! {
    /// Float reg raw bits roundtrip: write bits, read back same bits.
    #[test]
    fn riscv_float_reg_bits_roundtrip(
        idx in 0u8..32u8,
        bits in any::<u64>(),
    ) {
        let mut state = RiscvArchState::new();
        state.write_f_bits(idx, bits);
        prop_assert_eq!(state.read_f_bits(idx), bits);
    }

    /// f32 roundtrip (non-NaN values): value is preserved through NaN-boxing.
    #[test]
    fn riscv_float_reg_f32_roundtrip(
        idx in 0u8..32u8,
        // Avoid signaling NaN since Rust may quiet it on float load
        raw_bits in any::<u32>().prop_filter("not sNaN", |&b| {
            let f = f32::from_bits(b);
            !f.is_nan() || (b & 0x0040_0000 != 0) // quiet NaN only
        }),
    ) {
        let mut state = RiscvArchState::new();
        let val = f32::from_bits(raw_bits);
        state.write_f32(idx, val);
        let back = state.read_f32(idx);
        if val.is_nan() {
            prop_assert!(back.is_nan());
        } else {
            prop_assert_eq!(back.to_bits(), val.to_bits());
        }
    }

    /// NaN-boxing invariant: upper 32 bits are all-ones after write_f32.
    #[test]
    fn riscv_float_reg_nan_box_preserved(
        idx in 0u8..32u8,
        raw_bits in any::<u32>(),
    ) {
        let mut state = RiscvArchState::new();
        state.write_f32(idx, f32::from_bits(raw_bits));
        let stored = state.read_f_bits(idx);
        prop_assert_eq!(stored >> 32, 0xFFFF_FFFFu64,
            "NaN-box upper bits corrupted for f{}", idx);
    }
}

// ── RISC-V PC ─────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn riscv_pc_roundtrip(val in any::<u64>()) {
        let mut state = RiscvArchState::new();
        state.write_pc(val);
        prop_assert_eq!(state.read_pc(), val);
    }
}

// ── RISC-V CSR ────────────────────────────────────────────────────────────

proptest! {
    /// Write then read any defined CSR address returns the written value.
    /// (No side effects — helm-core doesn't implement those.)
    #[test]
    fn riscv_csr_roundtrip(val in any::<u64>()) {
        let mut state = RiscvArchState::new();
        // Use mstatus (0x300) as a representative read-write CSR
        state.write_csr_raw(0x300, val);
        prop_assert_eq!(state.read_csr_raw(0x300), val);
    }
}

// ── AArch64 GPR ───────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn aarch64_gpr_roundtrip(
        idx in 0u8..31u8,
        val in any::<u64>(),
    ) {
        let mut state = Aarch64ArchState::new();
        state.write_x(idx, val);
        prop_assert_eq!(state.read_x(idx), val);
    }

    #[test]
    fn aarch64_w_reg_zero_extends(
        idx in 0u8..31u8,
        val in any::<u32>(),
    ) {
        let mut state = Aarch64ArchState::new();
        state.write_w(idx, val);
        prop_assert_eq!(state.read_x(idx), val as u64,
            "W{} write did not zero-extend to X{}", idx, idx);
    }

    #[test]
    fn aarch64_gpr_no_aliasing(
        idx_a in 0u8..31u8,
        idx_b in 0u8..31u8,
        val_a in any::<u64>(),
        val_b in any::<u64>(),
    ) {
        prop_assume!(idx_a != idx_b);
        let mut state = Aarch64ArchState::new();
        state.write_x(idx_a, val_a);
        state.write_x(idx_b, val_b);
        prop_assert_eq!(state.read_x(idx_a), val_a);
    }
}

// ── NZCV ──────────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn pstate_nzcv_roundtrip(
        n in any::<bool>(),
        z in any::<bool>(),
        c in any::<bool>(),
        v in any::<bool>(),
    ) {
        let mut state = Aarch64ArchState::new();
        state.pstate_mut().set_nzcv(n, z, c, v);
        let p = state.pstate();
        prop_assert_eq!(p.n(), n);
        prop_assert_eq!(p.z(), z);
        prop_assert_eq!(p.c(), c);
        prop_assert_eq!(p.v(), v);
    }
}
```

---

## 5. Unit Tests: MemFault Propagation

These tests verify the `MemFault` enum properties and the mock `ExecContext`'s fault propagation behavior.

```rust
// helm-core/src/mem/fault.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_fault_is_copy() {
        fn requires_copy<T: Copy>(_: T) {}
        let fault = MemFault::Unmapped { addr: 0x1234 };
        requires_copy(fault);
        // Verify we can use it after passing to requires_copy (Copy semantics)
        let _ = fault;
    }

    #[test]
    fn mem_fault_misaligned_display() {
        let fault = MemFault::Misaligned { addr: 0x1001, size: 4 };
        let msg = format!("{}", fault);
        assert!(msg.contains("misaligned"), "Display must mention misaligned: {}", msg);
        assert!(msg.contains("0x1001"), "Display must contain address: {}", msg);
    }

    #[test]
    fn mem_fault_equality() {
        let a = MemFault::Unmapped { addr: 0x5000 };
        let b = MemFault::Unmapped { addr: 0x5000 };
        let c = MemFault::Unmapped { addr: 0x6000 };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn mem_fault_variants_distinct() {
        let unmapped = MemFault::Unmapped { addr: 0x1000 };
        let misaligned = MemFault::Misaligned { addr: 0x1001, size: 4 };
        assert_ne!(unmapped, misaligned, "Different MemFault variants must not be equal");
    }

    /// Verify that a mock ExecContext propagates MemFault through read_mem.
    #[test]
    fn exec_context_read_mem_propagates_fault() {
        use crate::testing::MockExecContext;

        let mut ctx = MockExecContext::new();
        // Configure the mock to return Unmapped for any address
        ctx.set_mem_fault(MemFault::Unmapped { addr: 0xDEAD_0000 });

        let result = ctx.read_mem(0xDEAD_0000, AccessType::Load, 4);
        assert!(result.is_err(), "read_mem should propagate MemFault");
        assert_eq!(
            result.unwrap_err(),
            MemFault::Unmapped { addr: 0xDEAD_0000 }
        );
    }

    /// Verify that write_mem propagates MemFault.
    #[test]
    fn exec_context_write_mem_propagates_fault() {
        use crate::testing::MockExecContext;

        let mut ctx = MockExecContext::new();
        ctx.set_mem_fault(MemFault::AccessFault {
            addr: 0x0000_1000,
            reason: AccessFaultReason::WriteToReadOnly,
        });

        let result = ctx.write_mem(0x0000_1000, AccessType::Store, 4, 0xABCD);
        assert!(result.is_err());
        match result.unwrap_err() {
            MemFault::AccessFault { addr, reason } => {
                assert_eq!(addr, 0x0000_1000);
                assert_eq!(reason, AccessFaultReason::WriteToReadOnly);
            }
            other => panic!("Expected AccessFault, got {:?}", other),
        }
    }

    /// Verify that a successful memory read returns the correct value.
    #[test]
    fn exec_context_read_mem_success() {
        use crate::testing::MockExecContext;

        let mut ctx = MockExecContext::new();
        ctx.set_mem_value(0x8000_0000, 4, 0xDEAD_BEEF);

        let result = ctx.read_mem(0x8000_0000, AccessType::Load, 4);
        assert_eq!(result, Ok(0xDEAD_BEEF));
    }
}
```

---

## 6. Unit Tests: HartException

```rust
// helm-core/src/error.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hart_exception_is_copy() {
        fn requires_copy<T: Copy>(_: T) {}
        let exc = HartException::IllegalInstruction { pc: 0x1000, encoding: 0 };
        requires_copy(exc);
        let _ = exc;
    }

    #[test]
    fn ecall_from_umode_display() {
        let exc = HartException::EcallFromUMode { pc: 0x8000_2000 };
        let msg = format!("{}", exc);
        assert!(msg.contains("ecall") || msg.contains("U-mode"),
            "Display must mention ecall/U-mode: {}", msg);
    }

    #[test]
    fn illegal_instruction_carries_encoding() {
        let exc = HartException::IllegalInstruction { pc: 0x1234, encoding: 0xDEAD_BEEF };
        match exc {
            HartException::IllegalInstruction { pc, encoding } => {
                assert_eq!(pc, 0x1234);
                assert_eq!(encoding, 0xDEAD_BEEF);
            }
            _ => panic!("Expected IllegalInstruction"),
        }
    }

    #[test]
    fn different_exceptions_not_equal() {
        let a = HartException::Breakpoint { pc: 0x1000 };
        let b = HartException::EcallFromUMode { pc: 0x1000 };
        assert_ne!(a, b);
    }

    #[test]
    fn same_exception_same_fields_equal() {
        let a = HartException::LoadPageFault { vaddr: 0xFFFF_0000 };
        let b = HartException::LoadPageFault { vaddr: 0xFFFF_0000 };
        assert_eq!(a, b);
    }

    #[test]
    fn hart_exception_can_be_used_as_error_result() {
        fn may_fail() -> Result<(), HartException> {
            Err(HartException::SimulationHalt)
        }
        let result = may_fail();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), HartException::SimulationHalt);
    }
}
```

---

## 7. Functional Separation Tests: ExecContext vs ThreadContext

These tests verify the trait split: methods that belong to `ExecContext` are accessible from an `impl ExecContext` (hot-path), and `ThreadContext` adds the cold-path methods on top.

```rust
// helm-core/tests/interface_split.rs

use helm_core::exec_context::ExecContext;
use helm_core::thread_context::ThreadContext;
use helm_core::testing::{MockExecContext, MockThreadContext};
use helm_core::mem::{AccessType, MemFault};
use helm_core::error::HartException;
use helm_core::arch_state::Isa;

// ── Verify ExecContext provides hot-path methods ───────────────────────────

#[test]
fn exec_context_provides_int_reg_access() {
    let mut ctx = MockExecContext::new();
    ctx.write_int_reg(1, 42);
    assert_eq!(ctx.read_int_reg(1), 42);
}

#[test]
fn exec_context_provides_float_reg_access() {
    let mut ctx = MockExecContext::new();
    ctx.write_float_reg(0, 0xFFFF_FFFF_3F80_0000u64); // NaN-boxed 1.0f32
    assert_eq!(ctx.read_float_reg(0), 0xFFFF_FFFF_3F80_0000u64);
}

#[test]
fn exec_context_provides_pc_access() {
    let mut ctx = MockExecContext::new();
    ctx.write_pc(0x8000_4000);
    assert_eq!(ctx.read_pc(), 0x8000_4000);
}

#[test]
fn exec_context_provides_csr_access() {
    let mut ctx = MockExecContext::new();
    ctx.write_csr(0x300, 0x0000_1800);
    assert_eq!(ctx.read_csr(0x300), 0x0000_1800);
}

#[test]
fn exec_context_provides_mem_access() {
    let mut ctx = MockExecContext::new();
    ctx.set_mem_value(0x1000, 4, 0xABCD_EF01);
    let result = ctx.read_mem(0x1000, AccessType::Load, 4);
    assert_eq!(result, Ok(0xABCD_EF01));
}

#[test]
fn exec_context_provides_raise_exception() {
    let mut ctx = MockExecContext::new();
    let result: Result<!, HartException> = ctx.raise_exception(
        HartException::IllegalInstruction { pc: 0x1000, encoding: 0xDEAD }
    );
    assert_eq!(result.unwrap_err(),
        HartException::IllegalInstruction { pc: 0x1000, encoding: 0xDEAD });
}

/// Verify that a function requiring only ExecContext can be called with
/// a MockExecContext (static dispatch, trait bound, no vtable).
#[test]
fn exec_context_static_dispatch_works() {
    fn add_regs<C: ExecContext>(ctx: &mut C, rd: u8, rs1: u8, rs2: u8) {
        let a = ctx.read_int_reg(rs1);
        let b = ctx.read_int_reg(rs2);
        ctx.write_int_reg(rd, a.wrapping_add(b));
    }

    let mut ctx = MockExecContext::new();
    ctx.write_int_reg(1, 100);
    ctx.write_int_reg(2, 200);
    add_regs(&mut ctx, 3, 1, 2);
    assert_eq!(ctx.read_int_reg(3), 300);
}

// ── Verify ThreadContext adds cold-path methods ────────────────────────────

#[test]
fn thread_context_provides_hart_id() {
    let ctx = MockThreadContext::new_with_hart_id(7);
    assert_eq!(ctx.hart_id(), 7);
}

#[test]
fn thread_context_provides_exec_mode() {
    use helm_core::thread_context::ExecMode;
    let ctx = MockThreadContext::new();
    assert_eq!(ctx.exec_mode(), ExecMode::Functional);
}

#[test]
fn thread_context_provides_bulk_reg_read() {
    let mut ctx = MockThreadContext::new();
    for i in 1u8..32 { ctx.write_int_reg(i, i as u64 * 10); }

    let mut out = Vec::new();
    ctx.read_all_int_regs(&mut out);
    assert_eq!(out.len(), ctx.num_int_regs(), "Bulk read must fill all registers");
    // x0 = 0 (hardwired), x1 = 10, x2 = 20, ...
    assert_eq!(out[0], 0, "x0 must be 0 in bulk read");
    assert_eq!(out[1], 10, "x1 must be 10");
    assert_eq!(out[5], 50, "x5 must be 50");
}

#[test]
fn thread_context_provides_pause_resume() {
    let mut ctx = MockThreadContext::new();
    assert!(!ctx.is_paused(), "Hart should start unpaused");
    ctx.request_pause();
    assert!(ctx.is_paused(), "Hart should be paused after request_pause");
    ctx.resume();
    assert!(!ctx.is_paused(), "Hart should be running after resume");
}

#[test]
fn thread_context_provides_functional_mem_read() {
    let mut ctx = MockThreadContext::new();
    ctx.set_mem_bytes(0x2000, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let result = ctx.read_mem_functional(0x2000, 4).unwrap();
    assert_eq!(result, vec![0xDE, 0xAD, 0xBE, 0xEF]);
}

#[test]
fn thread_context_provides_attr_access() {
    use helm_core::attr::AttrValue;
    let mut ctx = MockThreadContext::new();
    // Write PC via ExecContext method
    ctx.write_pc(0x9000_0000);
    // Read via attribute system
    let attr_val = ctx.get_attr("pc").expect("pc attribute must exist");
    match attr_val {
        AttrValue::U64(v) => assert_eq!(v, 0x9000_0000),
        other => panic!("Expected U64, got {:?}", other),
    }
}

/// Verify that ThreadContext satisfies ExecContext at the type level:
/// a &mut dyn ThreadContext can be passed where &mut dyn ExecContext is needed.
///
/// This test verifies the supertrait relationship compiles correctly.
#[test]
fn thread_context_is_exec_context_supertrait() {
    fn needs_exec_ctx(ctx: &mut dyn ExecContext) {
        ctx.write_int_reg(1, 999);
    }

    let mut ctx = MockThreadContext::new();
    // Coerce &mut dyn ThreadContext to &mut dyn ExecContext via supertrait
    needs_exec_ctx(&mut ctx as &mut dyn ExecContext);
    assert_eq!(ctx.read_int_reg(1), 999,
        "ThreadContext write via ExecContext coercion must persist");
}

/// Verify that ThreadContext does NOT expose hot-path methods that should
/// NOT exist in the interface (e.g., there is no equivalent of write_csr
/// with different semantics on ThreadContext vs ExecContext).
///
/// This is a compile-time test: if the code compiles, the method set is
/// correctly unified through the supertrait.
#[test]
fn thread_context_csr_access_goes_through_exec_context() {
    let mut ctx = MockThreadContext::new();
    // CSR access via the ExecContext methods (available through supertrait)
    ctx.write_csr(0x341, 0x8000_2000); // mepc
    assert_eq!(ctx.read_csr(0x341), 0x8000_2000);
}
```

---

## 8. Mock Implementations

The `testing` module provides lightweight mock implementations of `ExecContext` and `ThreadContext` for use in tests across the workspace. These mocks live in `helm-core` (feature-gated) so that `helm-arch` tests do not need to instantiate a full `HelmEngine`.

```rust
// helm-core/src/testing.rs
// Available under #[cfg(any(test, feature = "testing"))]

use std::collections::HashMap;
use crate::exec_context::ExecContext;
use crate::thread_context::{ThreadContext, ExecMode};
use crate::arch_state::Isa;
use crate::mem::{AccessType, MemFault};
use crate::error::HartException;
use crate::attr::{AttrValue, AttrError};

/// A minimal mock ExecContext for unit tests in helm-arch and helm-core.
///
/// Backed by simple Vec/HashMap storage. No timing model, no real memory map.
/// Configure expected behavior before each test via the `set_*` methods.
pub struct MockExecContext {
    int_regs:   [u64; 32],
    float_regs: [u64; 32],
    pc:         u64,
    csrs:       HashMap<u32, u64>,
    /// If set, all read_mem/write_mem calls return this fault.
    mem_fault:  Option<MemFault>,
    /// Staged memory reads: (addr, size) → value
    mem_values: HashMap<(u64, usize), u64>,
}

impl MockExecContext {
    pub fn new() -> Self {
        Self {
            int_regs:   [0u64; 32],
            float_regs: [0u64; 32],
            pc:         0x8000_0000,
            csrs:       HashMap::new(),
            mem_fault:  None,
            mem_values: HashMap::new(),
        }
    }

    /// Configure the mock to return this fault for all memory accesses.
    pub fn set_mem_fault(&mut self, fault: MemFault) {
        self.mem_fault = Some(fault);
    }

    /// Configure a specific (addr, size) → value mapping for read_mem.
    pub fn set_mem_value(&mut self, addr: u64, size: usize, val: u64) {
        self.mem_values.insert((addr, size), val);
    }

    /// Clear the mem fault configuration (restore success behavior).
    pub fn clear_mem_fault(&mut self) {
        self.mem_fault = None;
    }
}

impl ExecContext for MockExecContext {
    fn read_int_reg(&self, idx: u8) -> u64 {
        if idx == 0 { return 0; }
        self.int_regs[idx as usize]
    }

    fn write_int_reg(&mut self, idx: u8, val: u64) {
        if idx != 0 { self.int_regs[idx as usize] = val; }
    }

    fn read_float_reg(&self, idx: u8) -> u64 { self.float_regs[idx as usize] }
    fn write_float_reg(&mut self, idx: u8, val: u64) { self.float_regs[idx as usize] = val; }

    fn read_pc(&self) -> u64 { self.pc }
    fn write_pc(&mut self, val: u64) { self.pc = val; }

    fn read_csr(&self, addr: u32) -> u64 { self.csrs.get(&addr).copied().unwrap_or(0) }
    fn write_csr(&mut self, addr: u32, val: u64) { self.csrs.insert(addr, val); }

    fn read_mem(&mut self, addr: u64, _access: AccessType, size: usize)
        -> Result<u64, MemFault>
    {
        if let Some(fault) = self.mem_fault {
            return Err(fault);
        }
        Ok(self.mem_values.get(&(addr, size)).copied().unwrap_or(0))
    }

    fn write_mem(&mut self, addr: u64, _access: AccessType, size: usize, val: u64)
        -> Result<(), MemFault>
    {
        if let Some(fault) = self.mem_fault {
            return Err(fault);
        }
        self.mem_values.insert((addr, size), val);
        Ok(())
    }

    fn raise_exception(&mut self, exc: HartException) -> Result<!, HartException> {
        Err(exc)
    }

    fn isa(&self) -> Isa { Isa::RiscV }
}

/// A mock ThreadContext that extends MockExecContext with cold-path methods.
///
/// Suitable for testing GDB stub, syscall handler, and Python binding code
/// without a full HelmEngine.
pub struct MockThreadContext {
    base:        MockExecContext,
    hart_id:     u64,
    paused:      bool,
    mem_bytes:   HashMap<u64, Vec<u8>>,
}

impl MockThreadContext {
    pub fn new() -> Self {
        Self {
            base:      MockExecContext::new(),
            hart_id:   0,
            paused:    false,
            mem_bytes: HashMap::new(),
        }
    }

    pub fn new_with_hart_id(id: u64) -> Self {
        let mut ctx = Self::new();
        ctx.hart_id = id;
        ctx
    }

    /// Configure a byte slice at a given address for read_mem_functional.
    pub fn set_mem_bytes(&mut self, addr: u64, data: &[u8]) {
        self.mem_bytes.insert(addr, data.to_vec());
    }
}

// Delegate ExecContext to the base MockExecContext
impl ExecContext for MockThreadContext {
    fn read_int_reg(&self, idx: u8) -> u64 { self.base.read_int_reg(idx) }
    fn write_int_reg(&mut self, idx: u8, val: u64) { self.base.write_int_reg(idx, val) }
    fn read_float_reg(&self, idx: u8) -> u64 { self.base.read_float_reg(idx) }
    fn write_float_reg(&mut self, idx: u8, val: u64) { self.base.write_float_reg(idx, val) }
    fn read_pc(&self) -> u64 { self.base.read_pc() }
    fn write_pc(&mut self, val: u64) { self.base.write_pc(val) }
    fn read_csr(&self, addr: u32) -> u64 { self.base.read_csr(addr) }
    fn write_csr(&mut self, addr: u32, val: u64) { self.base.write_csr(addr, val) }
    fn read_mem(&mut self, addr: u64, access: AccessType, size: usize)
        -> Result<u64, MemFault>
    { self.base.read_mem(addr, access, size) }
    fn write_mem(&mut self, addr: u64, access: AccessType, size: usize, val: u64)
        -> Result<(), MemFault>
    { self.base.write_mem(addr, access, size, val) }
    fn raise_exception(&mut self, exc: HartException) -> Result<!, HartException> {
        self.base.raise_exception(exc)
    }
    fn isa(&self) -> Isa { Isa::RiscV }
}

impl ThreadContext for MockThreadContext {
    fn hart_id(&self) -> u64 { self.hart_id }
    fn isa_id(&self) -> Isa { Isa::RiscV }
    fn exec_mode(&self) -> ExecMode { ExecMode::Functional }

    fn read_all_int_regs(&self, out: &mut Vec<u64>) {
        out.clear();
        for i in 0u8..32 { out.push(self.read_int_reg(i)); }
    }

    fn write_all_int_regs(&mut self, values: &[u64]) {
        for (i, &v) in values.iter().enumerate().take(32) {
            self.write_int_reg(i as u8, v);
        }
    }

    fn read_all_float_regs(&self, out: &mut Vec<u64>) {
        out.clear();
        for i in 0u8..32 { out.push(self.read_float_reg(i)); }
    }

    fn write_all_float_regs(&mut self, values: &[u64]) {
        for (i, &v) in values.iter().enumerate().take(32) {
            self.write_float_reg(i as u8, v);
        }
    }

    fn num_int_regs(&self) -> usize { 32 }
    fn num_float_regs(&self) -> usize { 32 }

    fn request_pause(&mut self) { self.paused = true; }
    fn resume(&mut self) { self.paused = false; }
    fn is_paused(&self) -> bool { self.paused }

    fn get_pc(&self) -> u64 { self.read_pc() }
    fn set_pc(&mut self, val: u64) { self.write_pc(val); }

    fn get_attr(&self, name: &str) -> Option<AttrValue> {
        match name {
            "pc" => Some(AttrValue::U64(self.read_pc())),
            n if n.starts_with('x') => {
                let idx: u8 = n[1..].parse().ok()?;
                Some(AttrValue::U64(self.read_int_reg(idx)))
            }
            _ => None,
        }
    }

    fn set_attr(&mut self, name: &str, val: AttrValue) -> Result<(), AttrError> {
        match name {
            "pc" => {
                if let AttrValue::U64(v) = val {
                    self.write_pc(v);
                    Ok(())
                } else {
                    Err(AttrError::TypeMismatch {
                        name: "pc",
                        expected: "U64",
                    })
                }
            }
            n if n.starts_with('x') => {
                let idx: u8 = n[1..].parse().map_err(|_| AttrError::UnknownAttr { name: "?" })?;
                if let AttrValue::U64(v) = val {
                    self.write_int_reg(idx, v);
                    Ok(())
                } else {
                    Err(AttrError::TypeMismatch { name: n, expected: "U64" })
                }
            }
            _ => Err(AttrError::UnknownAttr { name }),
        }
    }

    fn list_attrs(&self) -> Vec<&'static str> {
        // Return a minimal set for testing
        vec!["pc", "x0", "x1", "x2", "x3", "x4", "x5"]
    }

    fn read_mem_functional(&self, addr: u64, len: usize) -> Result<Vec<u8>, MemFault> {
        if let Some(bytes) = self.mem_bytes.get(&addr) {
            let slice = &bytes[..len.min(bytes.len())];
            let mut result = slice.to_vec();
            while result.len() < len { result.push(0); }
            Ok(result)
        } else {
            Ok(vec![0u8; len])
        }
    }

    fn write_mem_functional(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault> {
        self.mem_bytes.insert(addr, data.to_vec());
        Ok(())
    }
}
```

---

## 9. Test Coverage Targets

| Module | Target | Notes |
|---|---|---|
| `RiscvArchState` integer registers | 100% line coverage | x0 zero, all indices, independence, overflow |
| `RiscvArchState` float registers | 100% line coverage | NaN-boxing, f32/f64 roundtrip, raw bits |
| `RiscvArchState` PC | 100% | Read, write, initial value |
| `CsrFile` | 90% | Read, write, kind, undefined behavior |
| `Aarch64ArchState` GPR | 100% | XZR, W zero-extend, independence |
| `VRegFile` | 100% | Q, D, S, H, B views; upper-half zeroing |
| `Pstate` | 100% | All four flags, bit layout, SPSR encode/decode |
| `SysRegFile` | 80% | Read, write, undefined → 0 |
| `MemFault` enum | 100% | All variants, Copy, Display, equality |
| `HartException` enum | 100% | All variants, Copy, Display, equality |
| `ExecContext` / `ThreadContext` split | 100% | Supertrait coercion, hot-path vs cold-path division |
| `MockExecContext` | 90% | Used by helm-arch tests |
| `MockThreadContext` | 90% | Used by helm-debug and helm-engine/se tests |

Property test `proptest` runs default 256 cases per property in CI. `PROPTEST_CASES=10000` is set in nightly CI runs for deeper coverage.

Run the full test suite:

```bash
cargo test -p helm-core
cargo test -p helm-core --features testing
PROPTEST_CASES=10000 cargo test -p helm-core proptest
```
