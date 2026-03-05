//! ELF64 loader for AArch64 SE mode.

use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;

// Auxiliary vector tags
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_CLKTCK: u64 = 17;
const AT_RANDOM: u64 = 25;

/// Result of loading an ELF binary.
pub struct LoadedBinary {
    pub entry_point: Addr,
    pub address_space: AddressSpace,
    pub initial_sp: Addr,
    pub phdr_addr: Addr,
    pub phent: u16,
    pub phnum: u16,
}

/// Load a static AArch64 ELF64 binary.
pub fn load_elf(path: &str, argv: &[&str], envp: &[&str]) -> HelmResult<LoadedBinary> {
    let data = std::fs::read(path)?;

    // Validate ELF header
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return Err(helm_core::HelmError::Config("not an ELF file".into()));
    }
    if data[4] != 2 {
        return Err(helm_core::HelmError::Config("not ELF64".into()));
    }
    if data[5] != 1 {
        return Err(helm_core::HelmError::Config("not little-endian".into()));
    }
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    if e_machine != EM_AARCH64 {
        return Err(helm_core::HelmError::Config(format!(
            "not AArch64 (machine={e_machine})"
        )));
    }

    let e_entry = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes([data[54], data[55]]);
    let e_phnum = u16::from_le_bytes([data[56], data[57]]);

    let mut address_space = AddressSpace::new();
    let mut phdr_addr: Addr = 0;

    // Load PT_LOAD segments
    for i in 0..e_phnum as usize {
        let ph = e_phoff + i * e_phentsize as usize;
        let p_type = u32::from_le_bytes(data[ph..ph + 4].try_into().unwrap());
        if p_type != PT_LOAD {
            continue;
        }

        let p_flags = u32::from_le_bytes(data[ph + 4..ph + 8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(data[ph + 8..ph + 16].try_into().unwrap()) as usize;
        let p_vaddr = u64::from_le_bytes(data[ph + 16..ph + 24].try_into().unwrap());
        let p_filesz = u64::from_le_bytes(data[ph + 32..ph + 40].try_into().unwrap()) as usize;
        let p_memsz = u64::from_le_bytes(data[ph + 40..ph + 48].try_into().unwrap()) as usize;

        let readable = p_flags & 4 != 0;
        let writable = p_flags & 2 != 0;
        let executable = p_flags & 1 != 0;

        // Page-align
        let page_mask: u64 = 0xFFF;
        let aligned_vaddr = p_vaddr & !page_mask;
        let offset_in_page = (p_vaddr - aligned_vaddr) as usize;
        let map_size = align_up(offset_in_page + p_memsz, 0x1000);

        address_space.map(
            aligned_vaddr,
            map_size as u64,
            (readable, writable, executable),
        );

        // Copy file data
        if p_filesz > 0 && p_offset + p_filesz <= data.len() {
            address_space.write(p_vaddr, &data[p_offset..p_offset + p_filesz])?;
        }
        // .bss is already zero-filled from map()

        // Record where the program headers are loaded (for AT_PHDR)
        if i == 0 {
            phdr_addr = aligned_vaddr + e_phoff as u64;
        }
    }

    // Setup stack
    let stack_top: Addr = 0x7FFF_FFE0_0000;
    let stack_size: u64 = 8 * 1024 * 1024; // 8 MB
    let stack_base = stack_top - stack_size;
    address_space.map(stack_base, stack_size, (true, true, false));
    // Read-only guard page above the stack so unaligned wide reads
    // near the top (e.g. 8-byte strlen) don't fault.
    address_space.map(stack_top, 0x1000, (true, false, false));

    let initial_sp = build_stack(
        &mut address_space,
        stack_top,
        argv,
        envp,
        &ElfInfo {
            entry: e_entry,
            phdr: phdr_addr,
            phent: e_phentsize,
            phnum: e_phnum,
        },
    )?;

    Ok(LoadedBinary {
        entry_point: e_entry,
        address_space,
        initial_sp,
        phdr_addr,
        phent: e_phentsize,
        phnum: e_phnum,
    })
}

/// Build the initial stack: argc, argv ptrs, envp ptrs, auxv, strings.
struct ElfInfo {
    entry: Addr,
    phdr: Addr,
    phent: u16,
    phnum: u16,
}

fn build_stack(
    mem: &mut AddressSpace,
    stack_top: Addr,
    argv: &[&str],
    envp: &[&str],
    elf: &ElfInfo,
) -> HelmResult<Addr> {
    let entry = elf.entry;
    let phdr = elf.phdr;
    let phent = elf.phent;
    let phnum = elf.phnum;
    let mut sp = stack_top;

    // Helper: push bytes, return address
    // Push bytes onto stack, return address
    fn push_bytes(sp: &mut u64, mem: &mut AddressSpace, b: &[u8]) -> HelmResult<Addr> {
        *sp -= b.len() as u64;
        mem.write(*sp, b)?;
        Ok(*sp)
    }
    fn push_u64(sp: &mut u64, mem: &mut AddressSpace, val: u64) -> HelmResult<()> {
        *sp -= 8;
        mem.write(*sp, &val.to_le_bytes())
    }

    // 1. Write string data
    let mut argv_addrs = Vec::new();
    for arg in argv {
        let mut s = arg.as_bytes().to_vec();
        s.push(0);
        let addr = push_bytes(&mut sp, mem, &s)?;
        argv_addrs.push(addr);
    }

    let mut envp_addrs = Vec::new();
    for env in envp {
        let mut s = env.as_bytes().to_vec();
        s.push(0);
        let addr = push_bytes(&mut sp, mem, &s)?;
        envp_addrs.push(addr);
    }

    let at_random_addr = push_bytes(&mut sp, mem, &[0u8; 16])?;

    sp &= !0xF;

    // 2. Compute total u64 slots to determine alignment padding.
    //    AArch64 ABI requires SP to be 16-byte aligned, so total
    //    slots (each 8 bytes) must be even.
    let auxv_count = 12; // AT_PHDR..AT_RANDOM + AT_NULL
    let auxv_slots = auxv_count * 2;
    let total_slots = auxv_slots
        + 1 // envp NULL terminator
        + envp.len()
        + 1 // argv NULL terminator
        + argv.len()
        + 1; // argc
    let need_padding = !total_slots.is_multiple_of(2);

    // 3. Auxiliary vector
    // Push padding above AT_NULL if needed (harmless — nobody reads past
    // the AT_NULL terminator).
    if need_padding {
        push_u64(&mut sp, mem, 0)?;
    }

    push_u64(&mut sp, mem, 0)?;
    push_u64(&mut sp, mem, AT_NULL)?;
    push_u64(&mut sp, mem, at_random_addr)?;
    push_u64(&mut sp, mem, AT_RANDOM)?;
    push_u64(&mut sp, mem, 100)?;
    push_u64(&mut sp, mem, AT_CLKTCK)?;
    push_u64(&mut sp, mem, 1000)?;
    push_u64(&mut sp, mem, AT_EGID)?;
    push_u64(&mut sp, mem, 1000)?;
    push_u64(&mut sp, mem, AT_GID)?;
    push_u64(&mut sp, mem, 1000)?;
    push_u64(&mut sp, mem, AT_EUID)?;
    push_u64(&mut sp, mem, 1000)?;
    push_u64(&mut sp, mem, AT_UID)?;
    push_u64(&mut sp, mem, entry)?;
    push_u64(&mut sp, mem, AT_ENTRY)?;
    push_u64(&mut sp, mem, 4096)?;
    push_u64(&mut sp, mem, AT_PAGESZ)?;
    push_u64(&mut sp, mem, phnum as u64)?;
    push_u64(&mut sp, mem, AT_PHNUM)?;
    push_u64(&mut sp, mem, phent as u64)?;
    push_u64(&mut sp, mem, AT_PHENT)?;
    push_u64(&mut sp, mem, phdr)?;
    push_u64(&mut sp, mem, AT_PHDR)?;

    // 4. envp array
    push_u64(&mut sp, mem, 0)?;
    for addr in envp_addrs.iter().rev() {
        push_u64(&mut sp, mem, *addr)?;
    }

    // 5. argv array
    push_u64(&mut sp, mem, 0)?;
    for addr in argv_addrs.iter().rev() {
        push_u64(&mut sp, mem, *addr)?;
    }

    // 6. argc
    push_u64(&mut sp, mem, argv.len() as u64)?;

    debug_assert!(sp.is_multiple_of(16), "SP must be 16-byte aligned, got {sp:#x}");

    Ok(sp)
}

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}
