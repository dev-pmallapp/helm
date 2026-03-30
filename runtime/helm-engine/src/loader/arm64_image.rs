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

// ── FDT bootargs patcher ──────────────────────────────────────────────────────

/// Precedence: `--append` > DTB `chosen/bootargs` > kernel built-in cmdline.
///
/// If `append` is `Some`, we patch the FDT in-place:
/// - Find the `/chosen` node's `bootargs` property.
/// - Replace its value with the new string (null-terminated, 4-byte aligned).
/// - If the new value is shorter, pad with nulls and update the property length.
/// - If longer, return an error — in-place expansion would require rewriting
///   the entire structure block and is not worth the complexity here.
///   Use `boot_rpi_full.py` (which calls `dtc`) for that case.
fn patch_dtb_bootargs(dtb: &mut Vec<u8>, append: &str) -> Result<(), String> {
    // FDT layout (all big-endian):
    //   0x00  magic          u32  0xD00DFEED
    //   0x04  totalsize      u32
    //   0x08  off_dt_struct  u32
    //   0x0C  off_dt_strings u32
    //   0x10  off_mem_rsvmap u32
    //   ...
    //   Structure block: sequence of tokens (u32 BE):
    //     FDT_BEGIN_NODE  = 1   followed by node name (null-terminated, aligned)
    //     FDT_END_NODE    = 2
    //     FDT_PROP        = 3   followed by: len(u32), nameoff(u32), data(len bytes, aligned)
    //     FDT_NOP         = 4
    //     FDT_END         = 9

    const FDT_MAGIC: u32 = 0xD00D_FEED;
    const FDT_BEGIN_NODE: u32 = 1;
    const FDT_END_NODE: u32 = 2;
    const FDT_PROP: u32 = 3;
    const FDT_NOP: u32 = 4;
    const FDT_END: u32 = 9;

    if dtb.len() < 0x28 {
        return Err("DTB too small".into());
    }

    let magic = u32::from_be_bytes(dtb[0..4].try_into().unwrap());
    if magic != FDT_MAGIC {
        return Err(format!("Not a valid FDT (magic={magic:#010x})"));
    }

    let off_struct = u32::from_be_bytes(dtb[0x08..0x0C].try_into().unwrap()) as usize;
    let off_strings = u32::from_be_bytes(dtb[0x0C..0x10].try_into().unwrap()) as usize;

    // Find "bootargs" offset in the strings block
    let bootargs_str = b"bootargs\0";
    let strings_block = &dtb[off_strings..];
    let bootargs_nameoff = strings_block
        .windows(bootargs_str.len())
        .position(|w| w == bootargs_str)
        .ok_or("DTB strings block has no 'bootargs' property name")?;

    // Walk the structure block looking for /chosen node then its bootargs property
    let mut pos = off_struct;
    let mut depth: i32 = 0;
    let mut in_chosen = false;

    loop {
        if pos + 4 > dtb.len() {
            return Err("FDT structure block overrun".into());
        }
        let token = u32::from_be_bytes(dtb[pos..pos + 4].try_into().unwrap());
        pos += 4;

        match token {
            FDT_BEGIN_NODE => {
                // Node name: null-terminated, then aligned to 4 bytes
                let name_start = pos;
                while pos < dtb.len() && dtb[pos] != 0 {
                    pos += 1;
                }
                let name = std::str::from_utf8(&dtb[name_start..pos]).unwrap_or("");
                pos += 1; // consume null
                pos = (pos + 3) & !3; // align

                if depth == 1 && name == "chosen" {
                    in_chosen = true;
                } else if depth != 0 || name.is_empty() {
                    // depth 0 is the root node (empty name)
                }
                depth += 1;
            }
            FDT_END_NODE => {
                depth -= 1;
                if in_chosen && depth == 1 {
                    in_chosen = false;
                }
            }
            FDT_PROP => {
                if pos + 8 > dtb.len() {
                    return Err("FDT PROP token truncated".into());
                }
                let prop_len = u32::from_be_bytes(dtb[pos..pos + 4].try_into().unwrap()) as usize;
                let nameoff =
                    u32::from_be_bytes(dtb[pos + 4..pos + 8].try_into().unwrap()) as usize;
                let data_start = pos + 8;
                let data_end = data_start + prop_len;
                let aligned_end = (data_end + 3) & !3;
                pos = aligned_end;

                if in_chosen && nameoff == bootargs_nameoff {
                    // Found bootargs property. Replace in-place if it fits.
                    let new_val = format!("{append}\0");
                    let new_bytes = new_val.as_bytes();

                    if new_bytes.len() <= prop_len {
                        // Fits: overwrite value, zero-pad remainder, update len field.
                        dtb[data_start..data_start + new_bytes.len()].copy_from_slice(new_bytes);
                        dtb[data_start + new_bytes.len()..data_end].fill(0);
                        // FDT_PROP layout: token(4) + len(4) + nameoff(4) + data
                        // len field is at data_start - 8.
                        let len_off = data_start - 8;
                        dtb[len_off..len_off + 4]
                            .copy_from_slice(&(new_bytes.len() as u32).to_be_bytes());
                        return Ok(());
                    } else {
                        return Err(format!(
                            "--append string ({} bytes) is longer than existing DTB bootargs \
                             ({prop_len} bytes). Use a pre-built DTB with a larger bootargs property.",
                            new_bytes.len()
                        ));
                    }
                }
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => {
                return Err(format!(
                    "Unknown FDT token {token:#x} at offset {}",
                    pos - 4
                ))
            }
        }
    }

    Err("DTB has no /chosen bootargs property to patch. Add one to your DTB.".into())
}

fn load_arm64_kernel_common(
    kernel_path: &str,
    dtb_label: &str,
    mut dtb_data: Vec<u8>,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem: &mut FlatMem,
    ram_base: u64,
) -> Result<LoadedKernel, String> {
    // Read kernel image
    let raw_kernel_data =
        std::fs::read(kernel_path).map_err(|e| format!("cannot read kernel {kernel_path}: {e}"))?;
    let kernel_data = try_decompress_zboot(&raw_kernel_data).unwrap_or(raw_kernel_data);

    // Parse header
    let (text_offset, image_size) = parse_arm64_header(&kernel_data)?;

    // Determine kernel load address
    // Per ARM64 boot protocol: if text_offset is 0, use 2MB alignment
    let kernel_offset = if text_offset == 0 {
        0x0020_0000
    } else {
        text_offset
    };
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
    // Apply --append override (highest precedence).
    if let Some(cmdline) = append {
        if !cmdline.is_empty() {
            patch_dtb_bootargs(&mut dtb_data, cmdline)?;
            log::info!("Patched DTB bootargs with: {cmdline}");
        }
    }

    mem.load_bytes(dtb_addr, &dtb_data);
    log::info!(
        "Loaded DTB {} ({} bytes) at {dtb_addr:#x}",
        dtb_label,
        dtb_data.len()
    );

    // Load initramfs if provided
    let (initrd_addr, initrd_size) = if let Some(path) = initrd_path {
        let initrd_data =
            std::fs::read(path).map_err(|e| format!("cannot read initrd {path}: {e}"))?;
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

/// Load an ARM64 kernel Image, DTB, and optional initramfs into memory.
///
/// Memory layout (QEMU virt style):
/// - `ram_base + text_offset`: kernel Image
/// - `ram_base + 128 MiB`: DTB
/// - `ram_base + 64 MiB`: initramfs (if provided)
///
/// `append`: when `Some`, overrides the DTB `/chosen/bootargs` property
/// (highest precedence — beats DTB bootargs and kernel built-in cmdline).
///
/// Returns metadata for the loaded kernel.
pub fn load_arm64_kernel(
    kernel_path: &str,
    dtb_path: &str,
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem: &mut FlatMem,
    ram_base: u64,
) -> Result<LoadedKernel, String> {
    let dtb_data =
        std::fs::read(dtb_path).map_err(|e| format!("cannot read DTB {dtb_path}: {e}"))?;
    load_arm64_kernel_common(
        kernel_path,
        dtb_path,
        dtb_data,
        initrd_path,
        append,
        mem,
        ram_base,
    )
}

/// Load an ARM64 kernel Image with an in-memory DTB blob.
pub fn load_arm64_kernel_with_dtb_bytes(
    kernel_path: &str,
    dtb_data: &[u8],
    initrd_path: Option<&str>,
    append: Option<&str>,
    mem: &mut FlatMem,
    ram_base: u64,
) -> Result<LoadedKernel, String> {
    load_arm64_kernel_common(
        kernel_path,
        "<dtb-bytes>",
        dtb_data.to_vec(),
        initrd_path,
        append,
        mem,
        ram_base,
    )
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
                let magic =
                    u32::from_le_bytes(decompressed[0x38..0x3C].try_into().unwrap_or([0; 4]));
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
