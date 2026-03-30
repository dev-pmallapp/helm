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
    let xzr_len = if needs_xzr { XZR_REZERO_LEN } else { 0 };
    // Prologue: sub rsp,8 (4) + call rel32 (5) = 9 bytes
    // Epilogue: add rsp,8 (4) + movabs rax,next_pc (10) + mov [rdi+off],rax (7)
    //         + xzr (0|11) + mov eax,0 (5) + ret (1) = 27 or 38
    let prologue_len = 4 + 5; // sub rsp,8 + call rel32
    let epilogue_len = 4 + 17 + xzr_len + 6; // add rsp,8 + PC update + exit + ret
    let total = prologue_len + epilogue_len + stencil_len;
    let buf_len = page_align(total);
    let mut buf = MmapBuffer::new(buf_len)?;
    let buf_base = buf.as_ptr() as u64;
    let slice = buf.as_mut_slice();

    // Emit: sub rsp, 8 (align stack for nested call)
    // 48 83 EC 08
    slice[0] = 0x48;
    slice[1] = 0x83;
    slice[2] = 0xEC;
    slice[3] = 0x08;

    // Emit: call rel32 (target = prologue_len + epilogue_len - 4 - 5 ... )
    // The call is at offset 4, target is stencil at offset (prologue_len + epilogue_len)
    // rel32 = target - (call_addr + 5) = (prologue_len + epilogue_len) - (4 + 5)
    let call_addr = 4;
    let stencil_target = prologue_len + epilogue_len;
    let rel32 = (stencil_target as i32) - ((call_addr + 5) as i32);
    slice[4] = 0xE8; // call rel32
    slice[5..9].copy_from_slice(&rel32.to_le_bytes());

    // Emit epilogue starting at offset 9 (prologue_len).
    let mut epi = prologue_len;

    // add rsp, 8 (undo stack alignment)
    slice[epi] = 0x48;
    slice[epi + 1] = 0x83;
    slice[epi + 2] = 0xC4;
    slice[epi + 3] = 0x08;
    epi += 4;

    // movabs rax, next_pc (10 bytes)
    let next_pc_bytes = fields.next_pc.to_le_bytes();
    slice[epi] = 0x48;
    slice[epi + 1] = 0xB8;
    slice[epi + 2..epi + 10].copy_from_slice(&next_pc_bytes);
    epi += 10;

    // mov [rdi + PC_OFF], rax (7 bytes)
    let pc_off = regs::reg_offset(regs::REG_PC);
    let pc_off_bytes = (pc_off as u32).to_le_bytes();
    slice[epi] = 0x48;
    slice[epi + 1] = 0x89;
    slice[epi + 2] = 0x87;
    slice[epi + 3..epi + 7].copy_from_slice(&pc_off_bytes);
    epi += 7;

    // XZR re-zero if needed.
    if needs_xzr {
        emit_xzr_rezero(slice, &mut epi);
    }

    // mov eax, EXIT_END_OF_BLOCK (5 bytes)
    let exit_bytes = (EXIT_END_OF_BLOCK as u32).to_le_bytes();
    slice[epi] = 0xB8;
    slice[epi + 1..epi + 5].copy_from_slice(&exit_bytes);
    epi += 5;

    // ret (1 byte)
    slice[epi] = 0xC3;
    epi += 1;

    // Copy stencil bytes right after epilogue.
    debug_assert_eq!(epi, prologue_len + epilogue_len);
    let stencil_start = epi;
    slice[stencil_start..stencil_start + stencil_len]
        .copy_from_slice(stencil.bytes);

    // Patch 4-byte holes in the stencil.
    patch_holes(slice, stencil_start, stencil, &fields, buf_base);

    let _ = epi;

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
}
