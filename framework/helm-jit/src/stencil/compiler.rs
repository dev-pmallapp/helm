//! Stencil block compiler — copies and patches stencil templates into
//! executable memory.

#![allow(missing_docs)]
#![allow(unsafe_code)]

use std::ptr;

use crate::block::{CompiledBlock, JitBlockFn, EXIT_END_OF_BLOCK};
use crate::regs;
use super::types::{DecodedFields, HoleKind, RegField, Stencil};

/// x86-64 to re-zero XZR slot: `mov qword [rdi + XZR_OFF], 0`
/// Encoding: 48 C7 87 <off32> 00000000
const XZR_REZERO_LEN: usize = 11;

fn emit_xzr_rezero(buf: &mut [u8], offset: &mut usize) {
    let xzr_off = regs::reg_offset(regs::REG_XZR);
    let off_bytes = (xzr_off as u32).to_le_bytes();
    let patch = [
        0x48, 0xC7, 0x87, // MOV QWORD [rdi + disp32], imm32
        off_bytes[0], off_bytes[1], off_bytes[2], off_bytes[3],
        0x00, 0x00, 0x00, 0x00, // imm32 = 0
    ];
    buf[*offset..*offset + XZR_REZERO_LEN].copy_from_slice(&patch);
    *offset += XZR_REZERO_LEN;
}

/// RAII wrapper around `mmap`'d executable memory. Calls `munmap` on drop.
pub struct MmapBuffer {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: The buffer is self-contained memory that doesn't alias anything.
// Once mprotected to R+X it's immutable.
unsafe impl Send for MmapBuffer {}
unsafe impl Sync for MmapBuffer {}

impl MmapBuffer {
    /// Allocate a new anonymous mmap region with READ+WRITE permissions.
    fn new(len: usize) -> Option<Self> {
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return None;
        }
        Some(Self {
            ptr: ptr as *mut u8,
            len,
        })
    }

    /// Make the buffer executable (and read-only). Enforces W^X.
    fn make_executable(&self) -> bool {
        let ret = unsafe {
            libc::mprotect(
                self.ptr as *mut libc::c_void,
                self.len,
                libc::PROT_READ | libc::PROT_EXEC,
            )
        };
        ret == 0
    }

