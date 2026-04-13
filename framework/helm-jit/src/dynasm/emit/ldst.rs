//! Load/store instruction emitters (AArch64 → x86-64).
//!
//! Loads and stores call extern "C" helper functions (`jit_mem_read`,
//! `jit_mem_write`) to access guest memory. The helpers receive the FlatMem
//! pointer from `rsi`.
//!
//! ## Calling convention for memory helpers
//!
//! ```text
//! jit_mem_read(mem: *mut u8, addr: u64, size: u32, out: *mut u64) -> u64
//! jit_mem_write(mem: *mut u8, addr: u64, val: u64, size: u32) -> u64
//! ```
//!
//! Both return 0 on success, 1 on fault. On fault, the block exits with
//! `EXIT_EXCEPTION`.

#![allow(missing_docs)]
#![allow(clippy::similar_names)]

use crate::block::EXIT_EXCEPTION;
use crate::dynasm::pinned::{
    emit_pinned_epilogue, load_guest_to_rax, load_guest_to_rcx, store_eax_to_guest_32,
    store_rax_to_guest,
};
use crate::regs::{
    reg_offset, REG_JIT_MEM_READ, REG_JIT_MEM_WRITE, REG_JIT_SE_TLB, REG_JIT_TMP0, REG_PC,
    REG_SP, REG_XZR,
};
// REG_SP and REG_XZR are used to resolve reg 31 in base/data slots.
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// Add a 64-bit immediate to `rax`.
///
/// x86-64 `add reg64, imm` only supports imm32 in the encoding. For larger
/// immediates we load into `rcx` and add register-to-register. For values
/// that fit in i32 we use the compact encoding.
#[inline]
fn emit_add_rax_imm64(ops: &mut Assembler, imm: i64) {
    if i32::try_from(imm).is_ok() {
        dynasm!(ops ; add rax, imm as i32);
    } else {
        dynasm!(ops
            ; mov rcx, QWORD imm
            ; add rax, rcx
        );
    }
}

/// Access size in bytes from the insn.size field or opcode.
fn access_size(insn: &Instruction) -> u32 {
    match insn.opcode {
        Opcode::Ldrb | Opcode::Strb | Opcode::Ldrsb | Opcode::Ldurb | Opcode::Sturb => 1,
        Opcode::Ldrh | Opcode::Strh | Opcode::Ldrsh | Opcode::Ldurh | Opcode::Sturh => 2,
        Opcode::Ldrsw | Opcode::Ldursw => 4,
        Opcode::Ldr | Opcode::Str | Opcode::Ldur | Opcode::Stur => {
            if insn.sf {
                8
            } else {
                4
            }
        }
        _ => match insn.size {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 8,
        },
    }
}

#[inline]
fn is_reg_offset(insn: &Instruction) -> bool {
    insn.extend_type != 0 || (insn.rm != 0 && !insn.post_index)
}

fn emit_apply_ldst_extend(ops: &mut Assembler, extend_type: u32, extend_amt: u32) {
    match extend_type {
        0 => dynasm!(ops ; movzx ecx, cl),
        1 => dynasm!(ops ; movzx ecx, cx),
        2 => dynasm!(ops ; mov ecx, ecx),
        3 => {}
        4 => dynasm!(ops ; movsx rcx, cl),
        5 => dynasm!(ops ; movsx rcx, cx),
        6 => dynasm!(ops ; movsxd rcx, ecx),
        7 => {}
        _ => unreachable!(),
    }

    if extend_amt != 0 {
        let amt = extend_amt as i8;
        dynasm!(ops ; shl rcx, amt);
    }
}

