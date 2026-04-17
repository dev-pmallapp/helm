#!/usr/bin/env python3
"""Capture fault context with a ring buffer of recent instructions.

Real-world scenario: When debugging a fish shell crash at ~4680
instructions (the original debug_fish.py, commit 8581432), the
fault-detect plugin maintained a ring buffer of the last N instruction
PCs.  On fault, it dumps the instruction history leading up to the
crash, making it possible to trace backward from the fault site without
a full trace (which would be too slow for large runs).

This is also the pattern used to debug L4Re boot crashes (commit
a31eaed, l4re_ned.py) and unimplemented instruction aborts.

This example demonstrates:
  - fault-detect plugin with configurable history depth
  - Branch-trace for call flow leading to the fault
  - Register and memory dump at the crash site
  - Fault classification (SIGILL, SIGSEGV, data abort, etc.)

Usage:
    helm-aarch64 examples/debug/fault_detect.py --binary ./crashing_binary
    helm-aarch64 examples/debug/fault_detect.py --binary ./my_elf \\
        --history 128 --max-insns 1000000
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import helmutil

helmutil.require_launcher()
_helm_ng = helmutil.import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(
        description="Fault detector — ring buffer of recent instructions "
                    "dumped on crash/fault"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=50_000_000,
                   help="Max instructions (default 50M)")
    p.add_argument("--history", type=int, default=64,
                   help="Ring buffer depth — recent PCs to keep (default 64)")
    p.add_argument("--branch-trace", action="store_true",
                   help="Also attach branch-trace for call flow context")
    p.add_argument("--stub-trace", action="store_true",
                   help="Also attach stub-tracer to detect unimplemented "
                        "instruction crashes")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def _dump_regs(sim):
    """Print AArch64 register state."""
    print(f"\n  Register state:")
    for i in range(0, 31, 4):
        regs = "  ".join(f"x{i+j:<2d}={sim.xn(i+j):#018x}"
                         for j in range(4) if i + j < 31)
        print(f"    {regs}")
    print(f"    SP ={sim.sp:#018x}  PC ={sim.pc:#018x}")
    print(f"    NZCV={sim.nzcv:#x}  EL={sim.current_el}")


def _dump_memory_around_pc(sim):
    """Read and display memory around the faulting PC."""
    pc = sim.pc
    print(f"\n  Memory around PC ({pc:#018x}):")
    for offset in range(-16, 20, 4):
        addr = pc + offset
        try:
            val = sim.read_mem(addr)
            marker = " <-- PC" if offset == 0 else ""
            print(f"    {addr:#018x}: {val:#018x}{marker}")
        except Exception:
            pass


def main():
    args = parse_args()
    binary = args.binary
    if not os.path.isfile(binary):
        print(f"[fault] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
            "USER=helm"]

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                     timing="virtual")
    sim.load_elf(binary, argv, envp)

    # Attach fault-detect with ring buffer
    sim.add_plugin("fault-detect", f"history={args.history}")

    # Optional extra plugins
    if args.branch_trace:
        sim.add_plugin("branch-trace", "top=20")
    if args.stub_trace:
        sim.add_plugin("stub-tracer")

    print(f"[fault] binary={binary}  argv={argv}")
    print(f"[fault] fault-detect history={args.history}")
    print(f"[fault] Running up to {args.max_insns:,} instructions...")
    print()

    t0 = time.monotonic()
    chunk = 5_000_000
    remaining = args.max_insns
    stop_reason = "quantum"

    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop_reason = sim.run(n)
        remaining -= n
        if stop_reason not in ("quantum",):
            break

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    # Analyze the outcome
    print(f"[fault] Stopped: reason={stop_reason}  "
          f"insns={sim.insn_count:,}  {wall:.2f}s ({mips:.0f} MIPS)")

    if stop_reason.startswith("exit:"):
        code = sim.exit_code if sim.has_exited else "?"
        print(f"[fault] Clean exit with code {code} — no fault detected.")
    elif stop_reason.startswith("exception:") or stop_reason == "unsupported":
        print(f"\n[fault] *** FAULT DETECTED ***")
        print(f"[fault] stop_reason: {stop_reason}")
        print(f"[fault] PC:          {sim.pc:#018x}")
        print(f"[fault] insn_count:  {sim.insn_count:,}")
        _dump_regs(sim)
        _dump_memory_around_pc(sim)

        if sim.has_unimplemented_instructions:
            print(f"\n[fault] {sim.unimplemented_instruction_count} "
                  f"unimplemented instruction sites encountered")
    else:
        print(f"[fault] Reached limit — no fault in {args.max_insns:,} insns")
        _dump_regs(sim)

    # Flush all plugin reports (fault-detect prints ring buffer here)
    print(f"\n{'='*60}")
    print("Plugin reports (fault-detect ring buffer, branch trace, etc.):")
    print("=" * 60)
    sim.finish()


if __name__ == "__main__":
    main()
