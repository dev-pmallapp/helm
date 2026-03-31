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
/// **Leaf stencils** (DP, MOV, NOP — no stack frame, no helper calls) are
/// concatenated by stripping trailing `ret` bytes, eliminating per-instruction
/// dispatch overhead.
///
/// **Non-leaf stencils** (loads, stores — call helpers via the register array)
/// are wrapped in a call-trampoline that saves/restores r12 for the epilogue.
///
/// **Terminator stencils** (branches) are copied directly — they set PC and
/// return an exit code themselves.
///
/// Returns `None` if the entries are empty or mmap fails.
pub fn compile_block(
    pc: u64,
    entries: &[(&Stencil, DecodedFields)],
) -> Option<CompiledBlock> {
    if entries.is_empty() {
        return None;
    }

    let last_idx = entries.len() - 1;
    let (last_stencil, last_fields) = entries[last_idx];

    // ── Calculate buffer size ──
    let mut total = 0usize;
    let mut any_xzr = false;
    for (i, (s, f)) in entries.iter().enumerate() {
        if i < last_idx {
            // Middle entries must be leaf — use body_len (ret stripped).
            total += s.body_len;
            if f.rd == 31 {
                total += XZR_REZERO_LEN;
                any_xzr = true;
            }
        } else if s.is_terminator {
            total += s.bytes.len();
        } else if s.is_leaf {
            // Last leaf non-terminator: body_len + epilogue
            total += s.body_len;
            total += EPILOGUE_LEN;
            if f.rd == 31 {
                total += XZR_REZERO_LEN;
                any_xzr = true;
            }
        } else {
            // Last non-leaf non-terminator: trampoline wrapper
            total += trampoline_size(s, f);
        }
    }

    let buf_len = page_align(total.max(1));
    let mut buf = MmapBuffer::new(buf_len)?;
    let buf_base = buf.as_ptr() as u64;
    let slice = buf.as_mut_slice();
    let mut pos = 0;
    let insn_count = entries.len() as u32;

    // ── Emit entries ──
    for (i, (s, f)) in entries.iter().enumerate() {
        if i < last_idx {
            // Middle: chain leaf body (ret stripped)
            let copy_len = s.body_len;
            slice[pos..pos + copy_len].copy_from_slice(&s.bytes[..copy_len]);
            patch_holes(slice, pos, s, f, buf_base);
            pos += copy_len;
            if f.rd == 31 {
                emit_xzr_rezero(slice, &mut pos);
            }
        } else if s.is_terminator {
            // Last = terminator: copy full bytes (sets PC + returns exit code)
            let copy_len = s.bytes.len();
            slice[pos..pos + copy_len].copy_from_slice(s.bytes);
            patch_holes(slice, pos, s, f, buf_base);
            pos += copy_len;
        } else if s.is_leaf {
            // Last = leaf non-terminator: chain body + epilogue
            let copy_len = s.body_len;
            slice[pos..pos + copy_len].copy_from_slice(&s.bytes[..copy_len]);
            patch_holes(slice, pos, s, f, buf_base);
            pos += copy_len;
            if f.rd == 31 {
                emit_xzr_rezero(slice, &mut pos);
            }
            emit_epilogue(slice, &mut pos, f);
        } else {
            // Last = non-leaf: call-trampoline
            emit_trampoline(slice, &mut pos, s, f, buf_base);
        }
    }

    let _ = (pos, any_xzr);

    if !buf.make_executable() {
        return None;
    }
    let entry: JitBlockFn = unsafe { std::mem::transmute(buf.as_ptr()) };
    Some(unsafe { CompiledBlock::new(buf, entry, pc, insn_count) })
}

/// Size of the end-of-block epilogue: movabs+store PC (17) + mov eax,0 (5) + ret (1) = 23.
const EPILOGUE_LEN: usize = 23;

