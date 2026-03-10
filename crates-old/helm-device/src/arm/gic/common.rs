//! Shared GIC bitmap and priority helpers.

/// Check if a bit is set in a bitmap stored as `&[u32]`.
pub fn bitmap_is_set(bmap: &[u32], bit: u32) -> bool {
    let idx = (bit / 32) as usize;
    let off = bit % 32;
    idx < bmap.len() && bmap[idx] & (1 << off) != 0
}

/// Set a bit in a bitmap stored as `&mut [u32]`.
pub fn bitmap_set(bmap: &mut [u32], bit: u32) {
    let idx = (bit / 32) as usize;
    let off = bit % 32;
    if idx < bmap.len() {
        bmap[idx] |= 1 << off;
    }
}

/// Clear a bit in a bitmap stored as `&mut [u32]`.
pub fn bitmap_clear(bmap: &mut [u32], bit: u32) {
    let idx = (bit / 32) as usize;
    let off = bit % 32;
    if idx < bmap.len() {
        bmap[idx] &= !(1 << off);
    }
}

/// Read a u32 word from a bitmap at the given word index.
pub fn bitmap_read_word(bmap: &[u32], word_idx: usize) -> u32 {
    bmap.get(word_idx).copied().unwrap_or(0)
}

/// OR a value into a bitmap word (set-enable style).
pub fn bitmap_or_word(bmap: &mut [u32], word_idx: usize, value: u32) {
    if let Some(w) = bmap.get_mut(word_idx) {
        *w |= value;
    }
}

/// AND-NOT a value from a bitmap word (clear-enable style).
pub fn bitmap_andnot_word(bmap: &mut [u32], word_idx: usize, value: u32) {
    if let Some(w) = bmap.get_mut(word_idx) {
        *w &= !value;
    }
}

/// Read 4 bytes packed into a `u32` from a byte array.
pub fn byte_array_read4(arr: &[u8], base: usize) -> u32 {
    let mut val = 0u32;
    for i in 0..4 {
        if base + i < arr.len() {
            val |= (arr[base + i] as u32) << (i * 8);
        }
    }
    val
}

/// Write 4 bytes from a `u32` into a byte array.
pub fn byte_array_write4(arr: &mut [u8], base: usize, value: u32) {
    for i in 0..4 {
        if base + i < arr.len() {
            arr[base + i] = (value >> (i * 8)) as u8;
        }
    }
}

/// Find the highest-priority (lowest numeric value) pending+enabled
/// IRQ within a range, subject to a priority mask.
pub fn highest_pending_in_range(
    pending: &[u32],
    enabled: &[u32],
    priority: &[u8],
    priority_mask: u8,
    range: std::ops::Range<u32>,
) -> Option<u32> {
    let mut best: Option<(u32, u8)> = None;
    for irq in range {
        if bitmap_is_set(pending, irq) && bitmap_is_set(enabled, irq) {
            let prio = priority.get(irq as usize).copied().unwrap_or(0xFF);
            if prio < priority_mask && best.is_none_or(|(_, bp)| prio < bp) {
                best = Some((irq, prio));
            }
        }
    }
    best.map(|(irq, _)| irq)
}

/// Spurious interrupt ID returned when no interrupt is pending.
pub const SPURIOUS_IRQ: u32 = 1023;

/// Maximum IRQ count (GICv2/v3 SPI limit).
pub const MAX_IRQS: u32 = 1020;

/// SGI range: 0-15.
pub const SGI_END: u32 = 16;

/// PPI range: 16-31.
pub const PPI_END: u32 = 32;

/// SPI start (INTID 32).
pub const SPI_START: u32 = 32;

/// LPI start (INTID 8192, GICv3+).
pub const LPI_START: u32 = 8192;

/// Check whether `offset` falls within `[base, base+size)`.
pub fn in_range(offset: u64, base: u64, size: u64) -> bool {
    offset >= base && offset < base + size
}
