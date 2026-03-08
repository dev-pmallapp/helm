#!/usr/bin/env python3
"""Survey hot kernel functions to identify instruction patterns.

Examines blake2s_compress_generic and decompression code to determine
what instruction classes they use (ALU, SIMD, crypto, etc.)
"""
import struct
import sys

# AArch64 instruction decoder (minimal — just classifies groups)
def classify_insn(word):
    """Classify an AArch64 instruction word into a category."""
    op0 = (word >> 25) & 0xF  # bits [28:25]

    # Data processing — immediate
    if op0 in (0x8, 0x9):  # 100x
        return "dp-imm"

    # Branches, exception, system
    if op0 in (0xA, 0xB):  # 101x
        top = (word >> 22) & 0x3FF
        if (word >> 22) == 0b1101010100:  # system instructions
            l = (word >> 21) & 1
            op0_sys = (word >> 19) & 3
            crn = (word >> 12) & 0xF
            if op0_sys == 0 and crn == 2:
                return "hint"  # NOP/WFI/WFE/YIELD
            if op0_sys == 0 and crn == 3:
                return "barrier"  # DSB/DMB/ISB
            if l == 1:
                return "mrs"
            return "msr"
        if (word >> 26) & 0x1F == 0b00101:
            return "branch"
        if (word >> 25) & 0x3F == 0b011010:
            return "cond-branch"
        return "branch/sys"

    # Loads and stores
    if op0 in (0x4, 0x6, 0xC, 0xE):  # x1x0
        return "ldst"

    # Data processing — register
    if op0 in (0x5, 0xD):  # x101
        if (word >> 21) & 0x7FF == 0b11010110:
            return "dp-reg-2src"
        if (word >> 24) & 0x1F == 0b11011:
            return "dp-reg-3src"  # MADD/MSUB/UMULL etc.
        return "dp-reg"

    # SIMD / FP
    if op0 in (0x7, 0xF):  # x111
        if (word >> 28) & 1 == 0:
            return "fp"
        return "simd"

    return f"unknown({op0:#x})"


def disasm_range(s, start, size, label):
    """Read and classify instructions in a range."""
    data = []
    # Read in chunks
    chunk = 256
    for off in range(0, size, chunk):
        addr = start + off
        read_size = min(chunk, size - off)
        # Use monitor to read memory
        raw = s._read_raw(addr, read_size)
        if raw:
            data.extend(raw)
        else:
            data.extend([0] * read_size)

    if not data:
        print(f"  [{label}] Could not read memory at {start:#x}", flush=True)
        return {}

    counts = {}
    for i in range(0, len(data) - 3, 4):
        word = struct.unpack_from('<I', bytes(data), i)[0]
        cat = classify_insn(word)
        counts[cat] = counts.get(cat, 0) + 1

    total = sum(counts.values())
    print(f"  [{label}] {total} instructions at {start:#x}..{start+size:#x}:", flush=True)
    for cat, n in sorted(counts.items(), key=lambda x: -x[1]):
        pct = 100.0 * n / total if total else 0
        print(f"    {cat:<16} {n:>5}  ({pct:5.1f}%)", flush=True)
    return counts


# We can't use s._read_raw directly — let's use a different approach.
# Read the kernel binary from disk and map to virtual addresses.

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
SYSMAP = "assets/alpine/boot/System.map-6.12.67-0-rpi"

# Load System.map
symbols = {}
with open(SYSMAP) as f:
    for line in f:
        parts = line.strip().split(None, 2)
        if len(parts) == 3:
            try:
                symbols[parts[2]] = int(parts[0], 16)
            except ValueError:
                pass

# Read kernel binary
with open(KERNEL, "rb") as f:
    kernel_data = f.read()

# The kernel is loaded at RAM_BASE + text_offset
# text_offset is at offset 0x08 in the Image header
text_offset = struct.unpack_from('<Q', kernel_data, 0x08)[0]
if text_offset == 0:
    text_offset = 0x200000  # 2MB default
RAM_BASE = 0x40000000
KERNEL_BASE = RAM_BASE + text_offset

# Get _text symbol as the kernel virtual base
kernel_virt_base = symbols.get('_text', 0)
if kernel_virt_base == 0:
    kernel_virt_base = symbols.get('_stext', 0)

print(f"[HelmPy] Kernel loaded at PA {KERNEL_BASE:#x}", flush=True)
print(f"[HelmPy] Kernel virtual base: {kernel_virt_base:#x}", flush=True)
print(f"[HelmPy] Kernel binary size: {len(kernel_data)} bytes", flush=True)

def va_to_file_offset(va):
    """Convert a kernel virtual address to an offset in the kernel binary."""
    return va - kernel_virt_base

def classify_function(name):
    """Classify instructions in a kernel function."""
    addr = symbols.get(name)
    if not addr:
        print(f"  Symbol '{name}' not found", flush=True)
        return

    # Find function size by looking at next symbol
    sorted_addrs = sorted(set(symbols.values()))
    idx = sorted_addrs.index(addr)
    if idx + 1 < len(sorted_addrs):
        size = sorted_addrs[idx + 1] - addr
    else:
        size = 256  # default

    size = min(size, 4096)  # cap at 4KB

    file_off = va_to_file_offset(addr)
    if file_off < 0 or file_off + size > len(kernel_data):
        print(f"  [{name}] out of range: file_off={file_off}, size={size}", flush=True)
        return

    func_data = kernel_data[file_off:file_off + size]

    counts = {}
    for i in range(0, len(func_data) - 3, 4):
        word = struct.unpack_from('<I', func_data, i)[0]
        cat = classify_insn(word)
        counts[cat] = counts.get(cat, 0) + 1

    total = sum(counts.values())
    print(f"\n[{name}] {total} instructions ({size} bytes) at {addr:#x}:", flush=True)
    for cat, n in sorted(counts.items(), key=lambda x: -x[1]):
        pct = 100.0 * n / total if total else 0
        print(f"  {cat:<16} {n:>5}  ({pct:5.1f}%)", flush=True)

    # Check for specific patterns
    has_simd = counts.get("simd", 0) > 0
    has_fp = counts.get("fp", 0) > 0
    has_crypto = False
    for i in range(0, len(func_data) - 3, 4):
        word = struct.unpack_from('<I', func_data, i)[0]
        # SHA/AES/CRC crypto instructions: various encodings
        if (word >> 24) & 0xFF == 0xCE:  # crypto group
            has_crypto = True
    if has_simd:
        print(f"  ** Uses SIMD/NEON instructions", flush=True)
    if has_fp:
        print(f"  ** Uses FP instructions", flush=True)
    if has_crypto:
        print(f"  ** Uses crypto instructions (SHA/AES)", flush=True)


print("\n=== HOT FUNCTION SURVEY ===\n", flush=True)

# Key functions identified during boot profiling
hot_functions = [
    "blake2s_compress_generic",
    "__pi_lib_decompress",     # gzip decompression
    "__pi___decompress",
    "decompress_generic",
    "inflate_fast",
    "gunzip",
    "unzip",
    "populate_rootfs",
    "do_populate_rootfs",
    "crc32_le",
    "memcpy",
    "memset",
    "__memcpy",
    "__memset",
    "copy_page",
    "clear_page",
    "__arch_copy_from_user",
    "__arch_copy_to_user",
]

for fn in hot_functions:
    if fn in symbols:
        classify_function(fn)
    # Also try with __ prefix
    elif "__" + fn in symbols:
        classify_function("__" + fn)

print("\n=== DONE ===", flush=True)