fn emit_compute_address(ops: &mut Assembler, insn: &Instruction, base_slot: usize) {
    load_guest_to_rax(ops, base_slot);

    if is_reg_offset(insn) {
        let rm_slot = if insn.rm == 31 {
            REG_XZR
        } else {
            insn.rm as usize
        };
        load_guest_to_rcx(ops, rm_slot);
        emit_apply_ldst_extend(ops, insn.extend_type, insn.extend_amt);
        dynasm!(ops ; add rax, rcx);
    } else if insn.pre_index {
        emit_add_rax_imm64(ops, insn.imm);
        store_rax_to_guest(ops, base_slot);
    } else if !insn.post_index {
        emit_add_rax_imm64(ops, insn.imm);
    }
}

/// Emit a call to the runtime-selected memory-read helper.
///
/// **Calling convention (internal):**
/// - Before calling: `rax` = effective guest address.
/// - After returning (success): `rax` = loaded value.
///
/// Saves/restores rdi, rsi, and caller-saved pinned regs (r8–r11 = X0–X3)
/// around the C call so that pinned guest register state is preserved.
///
/// Stack layout during C call (total = 48 bytes, 16-byte aligned):
///   [rsp+40..47]  = rdi (saved)
///   [rsp+32..39]  = rsi (saved)
///   [rsp+24..31]  = r11 (saved)
///   [rsp+16..23]  = r10 (saved)
///   [rsp+8..15]   = r9  (saved)
///   [rsp+0..7]    = r8  (saved) / also used as output buffer for jit_mem_read
///   [rsp-8..-1]   = output slot (allocated before call via sub rsp,8)
///
/// Actually: push 5 callee values (rdi, rsi, r8–r11 → 6 pushes = 48 bytes).
/// Then sub rsp, 8 for the output buffer = 56 bytes total before call.
/// That keeps RSP 16-byte aligned after 6 pushes + 8 sub (6*8+8=56, 56%16=8 → need 1 more: use sub 16).
fn emit_mem_read(ops: &mut Assembler, size: u32) {
    let read_off = reg_offset(REG_JIT_MEM_READ);

    // rax = effective address on entry.
    // Save everything that the C call will clobber.
    dynasm!(ops
        ; push rdi            // save regs ptr
        ; push rsi            // save mem ptr
        ; push r8             // save pinned X0
        ; push r9             // save pinned X1
        ; push r10            // save pinned X2
        ; push r11            // save pinned X3
        ; sub rsp, 8          // output buffer (8B); RSP now 0 mod 16

        // mem_read(mem_or_ctx: *mut u8, addr: u64, size: u32, out: *mut u64) -> u64
        // arg1 = rdi = mem ptr (was rsi before we pushed rdi)
        // arg2 = rsi = address (was rax)
        // arg3 = rdx = size
        // arg4 = rcx = output pointer
        ; mov rsi, rax        // arg2: address (was rax)
    );
    dynasm!(ops
        ; mov rdi, [rsp + 40] // arg1: mem ptr (original rsi)
        ; mov edx, size as i32 // arg3: size
        ; lea rcx, [rsp]      // arg4: output pointer (&[rsp+0])
        ; mov rax, [rsp + 48] // original flat-reg pointer (rdi)
        ; mov rax, QWORD [rax + read_off]
        ; call rax

        // Grab output before we deallocate the stack slot.
        ; mov rcx, [rsp]
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi

        ; test rax, rax
        ; jnz >fault

        ; mov rax, rcx        // loaded value → rax
    );
}

