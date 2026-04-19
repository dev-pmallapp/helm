//! System instruction emitters.
//!
//! Compiled inline:
//! - **NOP**: no-op.
//! - **MRS**: reads system register via `jit_sysreg_read` helper.
//!   NZCV is read directly from the pinned rbp register.
//! - **MSR**: writes system register via `jit_sysreg_write` helper.
//!   NZCV is written directly to the pinned rbp register.
//! - **MsrImm**: DAIFSet/DAIFClr/SPSel via arch-state helper.
//! - **WFI/WFE**: emitted as no-ops (scheduling handled by engine loop).
//! - **DC ZVA**: 8 sequential 8-byte zero-stores via the memory write helper.
//!
//! Block-terminating instructions (SVC, HVC, SMC, ERET) remain unsupported
//! (`None`) because the JIT block decoder already stops at them.

#![allow(missing_docs)]

use crate::dynasm::pinned::{load_guest_to_rax, store_rax_to_guest};
use crate::regs::{
    reg_offset, REG_DAIF, REG_JIT_ARCH_STATE, REG_JIT_MEM_WRITE, REG_JIT_RETIRED, REG_JIT_TMP0,
    REG_NZCV, REG_PC, REG_SPSEL, REG_XZR,
};
use dynasm::dynasm;
use dynasmrt::x64::Assembler;
use dynasmrt::{DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// DC ZVA block size in bytes (must match DCZID_VALUE).
const DC_ZVA_BLOCK_SIZE: u64 = 64;

/// NZCV sysreg encoding: op0=3, op1=3, CRn=4, CRm=2, op2=0
const SYSREG_NZCV: i64 = 0b11_011_0100_0010_000;

/// DCZID_EL0 sysreg encoding: op0=3, op1=3, CRn=0, CRm=0, op2=7
const SYSREG_DCZID_EL0: i64 = 0b11_011_0000_0000_111;
/// DCZID_EL0 value: BS=4 (2^(4+2)=64 byte block), DZP=0 (not prohibited).
const DCZID_VALUE: u64 = 0x0000_0004;

/// CTR_EL0 sysreg encoding: op0=3, op1=3, CRn=0, CRm=0, op2=1
const SYSREG_CTR_EL0: i64 = 0b11_011_0000_0000_001;
/// CTR_EL0 constant: 64-byte cache lines.
const CTR_VALUE: u64 = 0x8444_C004;

// -- MRS emitter --------------------------------------------------------------

/// Emit MRS Xd, <sysreg> via helper call.
///
/// NZCV is special-cased: the value is already pinned in rbp, so no helper
/// call is needed.
fn emit_mrs(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let rd_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };

    if insn.imm == SYSREG_NZCV {
        // NZCV is pinned to rbp -- just copy it to Rd.
        load_guest_to_rax(ops, REG_NZCV);
        store_rax_to_guest(ops, rd_slot);
        return Some(false);
    }

    // Inline constant registers (no arch state access needed).
    if insn.imm == SYSREG_DCZID_EL0 {
        dynasm!(ops ; mov rax, QWORD DCZID_VALUE as i64);
        store_rax_to_guest(ops, rd_slot);
        return Some(false);
    }
    if insn.imm == SYSREG_CTR_EL0 {
        dynasm!(ops ; mov rax, QWORD CTR_VALUE as i64);
        store_rax_to_guest(ops, rd_slot);
        return Some(false);
    }

    let arch_off = reg_offset(REG_JIT_ARCH_STATE);
    let encoding = insn.imm as i32;

    // Save caller-saved pinned regs around the C call.
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8   // 6 pushes (48) + 8 = 56 -> RSP 0 mod 16
    );

    // jit_sysreg_read(arch_state: *mut u8, encoding: u32) -> u64
    dynasm!(ops
        ; mov rdi, [rsp + 48]           // original rdi (flat regs)
        ; mov rdi, QWORD [rdi + arch_off]  // arch_state ptr
        ; mov esi, encoding             // encoding
        ; mov rax, QWORD crate::helpers::jit_sysreg_read as *const () as i64
        ; call rax
    );

    // Result is in rax. Stash it in a scratch slot while we restore regs.
    let stash_off = reg_offset(REG_JIT_TMP0);
    dynasm!(ops
        ; mov rcx, [rsp + 48]           // original rdi
        ; mov QWORD [rcx + stash_off], rax
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    // Load stashed result and store to Rd.
    dynasm!(ops
        ; mov rax, QWORD [rdi + stash_off]
    );
    store_rax_to_guest(ops, rd_slot);

    Some(false)
}

