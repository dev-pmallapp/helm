//! ELF64 loader for AArch64 SE mode.
//!
//! Parses a statically-linked AArch64 ELF binary, loads all PT_LOAD segments
//! into a `FlatMem`, and builds the initial stack with argc/argv/envp/auxv.
//!
//! Adapted from the reference implementation in `helm.git`.

use crate::FlatMem;

const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;

// Auxiliary vector tags
const AT_NULL:   u64 = 0;
const AT_PHDR:   u64 = 3;
const AT_PHENT:  u64 = 4;
const AT_PHNUM:  u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE:   u64 = 7;
const AT_FLAGS:  u64 = 8;
const AT_ENTRY:  u64 = 9;
const AT_UID:    u64 = 11;
const AT_EUID:   u64 = 12;
const AT_GID:    u64 = 13;
const AT_EGID:   u64 = 14;
const AT_HWCAP:  u64 = 16;
const AT_CLKTCK: u64 = 17;
const AT_RANDOM: u64 = 25;

/// Result of loading an ELF binary into `FlatMem`.
pub struct LoadedBinary {
    /// Entry point virtual address (e_entry).
    pub entry_point: u64,
    /// Initial stack pointer (after argc/argv/envp/auxv are pushed).
    pub initial_sp:  u64,
    /// Virtual address where the program headers are loaded (for AT_PHDR).
    pub phdr_addr:   u64,
    /// e_phentsize
    pub phent: u16,
    /// e_phnum
    pub phnum: u16,
    /// First page-aligned address after all loaded segments (initial brk).
    pub brk_base: u64,
}

/// Load a static AArch64 ELF64 binary into `mem`.
///
/// `mem` must be large enough to contain all PT_LOAD segments and the stack.
/// Stack is placed at a fixed high address (0x7FFF_FFE0_0000 downward).
///
/// # Errors
/// Returns a human-readable error string on malformed or unsupported ELF.
pub fn load_elf(
    path: &str,
    argv: &[&str],
    envp: &[&str],
    mem: &mut FlatMem,
) -> Result<LoadedBinary, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;

    // ── ELF header validation ─────────────────────────────────────────────────
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    if data[4] != 2 {
        return Err("not ELF64 (class != 2)".into());
    }
    if data[5] != 1 {
        return Err("not little-endian (data encoding != 1)".into());
    }
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    if e_machine != EM_AARCH64 {
        return Err(format!("not AArch64 (e_machine={e_machine}, expected 183)"));
    }

    let e_entry    = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let e_phoff    = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes([data[54], data[55]]);
    let e_phnum    = u16::from_le_bytes([data[56], data[57]]);

    // ── Load PT_LOAD segments ─────────────────────────────────────────────────
    let mut phdr_addr: u64 = 0;
    let mut highest_addr: u64 = 0;

    for i in 0..e_phnum as usize {
        let ph = e_phoff + i * e_phentsize as usize;
        if ph + 56 > data.len() { break; }

        let p_type   = u32::from_le_bytes(data[ph..ph + 4].try_into().unwrap());
        if p_type != PT_LOAD { continue; }

        let p_offset = u64::from_le_bytes(data[ph + 8..ph + 16].try_into().unwrap()) as usize;
        let p_vaddr  = u64::from_le_bytes(data[ph + 16..ph + 24].try_into().unwrap());
        let p_filesz = u64::from_le_bytes(data[ph + 32..ph + 40].try_into().unwrap()) as usize;
        let p_memsz  = u64::from_le_bytes(data[ph + 40..ph + 48].try_into().unwrap()) as usize;

        // FlatMem is already zero-filled; just copy the file data.
        if p_filesz > 0 {
            let end = p_offset.checked_add(p_filesz)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| format!("PT_LOAD segment {i} out of bounds"))?;
            mem.load_bytes(p_vaddr, &data[p_offset..end]);
        }

        // Record program header location (first segment, for AT_PHDR)
        if i == 0 {
            phdr_addr = p_vaddr + e_phoff as u64;
        }

        // Track highest loaded VA for brk placement
        let seg_end = p_vaddr + p_memsz as u64;
        if seg_end > highest_addr {
            highest_addr = seg_end;
        }
    }

    // ── Build initial stack ───────────────────────────────────────────────────
    let stack_top: u64 = 0x7FFF_FFE0_0000;
    let initial_sp = build_stack(
        mem,
        stack_top,
        argv,
        envp,
        e_entry,
        phdr_addr,
        e_phentsize,
        e_phnum,
    );

    let brk_base = (highest_addr + 0xFFF) & !0xFFF;

    Ok(LoadedBinary {
        entry_point: e_entry,
        initial_sp,
        phdr_addr,
        phent: e_phentsize,
        phnum: e_phnum,
        brk_base,
    })
}

