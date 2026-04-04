//! AArch64 execute — ldst group.
#![allow(unused_imports, unused_variables)]
use super::helpers::*;
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::exception;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};

#[allow(clippy::too_many_lines)]
pub fn exec_ldst(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    let pc_written = false;
    match insn.opcode {
        // ── Load/Store ───────────────────────────────────────────────────────
        Ldr | Ldrb | Ldrh | Ldrsb | Ldrsh | Ldrsw | Ldur | Ldurb | Ldurh | Ldursb | Ldursh
        | Ldursw => {
            let base = a.read_xsp(insn.rn);
            let ea = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let (sz, signed) = ldst_size(insn.opcode);
            // For non-signed loads, use insn.sf to determine access size:
            // sf=false means 32-bit (W register) access = 4 bytes max
            let sz = if !signed && !insn.sf && sz == 8 {
                4
            } else {
                sz
            };
            let raw_val = mem
                .read(ea, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, ea))?;
            let val = if signed {
                let extended = sign_extend(raw_val, sz);
                // For W-target signed loads (sf=false), mask to 32 bits
                if insn.sf {
                    extended
                } else {
                    extended & 0xFFFF_FFFF
                }
            } else {
                raw_val
            };
            a.write_x(insn.rd, val);
            writeback_post(a, insn, ea);
        }
        Str | Strb | Strh | Stur | Sturb | Sturh => {
            let base = a.read_xsp(insn.rn);
            let ea = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let (sz, _) = ldst_size(insn.opcode);
            // For W-register stores (sf=false), use 4 bytes max
            let sz = if !insn.sf && sz == 8 { 4 } else { sz };
            let val = a.read_x(insn.rd);
            mem.write(ea, sz, val, AccessType::Store)
                .map_err(|e| mem_fault_store(e, ea))?;
            writeback_post(a, insn, ea);
        }
        Ldp => {
            let base = a.read_xsp(insn.rn);
            let ea = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let sz = if insn.sf { 8 } else { 4 };
            let v1 = mem
                .read(ea, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, ea))?;
            let v2 = mem
                .read(ea + sz as u64, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, ea))?;
            let (v1, v2) = if insn.signed_load {
                (sign_extend(v1, sz), sign_extend(v2, sz))
            } else {
                (v1, v2)
            };
            a.write_x(insn.rd, v1);
            a.write_x(insn.pair_second, v2);
            writeback_post(a, insn, ea);
        }
        Stp => {
            let base = a.read_xsp(insn.rn);
            let ea = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let sz = if insn.sf { 8 } else { 4 };
            let v1 = a.read_x(insn.rd);
            let v2 = a.read_x(insn.pair_second);
            mem.write(ea, sz, v1, AccessType::Store)
                .map_err(|e| mem_fault_store(e, ea))?;
            mem.write(ea + sz as u64, sz, v2, AccessType::Store)
                .map_err(|e| mem_fault_store(e, ea))?;
            writeback_post(a, insn, ea);
        }

        // ── Exclusive load/store with monitor (LL/SC semantics) ─────────────
        Ldxr | Ldaxr => {
            let base = a.read_xsp(insn.rn);
            let sz = 1usize << insn.size; // size=0->1B, 1->2B, 2->4B, 3->8B
            let val = mem
                .read(base, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, base))?;
            a.write_x(insn.rd, val);
            a.exclusive_addr = Some(base);
            a.exclusive_val = val;
        }
        Stxr | Stlxr => {
            let base = a.read_xsp(insn.rn);
            let sz = 1usize << insn.size;
            let mask = if sz < 8 {
                (1u64 << (sz * 8)) - 1
            } else {
                u64::MAX
            };
            // Re-read to check if another CPU modified the location.
            let current = mem
                .read(base, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, base))?;
            if a.exclusive_addr == Some(base) && (current & mask) == (a.exclusive_val & mask) {
                let val = a.read_x(insn.rd) & mask;
                mem.write(base, sz, val, AccessType::Atomic)
                    .map_err(|e| mem_fault_store(e, base))?;
                a.write_x(insn.rm, 0); // success
            } else {
                a.write_x(insn.rm, 1); // failure — retry loop
            }
            a.exclusive_addr = None;
        }
        Ldxp | Ldaxp => {
            let base = a.read_xsp(insn.rn);
            let sz = if insn.sf { 8 } else { 4 };
            let v0 = mem
                .read(base, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, base))?;
            let v1 = mem
                .read(base + sz as u64, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, base + sz as u64))?;
            a.write_x(insn.rd, v0);
            a.write_x(insn.pair_second, v1);
            a.exclusive_addr = Some(base);
            a.exclusive_val = v0;
        }
        Stxp | Stlxp => {
            let base = a.read_xsp(insn.rn);
            let sz = if insn.sf { 8 } else { 4 };
            let mask = if sz < 8 {
                (1u64 << (sz * 8)) - 1
            } else {
                u64::MAX
            };
            let current = mem
                .read(base, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, base))?;
            if a.exclusive_addr == Some(base) && (current & mask) == (a.exclusive_val & mask) {
                let v0 = a.read_x(insn.rd);
                let v1 = a.read_x(insn.pair_second);
                mem.write(base, sz, v0, AccessType::Atomic)
                    .map_err(|e| mem_fault_store(e, base))?;
                mem.write(base + sz as u64, sz, v1, AccessType::Atomic)
                    .map_err(|e| mem_fault_store(e, base + sz as u64))?;
                a.write_x(insn.rm, 0); // success
            } else {
                a.write_x(insn.rm, 1); // failure
            }
            a.exclusive_addr = None;
        }
        Clrex => {
            a.exclusive_addr = None;
        }

        // ── Load literal (PC-relative) ─────────────────────────────────────
        LdrLit => {
            let addr = a.pc.wrapping_add(insn.imm as u64);
            let size = if insn.sf { 8 } else { 4 };
            let val = mem
                .read(addr, size, AccessType::Load)
                .map_err(|e| mem_fault_load(e, addr))?;
            a.write_x(insn.rd, val);
        }
        LdrswLit => {
            let addr = a.pc.wrapping_add(insn.imm as u64);
            let val = mem
                .read(addr, 4, AccessType::Load)
                .map_err(|e| mem_fault_load(e, addr))?;
            a.write_x(insn.rd, val as i32 as i64 as u64);
        }

        // ── SIMD/FP load/store ────────────────────────────────────────────
        LdrSimd => {
            let size_bytes = match insn.ftype {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                _ => 16,
            };
            let base = a.read_xsp(insn.rn);
            let load_addr = if insn.imm == i64::MIN {
                // Register offset: rm + extend/shift
                let rm_val = a.read_x(insn.rm);
                let shift = insn.extend_amt;
                let offset = match insn.extend_type {
                    0b010 => (rm_val as u32 as u64) << shift,        // UXTW
                    0b011 => rm_val << shift,                        // LSL
                    0b110 => (rm_val as i32 as i64 as u64) << shift, // SXTW
                    0b111 => rm_val << shift,                        // SXTX
                    _ => rm_val,
                };
                base.wrapping_add(offset)
            } else {
                let eff = base.wrapping_add(insn.imm as u64);
                if insn.pre_index {
                    a.write_xsp(insn.rn, eff);
                }
                if insn.pre_index || !insn.post_index {
                    eff
                } else {
                    base
                }
            };
            if size_bytes <= 8 {
                let val = mem
                    .read(load_addr, size_bytes, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, load_addr))?;
                a.v[insn.rd as usize] = val as u128;
            } else {
                let lo = mem
                    .read(load_addr, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, load_addr))?;
                let hi = mem
                    .read(load_addr + 8, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, load_addr + 8))?;
                a.v[insn.rd as usize] = (hi as u128) << 64 | lo as u128;
            }
            if insn.imm != i64::MIN && insn.post_index {
                let eff = base.wrapping_add(insn.imm as u64);
                a.write_xsp(insn.rn, eff);
            }
        }
        StrSimd => {
            let size_bytes = match insn.ftype {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                _ => 16,
            };
            let base = a.read_xsp(insn.rn);
            let store_addr = if insn.imm == i64::MIN {
                // Register offset
                let rm_val = a.read_x(insn.rm);
                let shift = insn.extend_amt;
                let offset = match insn.extend_type {
                    0b010 => (rm_val as u32 as u64) << shift,
                    0b011 => rm_val << shift,
                    0b110 => (rm_val as i32 as i64 as u64) << shift,
                    0b111 => rm_val << shift,
                    _ => rm_val,
                };
                base.wrapping_add(offset)
            } else {
                let eff = base.wrapping_add(insn.imm as u64);
                if insn.pre_index {
                    a.write_xsp(insn.rn, eff);
                }
                if insn.pre_index || !insn.post_index {
                    eff
                } else {
                    base
                }
            };
            let val = a.v[insn.rd as usize];
            if size_bytes <= 8 {
                mem.write(store_addr, size_bytes, val as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, store_addr))?;
            } else {
                mem.write(store_addr, 8, val as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, store_addr))?;
                mem.write(store_addr + 8, 8, (val >> 64) as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, store_addr + 8))?;
            }
            if insn.imm != i64::MIN && insn.post_index {
                let eff = base.wrapping_add(insn.imm as u64);
                a.write_xsp(insn.rn, eff);
            }
        }
        LdurSimd => {
            let addr = a.read_xsp(insn.rn).wrapping_add(insn.imm as u64);
            let size_bytes = match insn.ftype {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                _ => 16,
            };
            if size_bytes <= 8 {
                let val = mem
                    .read(addr, size_bytes, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, addr))?;
                a.v[insn.rd as usize] = val as u128;
            } else {
                let lo = mem
                    .read(addr, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, addr))?;
                let hi = mem
                    .read(addr + 8, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, addr + 8))?;
                a.v[insn.rd as usize] = (hi as u128) << 64 | lo as u128;
            }
        }
        SturSimd => {
            let addr = a.read_xsp(insn.rn).wrapping_add(insn.imm as u64);
            let size_bytes = match insn.ftype {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                _ => 16,
            };
            let val = a.v[insn.rd as usize];
            if size_bytes <= 8 {
                mem.write(addr, size_bytes, val as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, addr))?;
            } else {
                mem.write(addr, 8, val as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, addr))?;
                mem.write(addr + 8, 8, (val >> 64) as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, addr + 8))?;
            }
        }
        LdpSimd => {
            let base = a.read_xsp(insn.rn);
            let addr = base.wrapping_add(insn.imm as u64);
            let eff = if insn.post_index { base } else { addr };
            let sz = match insn.ftype {
                0 => 4usize,
                1 => 8,
                _ => 16,
            }; // S=4,D=8,Q=16
            if sz <= 8 {
                let v1 = mem
                    .read(eff, sz, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, eff))?;
                let v2 = mem
                    .read(eff + sz as u64, sz, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, eff + sz as u64))?;
                a.v[insn.rd as usize] = v1 as u128;
                a.v[insn.pair_second as usize] = v2 as u128;
            } else {
                // Q-regs
                let lo1 = mem
                    .read(eff, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, eff))?;
                let hi1 = mem
                    .read(eff + 8, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, eff + 8))?;
                a.v[insn.rd as usize] = (hi1 as u128) << 64 | lo1 as u128;
                let lo2 = mem
                    .read(eff + 16, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, eff + 16))?;
                let hi2 = mem
                    .read(eff + 24, 8, AccessType::Load)
                    .map_err(|e| mem_fault_load(e, eff + 24))?;
                a.v[insn.pair_second as usize] = (hi2 as u128) << 64 | lo2 as u128;
            }
            if insn.pre_index || insn.post_index {
                let wb = if insn.post_index { addr } else { eff };
                a.write_xsp(insn.rn, wb);
            }
        }
        StpSimd => {
            let base = a.read_xsp(insn.rn);
            let addr = base.wrapping_add(insn.imm as u64);
            let eff = if insn.post_index { base } else { addr };
            let sz = match insn.ftype {
                0 => 4usize,
                1 => 8,
                _ => 16,
            };
            if sz <= 8 {
                mem.write(eff, sz, a.v[insn.rd as usize] as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, eff))?;
                mem.write(
                    eff + sz as u64,
                    sz,
                    a.v[insn.pair_second as usize] as u64,
                    AccessType::Store,
                )
                .map_err(|e| mem_fault_store(e, eff + sz as u64))?;
            } else {
                let v1 = a.v[insn.rd as usize];
                mem.write(eff, 8, v1 as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, eff))?;
                mem.write(eff + 8, 8, (v1 >> 64) as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, eff + 8))?;
                let v2 = a.v[insn.pair_second as usize];
                mem.write(eff + 16, 8, v2 as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, eff + 16))?;
                mem.write(eff + 24, 8, (v2 >> 64) as u64, AccessType::Store)
                    .map_err(|e| mem_fault_store(e, eff + 24))?;
            }
            if insn.pre_index || insn.post_index {
                let wb = if insn.post_index { addr } else { eff };
                a.write_xsp(insn.rn, wb);
            }
        }

        // ── LDAR / STLR ──────────────────────────────────────────────────
        Ldar => {
            let addr = a.read_xsp(insn.rn);
            let sz = 1 << insn.size;
            let val = mem
                .read(addr, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, addr))?;
            a.write_x(insn.rd, val);
        }
        Stlr => {
            let addr = a.read_xsp(insn.rn);
            let sz = 1 << insn.size;
            let val = a.read_x(insn.rd);
            mem.write(addr, sz, val, AccessType::Store)
                .map_err(|e| mem_fault_store(e, addr))?;
        }

        // ── LSE atomics ──────────────────────────────────────────────────
        Ldadd | Ldclr | Ldeor | Ldset | LdSmax | LdSmin | LdUmax | LdUmin => {
            let addr = a.read_xsp(insn.rn);
            let sz = 1usize << insn.size;
            let mask = if sz < 8 {
                (1u64 << (sz * 8)) - 1
            } else {
                u64::MAX
            };
            let old = mem
                .read(addr, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, addr))?;
            let rs = a.read_x(insn.rm) & mask;
            let old_m = old & mask;
            let new_val = match insn.opcode {
                Ldadd => old_m.wrapping_add(rs),
                Ldclr => old_m & !rs,
                Ldeor => old_m ^ rs,
                Ldset => old_m | rs,
                // Signed comparisons: sign-extend both to i64 then compare
                LdSmax => {
                    let bits = sz * 8;
                    let a_s = sext_mask(old_m, bits);
                    let b_s = sext_mask(rs, bits);
                    if a_s >= b_s {
                        old_m
                    } else {
                        rs
                    }
                }
                LdSmin => {
                    let bits = sz * 8;
                    let a_s = sext_mask(old_m, bits);
                    let b_s = sext_mask(rs, bits);
                    if a_s <= b_s {
                        old_m
                    } else {
                        rs
                    }
                }
                LdUmax => {
                    if old_m >= rs {
                        old_m
                    } else {
                        rs
                    }
                }
                LdUmin => {
                    if old_m <= rs {
                        old_m
                    } else {
                        rs
                    }
                }
                _ => unreachable!(),
            };
            mem.write(addr, sz, new_val & mask, AccessType::Atomic)
                .map_err(|e| mem_fault_store(e, addr))?;
            a.write_x(insn.rd, old_m);
        }
        Swp => {
            let addr = a.read_xsp(insn.rn);
            let sz = 1usize << insn.size;
            let old = mem
                .read(addr, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, addr))?;
            let mask = if sz < 8 {
                (1u64 << (sz * 8)) - 1
            } else {
                u64::MAX
            };
            mem.write(addr, sz, a.read_x(insn.rm) & mask, AccessType::Atomic)
                .map_err(|e| mem_fault_store(e, addr))?;
            a.write_x(insn.rd, old & mask);
        }
        Cas => {
            let addr = a.read_xsp(insn.rn);
            let sz = 1usize << insn.size;
            let mask = if sz < 8 {
                (1u64 << (sz * 8)) - 1
            } else {
                u64::MAX
            };
            let old = mem
                .read(addr, sz, AccessType::Atomic)
                .map_err(|e| mem_fault_load(e, addr))?;
            let expect = a.read_x(insn.rd) & mask;
            if (old & mask) == expect {
                mem.write(addr, sz, a.read_x(insn.rm) & mask, AccessType::Atomic)
                    .map_err(|e| mem_fault_store(e, addr))?;
            }
            a.write_x(insn.rd, old & mask);
        }
        Casp => { /* pair CAS — stub, return current value */ }

        // ── PRFM (prefetch → NOP) ────────────────────────────────────────
        Prfm => {}

        // ── DC ZVA (data cache zero by VA) ───────────────────────────────
        DcZva => {
            let va = a.read_x(insn.rd);
            let line = va & !63u64; // assume 64-byte cache line
            for off in (0..64).step_by(8) {
                mem.write(line + off, 8, 0, AccessType::Store).ok();
            }
        }

        // ── LRCPC (v8.3): LDAPR / LDAPRH / LDAPRB ───────────────────────────
        // Single-core functional: same semantics as LDAR (load with acquire ordering).
        Ldapr | Ldaprh | Ldaprb => {
            let addr = a.read_xsp(insn.rn);
            let sz = match insn.opcode {
                Opcode::Ldaprb => 1,
                Opcode::Ldaprh => 2,
                _ => {
                    if insn.sf {
                        8
                    } else {
                        4
                    }
                }
            };
            let val = mem
                .read(addr, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, addr))?;
            a.write_x(insn.rd, val);
        }

        // ── RCPC2 (v8.4): LDAPUR / STLUR with unscaled signed immediate ──────
        LdapurB | LdapurH | Ldapur => {
            let base = a.read_xsp(insn.rn);
            let ea = base.wrapping_add(insn.imm as u64);
            let sz = match insn.opcode {
                Opcode::LdapurB => 1,
                Opcode::LdapurH => 2,
                _ => {
                    if insn.sf {
                        8
                    } else {
                        4
                    }
                }
            };
            let val = mem
                .read(ea, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, ea))?;
            a.write_x(insn.rd, val);
        }
        StlurB | StlurH | Stlur => {
            let base = a.read_xsp(insn.rn);
            let ea = base.wrapping_add(insn.imm as u64);
            let sz = match insn.opcode {
                Opcode::StlurB => 1,
                Opcode::StlurH => 2,
                _ => {
                    if insn.sf {
                        8
                    } else {
                        4
                    }
                }
            };
            let val = a.read_x(insn.rd);
            mem.write(ea, sz, val, AccessType::Store)
                .map_err(|e| mem_fault_store(e, ea))?;
        }

        _ => unreachable!("wrong dispatch to ldst"),
    }
    Ok(pc_written)
}