/// Emit a call to the runtime-selected memory-write helper.
///
/// **Calling convention (internal):**
/// - Before calling: `rax` = effective guest address, `rcx` = value to write.
///
/// Saves/restores rdi, rsi, and caller-saved pinned regs around the C call.
fn emit_mem_write(ops: &mut Assembler, size: u32) {
    let write_off = reg_offset(REG_JIT_MEM_WRITE);

    // rax = effective address, rcx = value to write on entry.
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8          // alignment padding; RSP now 0 mod 16
    );
    dynasm!(ops
        // mem_write(mem_or_ctx: *mut u8, addr: u64, val: u64, size: u32) -> u64
        // Save addr and val before overwriting their registers.
        ; mov rdx, rcx        // arg3: value (was rcx)
        ; mov rsi, rax        // arg2: address (was rax)
        ; mov rdi, [rsp + 40] // arg1: mem ptr (original rsi)
        ; mov ecx, size as i32 // arg4: size

        ; mov rax, [rsp + 48] // original flat-reg pointer (rdi)
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

/// Emit an inline TLB load (SE mode).
///
/// On TLB hit (~99% of SE accesses): 8 instructions, no function call.
/// On TLB miss: falls back to `jit_se_tlb_fill_and_read` which fills the TLB
/// entry and performs the read.
///
/// Input: `rax` = guest effective address.
/// Output: `rax` = loaded value (zero-extended to 64 bits).
/// Clobbers: `rcx`, `rdx`, `r9` (all caller-saved).
fn emit_tlb_load(ops: &mut Assembler, size: u32) {
    let tlb_off = reg_offset(REG_JIT_SE_TLB); // slot 44 → offset into flat array
    let tmp_off = reg_offset(REG_JIT_TMP0);

    dynasm!(ops
        // r9 is pinned guest X1. Preserve it across the inline TLB fast path.
        ; mov QWORD [rdi + tmp_off], r9
        ; cmp QWORD [rdi + tlb_off], 0
        ; je >helper_read
        ; mov rcx, rax                   // rcx = guest addr
        ; shr rcx, 12                    // page number (VPN)
        ; mov rdx, QWORD [rdi + tlb_off] // rdx = TLB entries base ptr
        ; mov r9, rcx                    // r9 = VPN (tag to compare)
        ; and ecx, 0xFF                  // TLB index (256 entries)
        ; shl rcx, 4                     // entry_size = 16 bytes
        ; add rdx, rcx                   // rdx = &tlb[idx]
        ; cmp QWORD [rdx], r9            // tlb[idx].va_tag == VPN?
        ; jne >tlb_miss
        // TLB hit: compute host address = host_ptr + page_offset
        ; mov rcx, QWORD [rdx + 8]       // rcx = host_ptr (base of page)
        ; mov r9, rax
        ; and r9d, 0xFFF                 // page offset
        ; add rcx, r9                    // host effective address
    );
    // Load from host memory
    match size {
        1 => dynasm!(ops ; movzx eax, BYTE [rcx]),
        2 => dynasm!(ops ; movzx eax, WORD [rcx]),
        4 => dynasm!(ops ; mov eax, DWORD [rcx]),
        8 => dynasm!(ops ; mov rax, QWORD [rcx]),
        _ => unreachable!(),
    }
    dynasm!(ops
        ; jmp >tlb_done
        ; helper_read:
    );
    emit_mem_read(ops, size);
    dynasm!(ops
        ; jmp >tlb_done
        ; tlb_miss:
    );
    // TLB miss: call slow-path fill-and-read helper
    // Preserve caller-saved pinned regs (r8-r11 = X0-X3) and rdi/rsi around the call.
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8           // output buffer (8B); RSP now 0 mod 16
        // jit_se_tlb_fill_and_read(mem, tlb, addr, size, out) -> u64
        // arg1 (rdi) = mem ptr (original rsi)
        // arg2 (rsi) = tlb ptr = [flat + tlb_off]
        // arg3 (rdx) = addr (was rax)
        // arg4 (ecx) = size
        // arg5 (r8)  = out ptr = rsp
        ; mov rdx, rax                               // arg3: addr
        ; mov rdi, [rsp + 40]                        // arg1: mem ptr (original rsi)
    );
    // rdi now has mem ptr; the flat ptr (original rdi) is at [rsp+48].
    dynasm!(ops
        ; mov rsi, [rsp + 48]                        // rsi = original rdi (flat array ptr)
        ; mov rsi, QWORD [rsi + tlb_off]             // rsi = tlb entries base ptr
        ; mov ecx, size as i32                       // arg4: size
        ; lea r8, [rsp]                              // arg5: output ptr
        ; mov rax, QWORD crate::helpers::jit_se_tlb_fill_and_read as *const () as i64
        ; call rax
        ; mov rcx, [rsp]                             // output value
        ; add rsp, 8
        ; pop r11
        ; pop r10
        ; pop r9
        ; pop r8
        ; pop rsi
        ; pop rdi
        ; test rax, rax
        ; jnz >fault
        ; mov rax, rcx
        ; tlb_done:
        ; mov r9, QWORD [rdi + tmp_off]
    );
}

