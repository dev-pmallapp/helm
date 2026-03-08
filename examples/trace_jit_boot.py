#!/usr/bin/env python3
"""Trace JIT to find where exception occurs."""
import _helm_core, sys
sys.stdout.reconfigure(line_buffering=True)

sj = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="jit")

# Step through in chunks, watch for exception vector entry
prev_pc = sj.pc
for i in range(500):
    sj.run(1000)
    pc = sj.pc
    ic = sj.insn_count
    if pc != prev_pc:
        # Check if we jumped to an exception vector (low address or VBAR-relative)
        if pc < 0x1000 or (pc & 0xFFF) in [0, 0x80, 0x100, 0x180, 0x200, 0x280, 0x300, 0x380, 0x400, 0x480, 0x500, 0x580, 0x600, 0x680, 0x700, 0x780]:
            regs = sj.regs()
            print(f"  [{ic:>8}] EXCEPTION? PC={pc:#x} from={prev_pc:#x} "
                  f"EL={regs.get('current_el',0)} DAIF={regs.get('daif',0):#x}")
        if i < 50 or pc < 0x1000:
            print(f"  [{ic:>8}] PC: {prev_pc:#x} → {pc:#x}")
    prev_pc = pc
    if pc == 0x200 or pc == 0x0:
        regs = sj.regs()
        print(f"\n  STUCK at exception vector PC={pc:#x}")
        print(f"    EL={regs.get('current_el',0)} DAIF={regs.get('daif',0):#x} NZCV={regs.get('nzcv',0):#x}")
        for xn in [0,1,2,30]:
            print(f"    X{xn}={sj.xn(xn):#x}")
        break