    /// Get a pointer to the start of the buffer.
    fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Get a mutable slice of the buffer (only valid before `make_executable`).
    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for MmapBuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

/// Resolve a hole's value from the decoded instruction fields.
pub fn resolve_hole(hole: &HoleKind, fields: &DecodedFields) -> u64 {
    match hole {
        HoleKind::RegOffset(reg_field) => {
            let reg_index = match reg_field {
                RegField::Rd => fields.rd,
                RegField::Rn => fields.rn,
                RegField::Rm => fields.rm,
                RegField::Ra => fields.ra,
                RegField::Rt => fields.rt,
                RegField::Rt2 => fields.rt2,
            };
            u64::from(reg_index) * 8
        }
        HoleKind::ImmSext { from_bits } => {
            let val = fields.imm;
            let shift = 64 - *from_bits as u32;
            // Sign-extend by shifting left then arithmetic-right
            ((val << shift) >> shift) as u64
        }
        HoleKind::ImmZext => fields.imm as u64,
        HoleKind::Helper(helper) => helper.address(),
        HoleKind::BranchTarget => fields.branch_target,
        HoleKind::NextPc => fields.next_pc,
        HoleKind::Simm => fields.simm as u64,
        HoleKind::Shamt => u64::from(fields.shamt),
    }
}

/// Page size for mmap alignment.
fn page_size() -> usize {
    4096
}

/// Round up to the next page boundary.
fn page_align(size: usize) -> usize {
    let ps = page_size();
    (size + ps - 1) & !(ps - 1)
}

/// Compile a sequence of stencil entries into an executable block.
///
/// Each stencil is a complete x86-64 function (with its own prologue/epilogue).
/// For non-terminator stencils, we emit an epilogue wrapper that updates PC
/// and returns EXIT_END_OF_BLOCK after the stencil returns.
///
/// For terminator stencils (branches, syscalls), the stencil itself updates
/// PC and returns the exit code.
///
/// Returns `None` if the entries are empty or mmap fails.
pub fn compile_block(
    pc: u64,
    entries: &[(&Stencil, DecodedFields)],
) -> Option<CompiledBlock> {
    if entries.is_empty() {
        return None;
    }

    // For v1: compile exactly one stencil per block.
    // The stencil is a complete C function — we copy it in full, patch its
    // R_X86_64_32S holes (4-byte writes), and wrap non-terminators with
    // a trampoline that updates PC and returns EXIT_END_OF_BLOCK.
    //
    // Layout for non-terminator:
    //   [call stencil_fn]     — call rel32 (5 bytes, jumps over epilogue to stencil)
    //   [update PC]           — movabs rax, next_pc; mov [rdi+PC_OFF], rax (17 bytes)
    //   [XZR re-zero]         — optional 11 bytes
    //   [mov eax, 0; ret]     — return EXIT_END_OF_BLOCK (6 bytes)
    //   [stencil bytes]       — the complete stencil function
    //
    // Wait — this won't work because after `call`, the stencil's `ret` returns
    // to our epilogue code. But `call` pushes a return address, and `rdi`/`rsi`
    // are preserved across the call since the stencil is a leaf or saves them.
    // Actually the stencil IS a function with the right calling convention
    // (rdi=regs, rsi=mem), so we can `call` it directly.
    //
    // Simpler approach: just emit the stencil as the block body, but
    // intercept its `ret` by replacing it with a jump to our epilogue.
    //
    // Simplest correct approach: wrap in call/epilogue trampoline.

    let (stencil, fields) = entries[0];
    let stencil_len = stencil.bytes.len();

    if stencil.is_terminator {
        // Terminator: copy stencil directly. It updates PC and returns exit code.
        let buf_len = page_align(stencil_len);
        let mut buf = MmapBuffer::new(buf_len)?;
        let buf_base = buf.as_ptr() as u64;
        let slice = buf.as_mut_slice();

        // Copy stencil bytes.
        slice[..stencil_len].copy_from_slice(stencil.bytes);

        // Patch 4-byte holes.
        patch_holes(slice, 0, stencil, &fields, buf_base);

        if !buf.make_executable() {
            return None;
        }
        let entry: JitBlockFn = unsafe { std::mem::transmute(buf.as_ptr()) };
        return Some(unsafe { CompiledBlock::new(buf, entry, pc, 1) });
    }

    // Non-terminator: emit call trampoline + epilogue.
    //
    // Layout:
    //   offset 0:   call rel32          → jumps to stencil at offset (epilogue_len + 5)
    //   offset 5:   [epilogue: update PC, XZR re-zero, mov eax 0, ret]
    //   offset E:   [stencil bytes, patched]
    //
    // After the call, the stencil's `ret` returns to offset 5 (the epilogue).

    let needs_xzr = fields.rd == 31;
    // XZR re-zero with r12 base is 12 bytes (49 C7 84 24 <disp32> 00000000)
    let xzr_len = if needs_xzr { 12 } else { 0 };
    // Prologue: push r12 (2) + mov r12,rdi (3) + call rel32 (5) = 10 bytes
    //   (push r12 also aligns stack: entry RSP%16==8, after push RSP%16==0)
    // Epilogue: movabs rax,next_pc (10) + mov [r12+off],rax (8)
    //         + xzr using r12 (0|12) + mov eax,0 (5) + pop r12 (2) + ret (1) = 26 or 38
    let prologue_len = 2 + 3 + 5; // push r12 + mov r12,rdi + call rel32
    let epilogue_len = 10 + 8 + xzr_len + 5 + 2 + 1;
    let total = prologue_len + epilogue_len + stencil_len;
    let buf_len = page_align(total);
    let mut buf = MmapBuffer::new(buf_len)?;
    let buf_base = buf.as_ptr() as u64;
    let slice = buf.as_mut_slice();

    let mut pos = 0;

    // push r12 (save callee-saved reg, also aligns stack)
    // 41 54
    slice[pos] = 0x41;
    slice[pos + 1] = 0x54;
    pos += 2;

    // mov r12, rdi (save regs pointer in callee-saved r12)
    // 49 89 FC
    slice[pos] = 0x49;
    slice[pos + 1] = 0x89;
    slice[pos + 2] = 0xFC;
    pos += 3;

    // call rel32 → stencil
    let stencil_target = prologue_len + epilogue_len;
    let rel32 = (stencil_target as i32) - ((pos + 5) as i32);
    slice[pos] = 0xE8;
    slice[pos + 1..pos + 5].copy_from_slice(&rel32.to_le_bytes());
    pos += 5;

    debug_assert_eq!(pos, prologue_len);

    // ── Epilogue (stencil returns here) ──

    // movabs rax, next_pc (10 bytes)
    let next_pc_bytes = fields.next_pc.to_le_bytes();
    slice[pos] = 0x48;
    slice[pos + 1] = 0xB8;
    slice[pos + 2..pos + 10].copy_from_slice(&next_pc_bytes);
    pos += 10;

    // mov [r12 + PC_OFF], rax (8 bytes: 49 89 84 24 <disp32>)
    let pc_off = regs::reg_offset(regs::REG_PC);
    let pc_off_bytes = (pc_off as u32).to_le_bytes();
    slice[pos] = 0x49;
    slice[pos + 1] = 0x89;
    slice[pos + 2] = 0x84;
    slice[pos + 3] = 0x24;
    slice[pos + 4..pos + 8].copy_from_slice(&pc_off_bytes);
    pos += 8;

    // XZR re-zero if needed (using r12 as base).
    if needs_xzr {
        // mov qword [r12 + XZR_OFF], 0
        // 49 C7 84 24 <disp32> 00000000 = 12 bytes
        let xzr_off = regs::reg_offset(regs::REG_XZR);
        let xzr_off_bytes = (xzr_off as u32).to_le_bytes();
        slice[pos] = 0x49;
        slice[pos + 1] = 0xC7;
        slice[pos + 2] = 0x84;
        slice[pos + 3] = 0x24;
        slice[pos + 4..pos + 8].copy_from_slice(&xzr_off_bytes);
        slice[pos + 8] = 0x00;
        slice[pos + 9] = 0x00;
        slice[pos + 10] = 0x00;
        slice[pos + 11] = 0x00;
        pos += 12;
    }

    // mov eax, EXIT_END_OF_BLOCK (5 bytes)
    let exit_bytes = (EXIT_END_OF_BLOCK as u32).to_le_bytes();
    slice[pos] = 0xB8;
    slice[pos + 1..pos + 5].copy_from_slice(&exit_bytes);
    pos += 5;

    // pop r12 (restore callee-saved)
    // 41 5C
    slice[pos] = 0x41;
    slice[pos + 1] = 0x5C;
    pos += 2;

    // ret (1 byte)
    slice[pos] = 0xC3;
    pos += 1;

    // Copy stencil bytes right after epilogue.
    debug_assert_eq!(pos, prologue_len + epilogue_len);
    let stencil_start = pos;
    slice[stencil_start..stencil_start + stencil_len]
        .copy_from_slice(stencil.bytes);

    // Patch 4-byte holes in the stencil.
    patch_holes(slice, stencil_start, stencil, &fields, buf_base);

    let _ = pos;

    if !buf.make_executable() {
        return None;
    }

    let entry: JitBlockFn = unsafe { std::mem::transmute(buf.as_ptr()) };
    Some(unsafe { CompiledBlock::new(buf, entry, pc, 1) })
}

/// Patch relocation holes in a stencil (4 bytes each).
///
/// For Abs32 (R_X86_64_32S): writes the low 32 bits of the resolved value.
/// For PcRel32 (R_X86_64_PLT32): writes `target - (buf_runtime_addr + reloc_site) - 4`.
fn patch_holes(
    buf: &mut [u8],
    stencil_start: usize,
    stencil: &Stencil,
    fields: &DecodedFields,
    buf_base: u64,
) {
    use super::types::RelocKind;

    for reloc in stencil.relocs.iter() {
        let bo = reloc.byte_offset as usize;
        if bo + 4 <= stencil.bytes.len() {
            let val = resolve_hole(&reloc.hole, fields);
            let dst = stencil_start + bo;
            let patched: u32 = match reloc.kind {
                RelocKind::Abs32 => val as u32,
                RelocKind::PcRel32 => {
                    // PC-relative: target - rip, where rip = buf_base + dst + 4
                    let rip = buf_base + dst as u64 + 4;
                    (val.wrapping_sub(rip)) as u32
                }
            };
            buf[dst..dst + 4].copy_from_slice(&patched.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::StencilReloc;

    #[test]
    fn mmap_buffer_allocates_and_drops() {
        let buf = MmapBuffer::new(4096);
        assert!(buf.is_some());
        let buf = buf.unwrap();
        assert!(!buf.as_ptr().is_null());
        // Drop runs munmap — should not crash.
    }

    #[test]
    fn mmap_buffer_make_executable() {
        let buf = MmapBuffer::new(4096).unwrap();
        assert!(buf.make_executable());
    }

    #[test]
    fn resolve_hole_reg_offset() {
        let fields = DecodedFields { rd: 5, ..Default::default() };
        let val = resolve_hole(&HoleKind::RegOffset(RegField::Rd), &fields);
        assert_eq!(val, 40); // 5 * 8
    }

    #[test]
    fn resolve_hole_reg_offset_xzr() {
        let fields = DecodedFields { rd: 31, ..Default::default() };
        let val = resolve_hole(&HoleKind::RegOffset(RegField::Rd), &fields);
        assert_eq!(val, 248); // 31 * 8
    }

    #[test]
    fn resolve_hole_imm_zext() {
        let fields = DecodedFields { imm: 42, ..Default::default() };
        let val = resolve_hole(&HoleKind::ImmZext, &fields);
        assert_eq!(val, 42);
    }

    #[test]
    fn resolve_hole_imm_sext_positive() {
        let fields = DecodedFields { imm: 0x7FF, ..Default::default() };
        let val = resolve_hole(&HoleKind::ImmSext { from_bits: 12 }, &fields);
        assert_eq!(val, 0x7FF); // positive, no change
    }

    #[test]
    fn resolve_hole_imm_sext_negative() {
        // 12-bit value 0xFFF = -1 when sign-extended
        let fields = DecodedFields { imm: -1, ..Default::default() };
        let val = resolve_hole(&HoleKind::ImmSext { from_bits: 12 }, &fields);
        assert_eq!(val as i64, -1);
    }

    #[test]
    fn resolve_hole_branch_target() {
        let fields = DecodedFields {
            branch_target: 0x4000_1000,
            ..Default::default()
        };
        let val = resolve_hole(&HoleKind::BranchTarget, &fields);
        assert_eq!(val, 0x4000_1000);
    }

    #[test]
    fn resolve_hole_next_pc() {
        let fields = DecodedFields {
            next_pc: 0x4000_0004,
            ..Default::default()
        };
        let val = resolve_hole(&HoleKind::NextPc, &fields);
        assert_eq!(val, 0x4000_0004);
    }

    #[test]
    fn resolve_hole_helper_mem_read() {
        use crate::helpers::jit_mem_read;
        let fields = DecodedFields::default();
        let val = resolve_hole(
            &HoleKind::Helper(super::super::types::HelperFn::MemRead),
            &fields,
        );
        assert_eq!(val, jit_mem_read as *const () as u64);
    }

    #[test]
    fn compile_block_empty_returns_none() {
        assert!(compile_block(0x1000, &[]).is_none());
    }

    #[test]
    fn compile_block_single_nop_stencil() {
        // A trivial stencil: just a NOP (0x90) + ret — non-terminator.
        static NOP_BYTES: [u8; 2] = [0x90, 0xC3];
        static NOP_RELOCS: [StencilReloc; 0] = [];
        let stencil = Stencil {
            bytes: &NOP_BYTES,
            body_len: 1,
            relocs: &NOP_RELOCS,
            is_terminator: false,
        };
        let fields = DecodedFields {
            next_pc: 0x1004,
            ..Default::default()
        };
        let block = compile_block(0x1000, &[(&stencil, fields)]);
        assert!(block.is_some());
        let block = block.unwrap();
        assert_eq!(block.guest_pc, 0x1000);
        assert_eq!(block.insn_count, 1);
    }

    #[test]
    fn compile_block_terminator_no_epilogue() {
        // A terminator stencil: mov rax, 0; ret (returns EXIT_END_OF_BLOCK).
        static TERM_BYTES: [u8; 8] = [
            0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
            0xC3, // ret
        ];
        static TERM_RELOCS: [StencilReloc; 0] = [];
        let stencil = Stencil {
            bytes: &TERM_BYTES,
            body_len: 8,
            relocs: &TERM_RELOCS,
            is_terminator: true,
        };
        let fields = DecodedFields::default();
        let block = compile_block(0x1000, &[(&stencil, fields)]);
        assert!(block.is_some());
    }

    #[test]
    fn page_align_rounds_up() {
        assert_eq!(page_align(1), 4096);
        assert_eq!(page_align(4096), 4096);
        assert_eq!(page_align(4097), 8192);
    }

    #[test]
    fn execute_generated_add_imm_stencil() {
        use crate::regs::{REG_COUNT, REG_PC};
        use crate::stencil::data::aarch64;
        use helm_arch::aarch64::insn::{Instruction, Opcode};

        // Build an ADD X1, X2, #42 instruction
        let insn = Instruction {
            opcode: Opcode::AddImm,
            rd: 1,
            rn: 2,
            imm: 42,
            sf: true,
            pc: 0x1000,
            ..Instruction::zeroed()
        };
        let stencil = aarch64::lookup(&insn).unwrap().unwrap();
        let fields = crate::stencil::fields::extract_fields_a64(&insn, 0x1000);
        let block = compile_block(0x1000, &[(stencil, fields)]).unwrap();

        let mut regs = [0u64; REG_COUNT];
        regs[2] = 100; // X2 = 100
        let mut dummy_mem = [0u8; 8];
        let exit = unsafe {
            (block.entry)(regs.as_mut_ptr(), dummy_mem.as_mut_ptr())
        };

        assert_eq!(exit, 0, "should return EXIT_END_OF_BLOCK");
        assert_eq!(regs[1], 142, "X1 should be 100+42=142");
        assert_eq!(regs[REG_PC], 0x1004, "PC should advance to next insn");
    }

    #[test]
    fn execute_generated_ldr64_stencil() {
        use crate::regs::{REG_COUNT, REG_PC, REG_JIT_MEM_READ, REG_JIT_MEM_WRITE};
        use crate::stencil::data::aarch64;
        use crate::helpers;
        use helm_arch::aarch64::insn::{Instruction, Opcode};
        use helm_memory::FlatMem;
        use helm_core::MemInterface;

        // Set up guest memory with a known value at the base
        let mut mem = FlatMem::new(0x1_0000, 0x1000);
        mem.write(0x1_0000, 8, 0xDEAD_BEEF_CAFE_BABEu64, helm_core::AccessType::Store).unwrap();

        // Build LDR X3, [X4, #0] — load 8 bytes from address in X4
        let insn = Instruction {
            opcode: Opcode::Ldr,
            rd: 3,  // destination = X3
            rn: 4,  // base = X4
            imm: 0, // offset = 0
            sf: true,
            size: 3, // 8 bytes
            pc: 0x2000,
            ..Instruction::zeroed()
        };
        let stencil = aarch64::lookup(&insn).unwrap().unwrap();
        let fields = crate::stencil::fields::extract_fields_a64(&insn, 0x2000);
        let block = compile_block(0x2000, &[(stencil, fields)]).unwrap();

        let mut regs = [0u64; REG_COUNT];
        regs[4] = 0x1_0000; // X4 = base address
        // Populate helper function pointers
        regs[REG_JIT_MEM_READ] = helpers::jit_mem_read as *const () as u64;
        regs[REG_JIT_MEM_WRITE] = helpers::jit_mem_write as *const () as u64;

        let mem_ptr = &mut mem as *mut FlatMem as *mut u8;
let exit = unsafe {
            (block.entry)(regs.as_mut_ptr(), mem_ptr)
        };

        assert_eq!(exit, 0, "should return EXIT_END_OF_BLOCK");
        assert_eq!(regs[3], 0xDEAD_BEEF_CAFE_BABE, "X3 should have loaded value");
        assert_eq!(regs[REG_PC], 0x2004, "PC should advance");
    }

    #[test]
    fn execute_generated_cbz_taken() {
        use crate::regs::{REG_COUNT, REG_PC};
        use crate::stencil::data::aarch64;
        use helm_arch::aarch64::insn::{Instruction, Opcode};

        // CBZ X5, target (X5==0 → taken)
        let insn = Instruction {
            opcode: Opcode::Cbz,
            rd: 5,     // Rt
            imm: 0x40, // branch offset
            sf: true,
            pc: 0x3000,
            ..Instruction::zeroed()
        };
        let stencil = aarch64::lookup(&insn).unwrap().unwrap();
        let fields = crate::stencil::fields::extract_fields_a64(&insn, 0x3000);
        let block = compile_block(0x3000, &[(stencil, fields)]).unwrap();

        let mut regs = [0u64; REG_COUNT];
        regs[5] = 0; // X5 = 0 → branch taken
        let mut dummy_mem = [0u8; 8];
        let exit = unsafe {
            (block.entry)(regs.as_mut_ptr(), dummy_mem.as_mut_ptr())
        };
        assert_eq!(exit, 0, "CBZ should return EXIT_END_OF_BLOCK");
        assert_eq!(regs[REG_PC], 0x3040, "PC should be branch target (0x3000+0x40)");
    }

    #[test]
    fn execute_generated_cbz_not_taken() {
        use crate::regs::{REG_COUNT, REG_PC};
        use crate::stencil::data::aarch64;
        use helm_arch::aarch64::insn::{Instruction, Opcode};

        // CBZ X5, target (X5!=0 → not taken)
        let insn = Instruction {
            opcode: Opcode::Cbz,
            rd: 5,
            imm: 0x40,
            sf: true,
            pc: 0x3000,
            ..Instruction::zeroed()
        };
        let stencil = aarch64::lookup(&insn).unwrap().unwrap();
        let fields = crate::stencil::fields::extract_fields_a64(&insn, 0x3000);
        let block = compile_block(0x3000, &[(stencil, fields)]).unwrap();

        let mut regs = [0u64; REG_COUNT];
        regs[5] = 42; // X5 = 42 → not taken
        let mut dummy_mem = [0u8; 8];
        let exit = unsafe {
            (block.entry)(regs.as_mut_ptr(), dummy_mem.as_mut_ptr())
        };
        assert_eq!(exit, 0, "CBZ should return EXIT_END_OF_BLOCK");
        assert_eq!(regs[REG_PC], 0x3004, "PC should be next_pc (0x3000+4)");
    }
}