/// Emit an inline TLB store (SE mode).
///
/// Input: `rax` = guest effective address, `rcx` = value to write.
/// Clobbers: `rdx`, `r9` (caller-saved).
fn emit_tlb_store(ops: &mut Assembler, size: u32) {
    let tlb_off = reg_offset(REG_JIT_SE_TLB);
    let tmp_off = reg_offset(REG_JIT_TMP0);

    // Save value before we clobber rcx for the TLB lookup.
    dynasm!(ops
        // r9 is pinned guest X1. Preserve it across the inline TLB fast path.
        ; mov QWORD [rdi + tmp_off], r9
        ; cmp QWORD [rdi + tlb_off], 0
        ; je >helper_write
        ; push rcx                       // save value to write
        ; mov rcx, rax                   // rcx = guest addr
        ; shr rcx, 12                    // VPN
        ; mov rdx, QWORD [rdi + tlb_off] // rdx = TLB base
        ; mov r9, rcx                    // r9 = VPN
        ; and ecx, 0xFF                  // TLB index
        ; shl rcx, 4                     // *16
        ; add rdx, rcx                   // &tlb[idx]
        ; cmp QWORD [rdx], r9            // hit?
        ; jne >tlb_miss_store
        // TLB hit
        ; pop rcx                        // restore value
        ; mov r9, QWORD [rdx + 8]        // r9 = host_ptr
        ; mov rdx, rax
        ; and edx, 0xFFF                 // page offset
        ; add r9, rdx                    // host effective address
    );
    match size {
        1 => dynasm!(ops ; mov BYTE [r9], cl),
        2 => dynasm!(ops ; mov WORD [r9], cx),
        4 => dynasm!(ops ; mov DWORD [r9], ecx),
        8 => dynasm!(ops ; mov QWORD [r9], rcx),
        _ => unreachable!(),
    }
    dynasm!(ops
        ; jmp >tlb_store_done
        ; helper_write:
    );
    emit_mem_write(ops, size);
    dynasm!(ops
        ; jmp >tlb_store_done
        ; tlb_miss_store:
        ; pop rcx                        // restore value for slow path
    );
    // Slow path: jit_se_tlb_fill_and_write(mem, tlb, addr, val, size) -> u64
    dynasm!(ops
        ; push rdi
        ; push rsi
        ; push r8
        ; push r9
        ; push r10
        ; push r11
        ; sub rsp, 8
        ; mov r9, rcx                                // r9 = value (before rdi overwrite)
        ; mov rdx, rax                               // rdx = addr
        ; mov rdi, [rsp + 40]                        // rdi = mem ptr (original rsi)
        ; mov rsi, [rsp + 48]                        // rsi = flat array ptr (original rdi)
        ; mov rsi, QWORD [rsi + tlb_off]             // rsi = tlb ptr
        ; mov rcx, r9                                // rcx = value
        ; mov r8d, size as i32                       // r8 = size
        ; mov rax, QWORD crate::helpers::jit_se_tlb_fill_and_write as *const () as i64
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
        ; tlb_store_done:
        ; mov r9, QWORD [rdi + tmp_off]
    );
}