// -- MSR emitter --------------------------------------------------------------

/// Emit MSR <sysreg>, Xd via helper call.
///
/// NZCV is special-cased: writes directly to the pinned rbp register.
fn emit_msr(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let rd_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };

    if insn.imm == SYSREG_NZCV {
        // NZCV is pinned to rbp -- load Rd value into rbp.
        load_guest_to_rax(ops, rd_slot);
        store_rax_to_guest(ops, REG_NZCV);
        return Some(false);
    }

    let arch_off = reg_offset(REG_JIT_ARCH_STATE);
    let encoding = insn.imm as i32;

    // Load the value from Rd BEFORE saving caller-saved regs (Rd might be pinned).
    load_guest_to_rax(ops, rd_slot);
    let stash_off = reg_offset(REG_JIT_TMP0);
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);

    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );

    // jit_sysreg_write(arch_state: *mut u8, encoding: u32, value: u64) -> u64
    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdx, QWORD [rdi + stash_off]   // value
        ; mov rdi, QWORD [rdi + arch_off]    // arch_state ptr
        ; mov esi, encoding                  // encoding
        ; mov rax, QWORD crate::helpers::jit_sysreg_write as *const () as i64
        ; call rax
    );

    dynasm!(ops
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    // Sync flat-array mirrors for registers that flat_to_arch writes back.
    // DAIF and SPSel are NOT written back by flat_to_arch, but we keep the
    // flat slots up-to-date so JIT code reading them later gets the right value.
    let daif_enc: i32 = 0b11_011_0100_0010_001;
    if encoding == daif_enc {
        // Re-read DAIF from arch state and update flat slot.
        let daif_off = reg_offset(REG_DAIF);
        dynasm!(ops
            ; mov rax, QWORD [rdi + arch_off]           // arch_state ptr
            // daif field offset: read arch_state.daif (u32) and shift left 6
            // Actually, let's just re-call the read helper for DAIF.
            // Simpler: store (value >> 6) & 0xF to flat slot (matches read path).
            ; mov rax, QWORD [rdi + stash_off]
            ; shr rax, 6
            ; and eax, 0xF
            ; mov QWORD [rdi + daif_off], rax
        );
    }

    Some(false)
}

// -- MsrImm emitter -----------------------------------------------------------

/// Emit MSR immediate (DAIFSet / DAIFClr / SPSel).
///
/// These modify PSTATE fields directly via the arch state pointer.
fn emit_msr_imm(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let op1 = ((insn.imm >> 16) & 7) as u32;
    let op2 = ((insn.imm >> 5) & 7) as u32;
    let crm = ((insn.imm >> 8) & 0xF) as u32;
    let arch_off = reg_offset(REG_JIT_ARCH_STATE);

    match (op1, op2) {
        (3, 6) => {
            // DAIFSet: a.daif |= crm
            // daif is a u32 field in Aarch64ArchState. We access it through
            // the arch_state pointer at a known offset.
            // Offset of `daif` in Aarch64ArchState = after x[31]*8 + sp(8) + pc(8) + nzcv(4)
            // = 31*8 + 8 + 8 + 4 = 268
            // Actually, let me just use the helper approach for safety.
            // Write to arch_state via field access.
            emit_msr_imm_daif(ops, crm, true, arch_off);
            Some(false)
        }
        (3, 7) => {
            // DAIFClr: a.daif &= !crm
            emit_msr_imm_daif(ops, crm, false, arch_off);
            Some(false)
        }
        (0, 5) => {
            // SPSel: a.spsel = (crm & 1) != 0
            emit_msr_imm_spsel(ops, crm, arch_off);
            Some(false)
        }
        _ => Some(false), // unknown PSTATE field -- no-op (matches interpreter)
    }
}

