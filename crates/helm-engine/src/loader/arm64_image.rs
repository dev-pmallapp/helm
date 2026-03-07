//! ARM64 Linux kernel Image loader.
//!
//! Loads the standard `Image` format used by AArch64 Linux kernels.
//! This is the format produced by `make Image` and found in Alpine,
//! Raspberry Pi OS, and other distro kernel packages.
//!
//! # Image header (from Linux Documentation/arch/arm64/booting.rst)
//!
//! ```text
//! Offset  Size  Field
//! 0x00    4     code0        (branch to kernel entry, or MZ for PE)
//! 0x04    4     code1
//! 0x08    8     text_offset  (image load offset from start of RAM)
//! 0x10    8     image_size   (effective size of image, LE)
//! 0x18    8     flags        (bit 0: kernel endianness, bits 1-2: page size)
//! 0x20    8     res2         (reserved)
//! 0x28    8     res3         (reserved)
//! 0x30    8     res4         (reserved)
//! 0x38    4     magic        (0x644d5241 = "ARMd" LE — ARM\x64)
//! 0x3C    4     res5         (PE/COFF offset or 0)
//! ```
//!
//! # Boot protocol
//!
//! 1. Load Image at RAM_BASE + text_offset (or 2MB-aligned address)
//! 2. Load DTB at a 2MB-aligned address (must not overlap kernel)
//! 3. Optionally load initramfs and tell kernel via DTB chosen node
//! 4. CPU registers: x0 = DTB physical address, x1-x3 = 0
//! 5. Jump to kernel entry = load address (first instruction)

use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

/// Magic bytes at offset 0x38 identifying an ARM64 Image.
const ARM64_IMAGE_MAGIC: u32 = 0x644d_5241; // "ARMd" in LE

/// Default RAM base for the kernel.
const DEFAULT_RAM_BASE: Addr = 0x4000_0000;

/// Default DTB load address (128 MB into RAM, 2MB-aligned).
const DEFAULT_DTB_ADDR: Addr = DEFAULT_RAM_BASE + 0x0800_0000;

/// Default initramfs address (64 MB into RAM).
const DEFAULT_INITRD_ADDR: Addr = DEFAULT_RAM_BASE + 0x0400_0000;

/// Parsed ARM64 Image header.
#[derive(Debug)]
pub struct Arm64ImageHeader {
    /// Branch instruction or MZ stub.
    pub code0: u32,
    /// Load offset from start of RAM.
    pub text_offset: u64,
    /// Effective image size in bytes.
    pub image_size: u64,
    /// Flags: bit 0 = LE(0)/BE(1), bits 1-2 = page size.
    pub flags: u64,
}

/// Result of loading an ARM64 kernel image for FS mode.
pub struct LoadedKernel {
    /// Address space with kernel, DTB, and initramfs loaded.
    pub address_space: AddressSpace,
    /// Kernel entry point (= load address).
    pub entry_point: Addr,
    /// Address where DTB was loaded (goes in x0).
    pub dtb_addr: Addr,
    /// Stack pointer (top of a 64KB scratch stack).
    pub initial_sp: Addr,
    /// RAM base address.
    pub ram_base: Addr,
    /// Kernel load address.
    pub kernel_addr: Addr,
    /// Size of loaded kernel image.
    pub kernel_size: u64,
    /// Initramfs address (0 if none).
    pub initrd_addr: Addr,
    /// Initramfs size (0 if none).
    pub initrd_size: u64,
}

/// Parse the ARM64 Image header.
pub fn parse_arm64_header(data: &[u8]) -> HelmResult<Arm64ImageHeader> {
    if data.len() < 0x40 {
        return Err(helm_core::HelmError::Config("image too small for ARM64 header".into()));
    }

    let magic = u32::from_le_bytes(data[0x38..0x3C].try_into().unwrap());
    if magic != ARM64_IMAGE_MAGIC {
        // Check for MZ header (PE/COFF stub) — still valid ARM64 Image
        let mz = u16::from_le_bytes(data[0..2].try_into().unwrap());
        if mz != 0x5A4D {
            return Err(helm_core::HelmError::Config(format!(
                "not an ARM64 Image (magic={magic:#010x}, expected {ARM64_IMAGE_MAGIC:#010x})"
            )));
        }
    }

    let code0 = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let text_offset = u64::from_le_bytes(data[0x08..0x10].try_into().unwrap());
    let image_size = u64::from_le_bytes(data[0x10..0x18].try_into().unwrap());
    let flags = u64::from_le_bytes(data[0x18..0x20].try_into().unwrap());

    Ok(Arm64ImageHeader {
        code0,
        text_offset,
        image_size,
        flags,
    })
}