/// Emit the fault handler with a jump-over on the normal path.
///
/// On fault: flush pinned regs, store faulting PC, return EXIT_EXCEPTION.
/// rdi is valid (restored by emit_mem_read/write before jnz).
fn emit_fault_exit(ops: &mut Assembler, insn: &Instruction) {
    let pc_off = reg_offset(REG_PC);
    dynasm!(ops
        // Normal path jumps over the fault handler
        ; jmp >no_fault
        ; fault:
        ; mov rax, QWORD insn.pc as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    // Flush pinned registers before the exceptional exit.
    emit_pinned_epilogue(ops);
    dynasm!(ops
        ; mov rax, QWORD EXIT_EXCEPTION as i64
        ; ret
        ; no_fault:
    );
}

// ── LDR immediate ───────────────────────────────────────────────────────────

/// Emit load with immediate offset (unsigned, pre-index, or post-index).
///
/// Handles: LDR, LDRB, LDRH, LDRSB, LDRSH, LDRSW.
///
/// Internal scratch convention: `rax` = effective address (passed to emit_mem_read).
pub fn emit_ldr_imm(ops: &mut Assembler, insn: &Instruction) {
    let base_slot = if insn.rn == 31 {
        REG_SP
    } else {
        insn.rn as usize
    };
    let rd_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };
    let size = access_size(insn);
    let imm = insn.imm;
    let is_signed = matches!(insn.opcode, Opcode::Ldrsb | Opcode::Ldrsh | Opcode::Ldrsw | Opcode::Ldursb | Opcode::Ldursh | Opcode::Ldursw);

    emit_compute_address(ops, insn, base_slot);

    // Inline TLB load (rax = effective address on entry).
    // Falls back to jit_se_tlb_fill_and_read on miss.
    emit_tlb_load(ops, size);
    // After: rax = loaded value (on success).

    // Sign-extend if needed.
    if is_signed {
        match size {
            1 => dynasm!(ops ; movsx rax, al),
            2 => dynasm!(ops ; movsx rax, ax),
            4 => dynasm!(ops ; movsxd rax, eax),
            _ => {}
        }
    }

    // Store result to destination register.
    if insn.sf || is_signed {
        store_rax_to_guest(ops, rd_slot);
    } else {
        store_eax_to_guest_32(ops, rd_slot);
    }

    // Post-index writeback: base += imm.
    if insn.post_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, imm);
        store_rax_to_guest(ops, base_slot);
    }

    emit_fault_exit(ops, insn);
}

// ── STR immediate ───────────────────────────────────────────────────────────

/// Emit store with immediate offset.
///
/// Handles: STR, STRB, STRH.
///
/// Internal scratch convention:
/// - `rax` = effective address (passed to emit_mem_write)
/// - `rcx` = value to write  (passed to emit_mem_write)
pub fn emit_str_imm(ops: &mut Assembler, insn: &Instruction) {
    let base_slot = if insn.rn == 31 {
        REG_SP
    } else {
        insn.rn as usize
    };
    let rd_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };
    let size = access_size(insn);
    let stash_off = crate::regs::reg_offset(38);

    emit_compute_address(ops, insn, base_slot);
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);

    // Load value to write into rcx (safe scratch; not pinned).
    load_guest_to_rax(ops, rd_slot);
    dynasm!(ops ; mov rcx, rax); // rcx = value to write

    dynasm!(ops ; mov rax, QWORD [rdi + stash_off]);

    emit_tlb_store(ops, size);

    // Post-index writeback.
    if insn.post_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, insn.imm);
        store_rax_to_guest(ops, base_slot);
    }

    emit_fault_exit(ops, insn);
}

// ── LDP (load pair) ────────────────────────────────────────────────────────

