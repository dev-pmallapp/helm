#!/usr/bin/env python3
"""Track kernel boot progress — reports PC, EL, and kernel-VA status.

Usage:
    helm-system-aarch64 examples/debug/boot_progress.py
    helm-system-aarch64 examples/debug/boot_progress.py -- --backend interp
"""
import argparse, sys, time
import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--kernel", default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--backend", default="jit", choices=["jit", "interp"])
    p.add_argument("--max-insns", type=int, default=500_000_000)
    args = p.parse_args()

    s = _helm_core.FsSession(
        kernel=args.kernel, machine="virt",
        append="earlycon=pl011,0x09000000 console=ttyAMA0",
        backend=args.backend,
    )

    checkpoints = [100_000, 1_000_000, 10_000_000, 50_000_000,
                   100_000_000, 200_000_000, 500_000_000]

    for cp in checkpoints:
        if cp > args.max_insns:
            break
        budget = cp - s.insn_count
        if budget <= 0:
            continue
        t0 = time.monotonic()
        s.run(budget)
        wall = time.monotonic() - t0
        mips = budget / wall / 1e6 if wall > 0.001 else 0

        kva = s.pc > 0xFFFF_0000_0000_0000
        tag = "KERNEL-VA" if kva else ("EXC-VEC" if s.pc < 0x1000 else "phys")

        print(f"  {cp:>12,}: PC={s.pc:#018x}  EL{s.current_el}  "
              f"{wall:.2f}s {mips:.0f}MIPS  {tag}")

        if s.pc < 0x1000:
            print(f"    *** stuck at exception vector {s.pc:#x} ***")
            break

    st = s.stats()
    print(f"\n  Total: {s.insn_count:,} insns  IRQs={st.get('irq_count',0)}  "
          f"skips={st.get('isa_skip_count',0)}")


if __name__ == "__main__":
    main()