/// Load an ARM64 kernel Image into an address space.
///
/// # Parameters
/// - `kernel_path`: Path to the vmlinuz/Image file
/// - `dtb_path`: Optional path to the .dtb file
/// - `initrd_path`: Optional path to the initramfs
/// - `ram_base`: Base address of RAM (default 0x4000_0000)
pub fn load_arm64_image(
    kernel_path: &str,
    dtb_path: Option<&str>,
    initrd_path: Option<&str>,
    ram_base: Option<Addr>,
) -> HelmResult<LoadedKernel> {
    let kernel_data = std::fs::read(kernel_path)?;
    let header = parse_arm64_header(&kernel_data)?;

    let ram = ram_base.unwrap_or(DEFAULT_RAM_BASE);

    // Kernel load address: RAM base + text_offset (2MB-aligned)
    let text_offset = if header.text_offset == 0 {
        0x8_0000 // 512KB default if header says 0
    } else {
        header.text_offset
    };
    let kernel_addr = ram + text_offset;

    // Image size from header (or actual file size)
    let image_size = if header.image_size == 0 {
        kernel_data.len() as u64
    } else {
        header.image_size
    };

    let mut address_space = AddressSpace::new();

    // Map RAM region (1 GB)
    let ram_size: u64 = 0x4000_0000; // 1 GB
    address_space.map(ram, ram_size, (true, true, true));

    // Load kernel image into RAM
    let load_end = kernel_addr + kernel_data.len() as u64;
    log::info!(
        "Loading ARM64 kernel: {} ({} bytes) at {:#x}..{:#x}",
        kernel_path, kernel_data.len(), kernel_addr, load_end
    );
    address_space.write(kernel_addr, &kernel_data)?;

    // Load DTB if provided
    let dtb_addr = if let Some(dtb_path) = dtb_path {
        let dtb_data = std::fs::read(dtb_path)?;
        // Place DTB at a 2MB-aligned address after the kernel
        let addr = align_up(load_end, 0x20_0000);
        log::info!(
            "Loading DTB: {} ({} bytes) at {:#x}",
            dtb_path, dtb_data.len(), addr
        );
        address_space.write(addr, &dtb_data)?;
        addr
    } else {
        // No DTB — use default address with a minimal stub
        DEFAULT_DTB_ADDR
    };

    // Load initramfs if provided
    let (initrd_addr, initrd_size) = if let Some(initrd_path) = initrd_path {
        let initrd_data = std::fs::read(initrd_path)?;
        let addr = DEFAULT_INITRD_ADDR;
        log::info!(
            "Loading initramfs: {} ({} bytes) at {:#x}",
            initrd_path, initrd_data.len(), addr
        );
        address_space.write(addr, &initrd_data)?;
        (addr, initrd_data.len() as u64)
    } else {
        (0, 0)
    };

    // Stack: 64KB at top of first 256MB of RAM
    let sp = ram + 0x1000_0000; // 256 MB into RAM
    address_space.map(sp - 0x1_0000, 0x1_0000, (true, true, false)); // 64KB stack

    log::info!(
        "ARM64 boot: entry={:#x} dtb={:#x} sp={:#x}",
        kernel_addr, dtb_addr, sp
    );

    Ok(LoadedKernel {
        address_space,
        entry_point: kernel_addr,
        dtb_addr,
        initial_sp: sp,
        ram_base: ram,
        kernel_addr,
        kernel_size: image_size,
        initrd_addr,
        initrd_size,
    })
}

fn align_up(addr: u64, alignment: u64) -> u64 {
    (addr + alignment - 1) & !(alignment - 1)
}