/// Emit `LDP Xt1, Xt2, [Xn, #imm]`.
///
/// Uses slot 38 (reserved scratch) in the flat array to stash the base
/// address between the two memory read calls.
pub fn emit_ldp(ops: &mut Assembler, insn: &Instruction) {
    let base_slot = if insn.rn == 31 {
        REG_SP
    } else {
        insn.rn as usize
    };
    let rt1_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };
    let rt2_slot = if insn.pair_second == 31 {
        REG_XZR
    } else {
        insn.pair_second as usize
    };
    let size: u32 = if insn.sf { 8 } else { 4 };
    let imm = insn.imm;
    // Slot 38 is reserved as ldst scratch; not used by lazy NZCV (41–43).
    let stash_off = crate::regs::reg_offset(38);

    // Compute base address into rax.
    load_guest_to_rax(ops, base_slot);
    if insn.pre_index {
        emit_add_rax_imm64(ops, imm);
        store_rax_to_guest(ops, base_slot);
    } else if insn.post_index {
        // rax = base; no offset yet.
    } else {
        emit_add_rax_imm64(ops, imm);
    }

    // Stash effective address in reserved flat slot 38.
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);
    emit_tlb_load(ops, size); // rax → rax = loaded value

    // Store rt1.
    if insn.sf {
        store_rax_to_guest(ops, rt1_slot);
    } else {
        store_eax_to_guest_32(ops, rt1_slot);
    }

    // Second load: [addr + size].
    dynasm!(ops
        ; mov rax, QWORD [rdi + stash_off]
        ; add rax, size as i32
    );
    emit_tlb_load(ops, size);

    // Store rt2.
    if insn.sf {
        store_rax_to_guest(ops, rt2_slot);
    } else {
        store_eax_to_guest_32(ops, rt2_slot);
    }

    // Post-index writeback.
    if insn.post_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, imm);
        store_rax_to_guest(ops, base_slot);
    }

    emit_fault_exit(ops, insn);
}

// ── STP (store pair) ────────────────────────────────────────────────────────

/// Emit `STP Xt1, Xt2, [Xn, #imm]`.
///
/// Uses slot 38 (reserved scratch) to stash the base address between stores.
/// Internal convention: rax = effective address, rcx = value to write.
pub fn emit_stp(ops: &mut Assembler, insn: &Instruction) {
    let base_slot = if insn.rn == 31 {
        REG_SP
    } else {
        insn.rn as usize
    };
    let rt1_slot = if insn.rd == 31 {
        REG_XZR
    } else {
        insn.rd as usize
    };
    let rt2_slot = if insn.pair_second == 31 {
        REG_XZR
    } else {
        insn.pair_second as usize
    };
    let size: u32 = if insn.sf { 8 } else { 4 };
    let imm = insn.imm;
    let stash_off = crate::regs::reg_offset(38);

    // Compute base address into rax.
    load_guest_to_rax(ops, base_slot);
    if insn.pre_index {
        emit_add_rax_imm64(ops, imm);
        store_rax_to_guest(ops, base_slot);
    } else if insn.post_index {
        // rax = base; offset applied at writeback.
    } else {
        emit_add_rax_imm64(ops, imm);
    }

    // Stash effective address.
    dynasm!(ops ; mov QWORD [rdi + stash_off], rax);

    // First store: value = rt1.
    load_guest_to_rax(ops, rt1_slot);
    dynasm!(ops
        ; mov rcx, rax                     // rcx = value to write
        ; mov rax, QWORD [rdi + stash_off] // rax = effective address
    );
    emit_tlb_store(ops, size);

    // Second store: [addr + size], value = rt2.
    load_guest_to_rax(ops, rt2_slot);
    dynasm!(ops
        ; mov rcx, rax                     // rcx = rt2 value
        ; mov rax, QWORD [rdi + stash_off]
        ; add rax, size as i32             // rax = effective address + size
    );
    emit_tlb_store(ops, size);

    // Post-index writeback.
    if insn.post_index {
        load_guest_to_rax(ops, base_slot);
        emit_add_rax_imm64(ops, imm);
        store_rax_to_guest(ops, base_slot);
    }

    emit_fault_exit(ops, insn);
}
