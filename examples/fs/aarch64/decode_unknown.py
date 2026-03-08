#!/usr/bin/env python3
"""Decode the 'unknown' instructions in hot functions."""
import struct

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
SYSMAP = "assets/alpine/boot/System.map-6.12.67-0-rpi"

symbols = {}
with open(SYSMAP) as f:
    for line in f:
        parts = line.strip().split(None, 2)
        if len(parts) == 3:
            try:
                symbols[parts[2]] = int(parts[0], 16)
            except ValueError:
                pass

with open(KERNEL, "rb") as f:
    kernel_data = f.read()

text_offset = struct.unpack_from('<Q', kernel_data, 0x08)[0]
if text_offset == 0:
    text_offset = 0x200000
kernel_virt_base = symbols.get('_text', 0)

def va_to_offset(va):
    return va - kernel_virt_base

def decode_insn(word):
    """More detailed AArch64 decode."""
    # Check for LDP/STP with SIMD (opc=10, V=1)
    # Encoding: opc[31:30] 101 V[26] 0 L[22] imm7 Rt2 Rn Rt
    if (word >> 27) & 0x7 == 0b101 and (word >> 26) & 1 == 1:
        opc = (word >> 30) & 3
        l = (word >> 22) & 1
        op = "LDP" if l else "STP"
        if opc == 0:
            sz = "S"  # 32-bit float
        elif opc == 1:
            sz = "D"  # 64-bit float
        elif opc == 2:
            sz = "Q"  # 128-bit SIMD
        else:
            sz = "?"
        rt = word & 0x1F
        rt2 = (word >> 10) & 0x1F
        rn = (word >> 5) & 0x1F
        return f"{op} {sz}{rt},{sz}{rt2},[X{rn}]"

    # Check for LD1/ST1 (SIMD multiple structure)
    # 0 Q 001100 L 0 Rm opcode size Rn Rt
    if (word >> 24) & 0xFF in (0x0C, 0x4C, 0x0D, 0x4D):
        l = (word >> 22) & 1
        op = "LD1" if l else "ST1"
        return f"SIMD {op} (multi-struct)"

    # PRFM (prefetch)
    if (word >> 22) & 0x3FF == 0b1111100010:
        return "PRFM (prefetch)"

    return f"raw: {word:#010x} (bits[31:25]={word>>25:#04x})"

# Check the unknown instructions in copy_page and memcpy
for fn_name in ["copy_page", "memcpy", "__memcpy", "clear_page", "__arch_copy_to_user"]:
    addr = symbols.get(fn_name)
    if not addr:
        continue
    sorted_addrs = sorted(set(symbols.values()))
    idx = sorted_addrs.index(addr)
    size = min(sorted_addrs[idx + 1] - addr if idx + 1 < len(sorted_addrs) else 256, 1024)

    off = va_to_offset(addr)
    data = kernel_data[off:off + size]

    unknowns = []
    for i in range(0, len(data) - 3, 4):
        word = struct.unpack_from('<I', data, i)[0]
        op0 = (word >> 25) & 0xF
        if op0 == 0x0:
            decoded = decode_insn(word)
            unknowns.append((addr + i, word, decoded))

    if unknowns:
        print(f"\n[{fn_name}] {len(unknowns)} unknown(0x0) instructions:", flush=True)
        for va, word, desc in unknowns[:10]:
            print(f"  {va:#x}: {word:#010x}  {desc}", flush=True)