/// Emit DAIFSet or DAIFClr through the sysreg write helper.
///
/// Reads current DAIF via the read helper, modifies the bits, writes back.
fn emit_msr_imm_daif(ops: &mut Assembler, crm: u32, set: bool, arch_off: i32) {
    let daif_off = reg_offset(REG_DAIF);

    // Read current DAIF from arch state.
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );

    let daif_encoding: i32 = 0b11_011_0100_0010_001;
    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdi, QWORD [rdi + arch_off]
        ; mov esi, daif_encoding
        ; mov rax, QWORD crate::helpers::jit_sysreg_read as *const () as i64
        ; call rax
    );

    // rax = current DAIF value (shifted left by 6 as per read_sysreg encoding).
    // Modify: set or clear the crm bits (which are in bits [9:6]).
    let crm_shifted = (crm as i32) << 6;
    if set {
        dynasm!(ops ; or eax, crm_shifted);
    } else {
        dynasm!(ops ; and eax, !(crm_shifted));
    }

    // Write back through the helper.
    // Stash modified value.
    dynasm!(ops ; mov rcx, rax);

    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdx, rcx                          // value
        ; mov rdi, QWORD [rdi + arch_off]       // arch_state ptr
        ; mov esi, daif_encoding                 // encoding
        ; mov rax, QWORD crate::helpers::jit_sysreg_write as *const () as i64
        ; call rax
    );

    dynasm!(ops
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    // Update flat DAIF slot.
    let crm_val = crm as i32;
    if set {
        dynasm!(ops
            ; mov eax, DWORD [rdi + daif_off]
            ; or eax, crm_val
            ; mov QWORD [rdi + daif_off], rax
        );
    } else {
        dynasm!(ops
            ; mov eax, DWORD [rdi + daif_off]
            ; and eax, !(crm_val)
            ; mov QWORD [rdi + daif_off], rax
        );
    }
}

/// Emit SPSel write through the arch state.
fn emit_msr_imm_spsel(ops: &mut Assembler, crm: u32, arch_off: i32) {
    let spsel_off = reg_offset(REG_SPSEL);
    let new_val = if (crm & 1) != 0 { 1i32 } else { 0i32 };

    // Write SPSel via sysreg helper is not needed -- SPSel is not a sysreg.
    // But we still need to update arch_state.spsel. The safest approach:
    // use the arch state pointer to write the field directly.
    // Since we don't know the exact field offset, use a helper pattern.
    // Actually, flat_to_arch doesn't write SPSel back, so just updating the
    // flat slot is not enough. We need to write arch_state.spsel too.
    //
    // Write via MSR encoding for SPSel doesn't exist in the sysreg helper.
    // SPSel is a PSTATE field, not a sysreg. The interpreter writes
    // a.spsel directly.
    //
    // For safety, exit the block and let the interpreter handle it.
    // SPSel changes are extremely rare (typically only during exception setup).
    //
    // Actually -- let's just handle this via the flat slot. The run_jit loop
    // rebuilds dispatch context on each iteration, and flat_to_arch doesn't
    // write SPSel back. But if we DON'T update arch_state, the SP banking
    // will be wrong.
    //
    // Simplest correct approach: call jit_sysreg_write with a pseudo-encoding,
    // or directly poke the arch_state field.
    //
    // For now, update the flat slot. The commit path doesn't write SPSel back,
    // and rebuild_aarch64_jit_flat_state reads it from arch_state. So we need
    // to update arch_state.spsel directly.
    //
    // We'll add a tiny helper for this.
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );

    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdi, QWORD [rdi + arch_off]   // arch_state ptr
        ; mov esi, new_val                   // new SPSel value
        ; mov rax, QWORD crate::helpers::jit_spsel_write as *const () as i64
        ; call rax
    );

    dynasm!(ops
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    // Update flat slot too.
    dynasm!(ops ; mov QWORD [rdi + spsel_off], new_val);
}