/// Push `bytes` onto the stack (pre-decrement SP), return the address written.
fn push_bytes(mem: &mut FlatMem, sp: &mut u64, bytes: &[u8]) -> u64 {
    *sp -= bytes.len() as u64;
    let addr = *sp;
    mem.load_bytes(addr, bytes);
    addr
}

/// Push one 8-byte little-endian value onto the stack (pre-decrement SP).
fn push_u64(mem: &mut FlatMem, sp: &mut u64, val: u64) {
    *sp -= 8;
    mem.load_bytes(*sp, &val.to_le_bytes());
}

/// Push argc / argv / envp / auxv onto the initial stack, return the new SP.
///
/// AArch64 ABI requires SP 16-byte aligned on entry.
#[allow(clippy::too_many_arguments)]
fn build_stack(
    mem: &mut FlatMem,
    stack_top: u64,
    argv: &[&str],
    envp: &[&str],
    entry: u64,
    phdr: u64,
    phent: u16,
    phnum: u16,
) -> u64 {
    let mut sp = stack_top;

    // 1. Push string data for argv
    let mut argv_addrs = Vec::with_capacity(argv.len());
    for arg in argv {
        let mut s = arg.as_bytes().to_vec();
        s.push(0);
        let addr = push_bytes(mem, &mut sp, &s);
        argv_addrs.push(addr);
    }

    // 2. Push string data for envp
    let mut envp_addrs = Vec::with_capacity(envp.len());
    for env in envp {
        let mut s = env.as_bytes().to_vec();
        s.push(0);
        let addr = push_bytes(mem, &mut sp, &s);
        envp_addrs.push(addr);
    }

    // 3. AT_RANDOM: 16-byte pseudo-random seed
    let at_random_addr = push_bytes(mem, &mut sp, &[0x5Eu8; 16]);

    // Align SP to 16 before pushing u64 values
    sp &= !0xF;

    // 4. Compute total 8-byte slots for alignment check
    let auxv_pairs = 14u64; // 14 AT_ pairs + AT_NULL pair = 15 pairs below
    let total_slots = 1  // argc
        + argv.len() as u64 + 1   // argv ptrs + null
        + envp.len() as u64 + 1   // envp ptrs + null
        + (auxv_pairs + 1) * 2;   // auxv pairs (key+val) + AT_NULL pair
    // Pad so that total_slots is even (SP stays 16-aligned after all pushes)
    if total_slots % 2 != 0 {
        push_u64(mem, &mut sp, 0);
    }

    // 5. Auxiliary vector (AT_NULL terminates)
    push_u64(mem, &mut sp, 0);             push_u64(mem, &mut sp, AT_NULL);
    push_u64(mem, &mut sp, at_random_addr); push_u64(mem, &mut sp, AT_RANDOM);
    push_u64(mem, &mut sp, 0x3);           push_u64(mem, &mut sp, AT_HWCAP);
    push_u64(mem, &mut sp, 100);           push_u64(mem, &mut sp, AT_CLKTCK);
    push_u64(mem, &mut sp, 0);             push_u64(mem, &mut sp, AT_FLAGS);
    push_u64(mem, &mut sp, 0);             push_u64(mem, &mut sp, AT_BASE);
    push_u64(mem, &mut sp, 1000);          push_u64(mem, &mut sp, AT_EGID);
    push_u64(mem, &mut sp, 1000);          push_u64(mem, &mut sp, AT_GID);
    push_u64(mem, &mut sp, 1000);          push_u64(mem, &mut sp, AT_EUID);
    push_u64(mem, &mut sp, 1000);          push_u64(mem, &mut sp, AT_UID);
    push_u64(mem, &mut sp, entry);         push_u64(mem, &mut sp, AT_ENTRY);
    push_u64(mem, &mut sp, 4096);          push_u64(mem, &mut sp, AT_PAGESZ);
    push_u64(mem, &mut sp, phnum as u64);  push_u64(mem, &mut sp, AT_PHNUM);
    push_u64(mem, &mut sp, phent as u64);  push_u64(mem, &mut sp, AT_PHENT);
    push_u64(mem, &mut sp, phdr);          push_u64(mem, &mut sp, AT_PHDR);

    // 6. envp array (null-terminated)
    push_u64(mem, &mut sp, 0);
    for addr in envp_addrs.iter().rev() {
        push_u64(mem, &mut sp, *addr);
    }

    // 7. argv array (null-terminated)
    push_u64(mem, &mut sp, 0);
    for addr in argv_addrs.iter().rev() {
        push_u64(mem, &mut sp, *addr);
    }

    // 8. argc
    push_u64(mem, &mut sp, argv.len() as u64);

    sp
}
