#!/usr/bin/env python3
"""Debug fish crash with new plugin system.

Uses fault-detect, execlog, mem-trace, and trace_after to investigate
the BRK assertion at ~4680 insns.
"""
import os
import sys

sys.stdout.reconfigure(line_buffering=True)
import _helm_ng

binary = os.environ.get("HELM_BINARY", "assets/binaries/fish")
if not os.path.isfile(binary):
    print(f"binary not found: {binary}", file=sys.stderr)
    sys.exit(1)

argv = ["fish", "-N", "-c", "echo hello"]
envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]

sim = _helm_ng.build_simulation(isa="aarch64", mode="se", timing="virtual")
sim.load_elf(binary, argv, envp)

# ── Print symbol table (key functions) ────────────────────────────────────────
print("=== SYMBOL TABLE (relevant) ===", file=sys.stderr)
for name, addr, size in sim.symbols():
    if any(k in name for k in ["main", "malloc", "free", "brk", "abort",
                                 "raise", "__assert", "fish_", "signal"]):
        print(f"  {name:40s} @ {addr:#018x}  size={size}", file=sys.stderr)
print(file=sys.stderr)

# ── Install plugins ───────────────────────────────────────────────────────────

# 1. Fault detector — ring buffer of last 64 PCs + syscall log + register dump
sim.add_plugin("fault-detect", "history=64")

# 2. Syscall trace — see what syscalls happen
sim.add_plugin("syscall-trace")

# 3. Full instruction trace for last ~200 insns before crash
#    We know it crashes at ~4680, so start tracing at 4480
sim.trace_after(insn_count=4480, events=["insn", "mem"], max=500)

# 4. Branch trace to see control flow
sim.add_plugin("branch-trace", "top=20")

# ── Run ───────────────────────────────────────────────────────────────────────
print("=== RUNNING (max 10000 insns) ===", file=sys.stderr)
result = sim.run(10000)
print(f"\n=== RESULT: {result} ===", file=sys.stderr)
print(f"PC={sim.pc:#x}  insn_count={sim.insn_count}", file=sys.stderr)

# Register dump
print("\n=== REGISTERS ===", file=sys.stderr)
for i in range(31):
    v = sim.xn(i)
    if v != 0:
        print(f"  x{i:<2d} = {v:#018x}", file=sys.stderr)
print(f"  SP  = {sim.sp:#018x}", file=sys.stderr)
print(f"  PC  = {sim.pc:#018x}", file=sys.stderr)

# ── Examine memory around crash PC ───────────────────────────────────────────
print("\n=== MEMORY AROUND PC ===", file=sys.stderr)
pc = sim.pc
for offset in range(-16, 20, 4):
    addr = pc + offset
    val = sim.read_mem(addr)
    marker = " <<<" if offset == 0 else ""
    print(f"  {addr:#018x}: {val:#018x}{marker}", file=sys.stderr)

# Finish — triggers atexit reports
print("\n=== PLUGIN REPORTS ===", file=sys.stderr)
sim.finish()