// -- SYS emitter --------------------------------------------------------------

/// Emit SYS instruction (TLBI, AT, IC, DC, barriers, hints) via helper.
///
/// Calls `jit_sys_exec` which dispatches to the interpreter's SYS handler,
/// correctly setting `tlb_flush_pending` for TLBI and handling AT/IC/DC.
fn emit_sys(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let arch_off = reg_offset(REG_JIT_ARCH_STATE);
    let raw = insn.raw as i64;
    let rd_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };

    // Load Xt value for TLBI VA-targeted forms.
    load_guest_to_rax(ops, rd_slot);
    let stash_off = reg_offset(REG_JIT_TMP0);
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);

    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );

    // jit_sys_exec(arch_state: *mut u8, raw: u32, xt_value: u64)
    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdx, QWORD [rdi + stash_off]   // xt_value
        ; mov rdi, QWORD [rdi + arch_off]    // arch_state ptr
        ; mov esi, raw as i32                // raw instruction
        ; mov rax, QWORD crate::helpers::jit_sys_exec as *const () as i64
        ; call rax
    );

    dynasm!(ops
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    Some(false)
}

/// Emit DC ZVA Xt: zero 64 bytes at the 64-byte-aligned address in Xt.
///
/// Emits 8 sequential 8-byte zero-writes through the runtime memory-write
/// helper (slot REG_JIT_MEM_WRITE in the flat register array), so this
/// works in both SE and FS modes.
fn emit_dc_zva(ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    let rd_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };
    let write_off = reg_offset(REG_JIT_MEM_WRITE);
    let stash_off = reg_offset(38); // scratch slot shared with ldst.rs

    // Load guest VA into rax, align to 64-byte boundary, stash in slot 38.
    load_guest_to_rax(ops, rd_slot);
    dynasm!(ops
        ; and rax, !(DC_ZVA_BLOCK_SIZE - 1) as i32
        ; mov QWORD [rdi + stash_off], rax
    );

    // 8 sequential 8-byte zero-writes.
    for i in 0..8 {
        let byte_offset = (i * 8) as i32;

        dynasm!(ops
            ; push rdi
            ; push rsi
            ; push r8
            ; push r9
            ; push r10
            ; push r11
            ; sub rsp, 8  // alignment: 6 pushes (48) + 8 = 56 => RSP 0 mod 16
        );
        dynasm!(ops
            // mem_write(ctx, addr, val, size) -> u64
            ; mov rax, QWORD [rdi + stash_off]
        );
        if byte_offset != 0 {
            dynasm!(ops ; add rax, byte_offset);
        }
        dynasm!(ops
            ; mov rsi, rax           // arg2: address
            ; mov rdi, [rsp + 40]    // arg1: mem/ctx ptr (original rsi)
            ; xor edx, edx           // arg3: value = 0
            ; mov ecx, 8i32          // arg4: size = 8

            ; mov rax, [rsp + 48]    // original rdi (flat regs)
            ; mov rax, QWORD [rax + write_off]
            ; call rax

            ; add rsp, 8
            ; pop r11
            ; pop r10
            ; pop r9
            ; pop r8
            ; pop rsi
            ; pop rdi

            ; test rax, rax
            ; jnz >fault
        );
    }

    // All writes succeeded -- skip fault exit.
    dynasm!(ops ; jmp >done);

    // Fault path: exit block with EXIT_EXCEPTION.
    dynasm!(ops
        ; fault:
    );
    let pc_off = reg_offset(crate::regs::REG_PC);
    let pc_val = insn.pc as i64;
    if i32::try_from(pc_val).is_ok() {
        dynasm!(ops ; mov QWORD [rdi + pc_off], pc_val as i32);
    } else {
        dynasm!(ops ; mov rax, QWORD pc_val ; mov QWORD [rdi + pc_off], rax);
    }
    crate::dynasm::pinned::emit_pinned_epilogue(ops);
    dynasm!(ops
        ; mov eax, crate::block::EXIT_EXCEPTION as i32
        ; ret
    );

    dynasm!(ops ; done:);
    Some(false)
}

