//! Stencil block compiler — copies and patches stencil templates into
//! executable memory.

#![allow(missing_docs)]
#![allow(unsafe_code)]

use std::ptr;

use crate::block::{CompiledBlock, JitBlockFn, EXIT_END_OF_BLOCK};
use crate::regs;
use super::types::{DecodedFields, HoleKind, RegField, Stencil};

/// Length of the end-of-block epilogue: movabs rax,next_pc(10) + mov [rdi+off],rax(7)
/// + mov eax,0(5) + ret(1) = 23 bytes.
const EPILOGUE_LEN: usize = 23;

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
/// Each entry is a `(stencil, decoded_fields)` pair. The stencils are
/// concatenated, holes are patched, and the result is mprotected to R+X.
///
/// Returns `None` if the entries are empty or mmap fails.
pub fn compile_block(
    pc: u64,
    entries: &[(&Stencil, DecodedFields)],
) -> Option<CompiledBlock> {
    if entries.is_empty() {
        return None;
    }

    // Calculate total buffer size.
    let mut total = 0usize;
    let last_idx = entries.len() - 1;
    for (i, (stencil, fields)) in entries.iter().enumerate() {
        if i == last_idx && stencil.is_terminator {
            // Terminator keeps its own epilogue (ret).
            total += stencil.bytes.len();
        } else if i == last_idx {
            // Last non-terminator: use body_len + epilogue.
            total += stencil.body_len;
            total += EPILOGUE_LEN;
        } else {
            // Middle stencil: body only.
            total += stencil.body_len;
        }

        // XZR re-zero needed if rd=31 and not the last stencil.
        if i < last_idx && fields.rd == 31 {
            total += XZR_REZERO_LEN;
        }
    }

    let buf_len = page_align(total.max(1));
    let mut buf = MmapBuffer::new(buf_len)?;

    // Copy stencil bytes and patch holes.
    let slice = buf.as_mut_slice();
    let mut offset = 0usize;

    for (i, (stencil, fields)) in entries.iter().enumerate() {
        let copy_len = if i == last_idx && stencil.is_terminator {
            stencil.bytes.len()
        } else {
            stencil.body_len
        };

        // Copy stencil bytes.
        slice[offset..offset + copy_len].copy_from_slice(&stencil.bytes[..copy_len]);

        // Patch holes.
        for reloc in stencil.relocs.iter() {
            let bo = reloc.byte_offset as usize;
            if bo + 8 <= copy_len {
                let val = resolve_hole(&reloc.hole, fields);
                let val_bytes = val.to_le_bytes();
                slice[offset + bo..offset + bo + 8].copy_from_slice(&val_bytes);
            }
        }

        offset += copy_len;

        // XZR re-zero if needed.
        if i < last_idx && fields.rd == 31 {
            emit_xzr_rezero(slice, &mut offset);
        }
    }

    // Emit epilogue if last stencil is not a terminator.
    let (_, last_fields) = entries[last_idx];
    let last_stencil = entries[last_idx].0;
    if !last_stencil.is_terminator {
        // Update PC to next_pc before returning.
        // Emit: mov qword [rdi + PC_OFF], next_pc
        let pc_off = regs::reg_offset(regs::REG_PC);
        let pc_off_bytes = (pc_off as u32).to_le_bytes();
        let next_pc_bytes = last_fields.next_pc.to_le_bytes();
        // movabs rax, next_pc (10 bytes)
        slice[offset] = 0x48;
        slice[offset + 1] = 0xB8;
        slice[offset + 2..offset + 10].copy_from_slice(&next_pc_bytes);
        // mov [rdi + pc_off], rax (7 bytes)
        slice[offset + 10] = 0x48;
        slice[offset + 11] = 0x89;
        slice[offset + 12] = 0x87;
        slice[offset + 13..offset + 17].copy_from_slice(&pc_off_bytes);
        offset += 17;
        // mov rax, EXIT_END_OF_BLOCK; ret
        let exit_bytes = (EXIT_END_OF_BLOCK as u32).to_le_bytes();
        // mov eax, imm32 (5 bytes) — shorter encoding, zero-extends to rax
        slice[offset] = 0xB8;
        slice[offset + 1..offset + 5].copy_from_slice(&exit_bytes);
        offset += 5;
        // ret
        slice[offset] = 0xC3;
        offset += 1;
    }

    let _ = offset; // suppress unused

    // Enforce W^X: make executable, remove write.
    if !buf.make_executable() {
        return None;
    }

    let entry: JitBlockFn = unsafe { std::mem::transmute(buf.as_ptr()) };
    let insn_count = entries.len() as u32;

    Some(unsafe { CompiledBlock::new(buf, entry, pc, insn_count) })
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
        // A trivial stencil: just a NOP (0x90) — non-terminator.
        static NOP_BYTES: [u8; 1] = [0x90];
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