/// Emit the end-of-block epilogue: update PC to next_pc, return EXIT_END_OF_BLOCK.
fn emit_epilogue(buf: &mut [u8], pos: &mut usize, fields: &DecodedFields) {
    let p = *pos;
    // movabs rax, next_pc (10 bytes)
    buf[p] = 0x48;
    buf[p + 1] = 0xB8;
    buf[p + 2..p + 10].copy_from_slice(&fields.next_pc.to_le_bytes());
    // mov [rdi + PC_OFF], rax (7 bytes)
    let pc_off_bytes = (regs::reg_offset(regs::REG_PC) as u32).to_le_bytes();
    buf[p + 10] = 0x48;
    buf[p + 11] = 0x89;
    buf[p + 12] = 0x87;
    buf[p + 13..p + 17].copy_from_slice(&pc_off_bytes);
    // mov eax, 0 (5 bytes)
    buf[p + 17] = 0xB8;
    buf[p + 18..p + 22].copy_from_slice(&(EXIT_END_OF_BLOCK as u32).to_le_bytes());
    // ret
    buf[p + 22] = 0xC3;
    *pos = p + 23;
}

/// Size of a non-leaf trampoline wrapper for a single stencil.
fn trampoline_size(s: &Stencil, f: &DecodedFields) -> usize {
    let xzr_len = if f.rd == 31 { 12 } else { 0 };
    let prologue = 2 + 3 + 5; // push r12 + mov r12,rdi + call rel32
    let epilogue = 10 + 8 + xzr_len + 5 + 2 + 1; // PC update + exit + pop + ret
    prologue + epilogue + s.bytes.len()
}

/// Emit a call-trampoline for a non-leaf stencil (saves rdi→r12, calls
/// stencil, updates PC via r12, returns).
fn emit_trampoline(
    buf: &mut [u8],
    pos: &mut usize,
    stencil: &Stencil,
    fields: &DecodedFields,
    buf_base: u64,
) {
    let p = *pos;
    let needs_xzr = fields.rd == 31;
    let xzr_len = if needs_xzr { 12 } else { 0 };
    let prologue_len = 10;
    let epilogue_len = 10 + 8 + xzr_len + 5 + 2 + 1;

    // push r12
    buf[p] = 0x41;
    buf[p + 1] = 0x54;
    // mov r12, rdi
    buf[p + 2] = 0x49;
    buf[p + 3] = 0x89;
    buf[p + 4] = 0xFC;
    // call rel32 → stencil
    let stencil_offset = prologue_len + epilogue_len;
    let rel32 = (stencil_offset as i32) - 10; // relative to end of call insn at p+10
    buf[p + 5] = 0xE8;
    buf[p + 6..p + 10].copy_from_slice(&rel32.to_le_bytes());

    let mut ep = p + prologue_len;

    // movabs rax, next_pc (10)
    buf[ep] = 0x48;
    buf[ep + 1] = 0xB8;
    buf[ep + 2..ep + 10].copy_from_slice(&fields.next_pc.to_le_bytes());
    ep += 10;
    // mov [r12 + PC_OFF], rax (8)
    let pc_off_bytes = (regs::reg_offset(regs::REG_PC) as u32).to_le_bytes();
    buf[ep] = 0x49;
    buf[ep + 1] = 0x89;
    buf[ep + 2] = 0x84;
    buf[ep + 3] = 0x24;
    buf[ep + 4..ep + 8].copy_from_slice(&pc_off_bytes);
    ep += 8;
    // XZR re-zero via r12
    if needs_xzr {
        let xzr_off_bytes = (regs::reg_offset(regs::REG_XZR) as u32).to_le_bytes();
        buf[ep] = 0x49;
        buf[ep + 1] = 0xC7;
        buf[ep + 2] = 0x84;
        buf[ep + 3] = 0x24;
        buf[ep + 4..ep + 8].copy_from_slice(&xzr_off_bytes);
        buf[ep + 8..ep + 12].copy_from_slice(&[0, 0, 0, 0]);
        ep += 12;
    }
    // mov eax, 0 (5)
    buf[ep] = 0xB8;
    buf[ep + 1..ep + 5].copy_from_slice(&(EXIT_END_OF_BLOCK as u32).to_le_bytes());
    ep += 5;
    // pop r12 (2)
    buf[ep] = 0x41;
    buf[ep + 1] = 0x5C;
    ep += 2;
    // ret
    buf[ep] = 0xC3;
    ep += 1;

    // Copy stencil bytes
    let stencil_start = ep;
    let sl = stencil.bytes.len();
    buf[stencil_start..stencil_start + sl].copy_from_slice(stencil.bytes);
    patch_holes(buf, stencil_start, stencil, fields, buf_base);

    *pos = stencil_start + sl;
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
            is_leaf: true,
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
            is_leaf: true,
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