/// Attempt to emit a system instruction.
pub fn emit_system(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) -> Option<bool> {
    match insn.opcode {
        Opcode::Nop => Some(false),
        Opcode::Mrs => emit_mrs(ops, insn),
        Opcode::Msr => emit_msr(ops, insn),
        Opcode::MsrImm => emit_msr_imm(ops, insn),
        Opcode::Wfe
        | Opcode::Dmb
        | Opcode::Dsb
        | Opcode::Isb
        | Opcode::Sev
        | Opcode::Sevl
        | Opcode::Yield
        | Opcode::Esb
        | Opcode::Sb
        | Opcode::Bti => Some(false),
        Opcode::DcZva => emit_dc_zva(ops, insn),
        // SYS covers TLBI, AT, IC, DC, barriers, and hints. TLBI sets
        // tlb_flush_pending in arch state, so we call a helper for all
        // SYS instructions to handle them correctly.
        Opcode::Sys => emit_sys(ops, insn),
        // Exception/EL-transition block terminators.
        Opcode::Svc => emit_exc_terminator(
            ops,
            insn,
            crate::helpers::jit_svc_entry as *const () as u64,
            insn_idx,
        ),
        Opcode::Hvc => emit_exc_terminator(
            ops,
            insn,
            crate::helpers::jit_hvc_entry as *const () as u64,
            insn_idx,
        ),
        Opcode::Smc => emit_exc_terminator(
            ops,
            insn,
            crate::helpers::jit_smc_entry as *const () as u64,
            insn_idx,
        ),
        Opcode::Eret => emit_eret_terminator(ops, insn, insn_idx),
        Opcode::Brk => emit_exc_terminator(
            ops,
            insn,
            crate::helpers::jit_brk_entry as *const () as u64,
            insn_idx,
        ),
        Opcode::Wfi => emit_wfi_terminator(ops, insn, insn_idx),
        _ => None,
    }
}

// -- Exception block terminators (SVC, HVC, SMC, BRK) -------------------------

/// Emit an exception-generating block terminator (SVC/HVC/SMC/BRK).
///
/// Calls the corresponding JIT helper with (arch_state, imm16), then exits
/// the block with the exit code returned by the helper. The helper updates
/// PC, SPSR, ELR, CurrentEL, etc. as needed.
fn emit_exc_terminator(
    ops: &mut Assembler,
    insn: &Instruction,
    helper_fn: u64,
    insn_idx: u32,
) -> Option<bool> {
    let arch_off = reg_offset(REG_JIT_ARCH_STATE);
    let pc_off = reg_offset(REG_PC);
    let retired_off = reg_offset(REG_JIT_RETIRED);
    let imm16 = (insn.imm as u32) & 0xFFFF;
    let insn_pc = insn.pc as i64;
    let retired_count = insn_idx + 1;

    // Write current PC to flat array so the helper sees the faulting/call PC.
    if let Ok(pc32) = i32::try_from(insn_pc) {
        dynasm!(ops ; mov QWORD [rdi + pc_off], pc32);
    } else {
        dynasm!(ops ; mov rax, QWORD insn_pc ; mov QWORD [rdi + pc_off], rax);
    }

    // Flush pinned registers to the flat array before the helper call,
    // so commit_aarch64_jit_state gets the right values.
    crate::dynasm::pinned::emit_pinned_epilogue(ops);
    // Accumulate retired count (includes this instruction).
    dynasm!(ops ; add QWORD [rdi + retired_off], retired_count as i32);

    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8   // 6 pushes (48) + 8 = 56 -> RSP 0 mod 16
    );

    // helper(arch_state: *mut u8, imm16: u32) -> u64
    dynasm!(ops
        ; mov rdi, [rsp + 48]            // original rdi (flat regs)
        ; mov rdi, QWORD [rdi + arch_off]  // arch_state ptr
        ; mov esi, imm16 as i32          // imm16
        ; mov rax, QWORD helper_fn as i64
        ; call rax
    );

    // rax = exit code from helper. Stash it.
    let stash_off = reg_offset(REG_JIT_TMP0);
    dynasm!(ops
        ; mov rcx, [rsp + 48]            // original rdi
        ; mov QWORD [rcx + stash_off], rax
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    // Return the exit code from the helper as the block exit code.
    dynasm!(ops
        ; mov rax, QWORD [rdi + stash_off]
        ; ret
    );

    Some(true) // block terminator
}

