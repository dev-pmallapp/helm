//! ARM64 Linux kernel Image loader.
//!
//! Parses the ARM64 kernel Image header and loads the kernel, DTB,
//! and optional initramfs into the simulator's physical memory.
//!
//! Reference: Documentation/arm64/booting.rst in the Linux source tree.

use crate::FlatMem;
use flate2::read::GzDecoder;
use std::io::Read;

/// ARM64 Image header magic: "ARM\x64" in little-endian = 0x644d5241.
const ARM64_IMAGE_MAGIC: u32 = 0x644d5241;

/// Loaded kernel metadata.
pub struct LoadedKernel {
    /// Entry point (kernel start address).
    pub entry: u64,
    /// Address where DTB was loaded.
    pub dtb_addr: u64,
    /// Address where initramfs was loaded (0 if none).
    pub initrd_addr: u64,
    /// Size of loaded initramfs.
    pub initrd_size: u64,
    /// Initial EL1 stack pointer.
    pub initial_sp: u64,
}

/// Parse an ARM64 Image header from raw bytes.
///
/// Returns (text_offset, image_size) on success.
fn parse_arm64_header(data: &[u8]) -> Result<(u64, u64), String> {
    if data.len() < 64 {
        return Err("Image too small for ARM64 header".into());
    }

    // Magic is at offset 56 (bytes 56-59)
    let magic = u32::from_le_bytes([data[56], data[57], data[58], data[59]]);
    if magic != ARM64_IMAGE_MAGIC {
        return Err(format!(
            "Invalid ARM64 Image magic: {magic:#010x} (expected {ARM64_IMAGE_MAGIC:#010x})"
        ));
    }

    // text_offset at offset 8 (8 bytes, LE)
    let text_offset = u64::from_le_bytes(data[8..16].try_into().unwrap());

    // image_size at offset 16 (8 bytes, LE)
    let image_size = u64::from_le_bytes(data[16..24].try_into().unwrap());

    Ok((text_offset, image_size))
}

/// Load an ARM64 kernel Image, DTB, and optional initramfs into memory.
///
/// Memory layout (QEMU virt style):
/// - `ram_base + text_offset`: kernel Image
/// - `ram_base + 128 MiB`: DTB
/// - `ram_base + 64 MiB`: initramfs (if provided)
///
/// Returns metadata for the loaded kernel.
pub fn load_arm64_kernel(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    mem: &mut FlatMem,
    ram_base: u64,
) -> Result<LoadedKernel, String> {
    // Read kernel image
    let raw_kernel_data = std::fs::read(kernel_path)
        .map_err(|e| format!("cannot read kernel {kernel_path}: {e}"))?;
    let kernel_data = try_decompress_zboot(&raw_kernel_data).unwrap_or(raw_kernel_data);

    // Parse header
    let (text_offset, image_size) = parse_arm64_header(&kernel_data)?;

    // Determine kernel load address
    // Per ARM64 boot protocol: if text_offset is 0, use 2MB alignment
    let kernel_offset = if text_offset == 0 { 0x0020_0000 } else { text_offset };
    let kernel_addr = ram_base + kernel_offset;
    let effective_image_size = if image_size == 0 {
        kernel_data.len() as u64
    } else {
        image_size
    };

    // Load kernel into memory
    mem.load_bytes(kernel_addr, &kernel_data);
    log::info!(
        "Loaded kernel {} ({} bytes) at {kernel_addr:#x}",
        kernel_path,
        kernel_data.len()
    );

    // Place DTB after the kernel image on a 2 MiB boundary.
    let dtb_addr = align_up(kernel_addr + effective_image_size, 0x0020_0000);
    let dtb_data = std::fs::read(dtb_path)
        .map_err(|e| format!("cannot read DTB {dtb_path}: {e}"))?;
    mem.load_bytes(dtb_addr, &dtb_data);
    log::info!(
        "Loaded DTB {} ({} bytes) at {dtb_addr:#x}",
        dtb_path,
        dtb_data.len()
    );

    // Load initramfs if provided
    let (initrd_addr, initrd_size) = if let Some(path) = initrd_path {
        let initrd_data = std::fs::read(path)
            .map_err(|e| format!("cannot read initrd {path}: {e}"))?;
        let addr = ram_base + 0x0400_0000; // 64 MiB offset
        mem.load_bytes(addr, &initrd_data);
        log::info!(
            "Loaded initramfs {} ({} bytes) at {addr:#x}",
            path,
            initrd_data.len()
        );
        (addr, initrd_data.len() as u64)
    } else {
        (0, 0)
    };

    let initial_sp = ram_base + 0x1000_0000; // 256 MiB scratch stack, matches old FS path

    Ok(LoadedKernel {
        entry: kernel_addr,
        dtb_addr,
        initrd_addr,
        initrd_size,
        initial_sp,
    })
}

fn align_up(addr: u64, alignment: u64) -> u64 {
    (addr + alignment - 1) & !(alignment - 1)
}

fn try_decompress_zboot(data: &[u8]) -> Option<Vec<u8>> {
    let has_mz = data.len() >= 2 && data[0] == 0x4D && data[1] == 0x5A;
    let has_arm64_magic = data.len() >= 0x3C
        && u32::from_le_bytes(data[0x38..0x3C].try_into().unwrap()) == ARM64_IMAGE_MAGIC;

    if !has_mz || has_arm64_magic {
        return None;
    }

    for offset in (0..data.len().saturating_sub(4)).step_by(4) {
        if data[offset] == 0x1f && data[offset + 1] == 0x8b && data[offset + 2] == 0x08 {
            let mut decoder = GzDecoder::new(&data[offset..]);
            let mut decompressed = Vec::new();
            if decoder.read_to_end(&mut decompressed).is_ok() && decompressed.len() > 0x40 {
                let magic = u32::from_le_bytes(decompressed[0x38..0x3C].try_into().unwrap_or([0; 4]));
                if magic == ARM64_IMAGE_MAGIC {
                    return Some(decompressed);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_arm64_header() {
        let mut header = vec![0u8; 64];
        // text_offset at offset 8
        let text_offset: u64 = 0x0020_0000;
        header[8..16].copy_from_slice(&text_offset.to_le_bytes());
        // image_size at offset 16
        let image_size: u64 = 0x0100_0000;
        header[16..24].copy_from_slice(&image_size.to_le_bytes());
        // magic at offset 56
        header[56..60].copy_from_slice(&ARM64_IMAGE_MAGIC.to_le_bytes());

        let (offset, size) = parse_arm64_header(&header).unwrap();
        assert_eq!(offset, 0x0020_0000);
        assert_eq!(size, 0x0100_0000);
    }

    #[test]
    fn reject_invalid_magic() {
        let header = vec![0u8; 64]; // All zeros — no magic
        let result = parse_arm64_header(&header);
        assert!(result.is_err());
    }
}