/// Emit ERET block terminator.
///
/// Calls `jit_eret_entry(arch_state)` which performs exception_return,
/// then exits the block with EXIT_EL_CHANGE.
fn emit_eret_terminator(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) -> Option<bool> {
    let arch_off = reg_offset(REG_JIT_ARCH_STATE);
    let pc_off = reg_offset(REG_PC);
    let insn_pc = insn.pc as i64;
    let retired_off = reg_offset(REG_JIT_RETIRED);
    let retired_count = insn_idx + 1;

    // Write current PC to flat array.
    if let Ok(pc32) = i32::try_from(insn_pc) {
        dynasm!(ops ; mov QWORD [rdi + pc_off], pc32);
    } else {
        dynasm!(ops ; mov rax, QWORD insn_pc ; mov QWORD [rdi + pc_off], rax);
    }

    // Flush pinned registers.
    crate::dynasm::pinned::emit_pinned_epilogue(ops);
    dynasm!(ops ; add QWORD [rdi + retired_off], retired_count as i32);

    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );

    // jit_eret_entry(arch_state: *mut u8) -> u64
    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdi, QWORD [rdi + arch_off]
        ; mov rax, QWORD crate::helpers::jit_eret_entry as *const () as i64
        ; call rax
    );

    let stash_off = reg_offset(REG_JIT_TMP0);
    dynasm!(ops
        ; mov rcx, [rsp + 48]
        ; mov QWORD [rcx + stash_off], rax
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    dynasm!(ops
        ; mov rax, QWORD [rdi + stash_off]
        ; ret
    );

    Some(true)
}

/// Emit WFI block terminator.
///
/// In FS mode: calls `jit_wfi_entry` which returns EXIT_WFI.
/// In SE mode: the helper returns EXIT_END_OF_BLOCK (no-op).
fn emit_wfi_terminator(ops: &mut Assembler, insn: &Instruction, insn_idx: u32) -> Option<bool> {
    let arch_off = reg_offset(REG_JIT_ARCH_STATE);
    let pc_off = reg_offset(REG_PC);
    let next_pc = insn.pc.wrapping_add(4) as i64;
    let retired_off = reg_offset(REG_JIT_RETIRED);
    let retired_count = insn_idx + 1;

    // Write next PC (WFI is not faulting; resume at PC+4).
    if let Ok(pc32) = i32::try_from(next_pc) {
        dynasm!(ops ; mov QWORD [rdi + pc_off], pc32);
    } else {
        dynasm!(ops ; mov rax, QWORD next_pc ; mov QWORD [rdi + pc_off], rax);
    }

    crate::dynasm::pinned::emit_pinned_epilogue(ops);
    dynasm!(ops ; add QWORD [rdi + retired_off], retired_count as i32);

    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
    );

    dynasm!(ops
        ; mov rdi, [rsp + 48]
        ; mov rdi, QWORD [rdi + arch_off]
        ; mov rax, QWORD crate::helpers::jit_wfi_entry as *const () as i64
        ; call rax
    );

    let stash_off = reg_offset(REG_JIT_TMP0);
    dynasm!(ops
        ; mov rcx, [rsp + 48]
        ; mov QWORD [rcx + stash_off], rax
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
    );

    dynasm!(ops
        ; mov rax, QWORD [rdi + stash_off]
        ; ret
    );

    Some(true)
}
