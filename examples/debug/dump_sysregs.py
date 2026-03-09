#!/usr/bin/env python3
"""Dump MMU / exception system registers at a given instruction count.

Usage:
    helm-system-aarch64 examples/debug/dump_sysregs.py
    helm-system-aarch64 examples/debug/dump_sysregs.py -- --at 5000000
"""
import argparse, sys
import _helm_core

sys.stdout.reconfigure(line_buffering=True)

SYSREGS = [
    "sctlr_el1", "tcr_el1", "ttbr0_el1", "ttbr1_el1",
    "mair_el1", "vbar_el1", "elr_el1", "spsr_el1",
    "esr_el1", "far_el1", "hcr_el2", "scr_el3",
    "cntv_ctl_el0", "cntv_cval_el0", "cntp_ctl_el0", "cntp_cval_el0",
]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--kernel", default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--at", type=int, default=10_000_000,
                    help="Dump sysregs after this many instructions")
    p.add_argument("--backend", default="jit", choices=["jit", "interp"])
    args = p.parse_args()

    s = _helm_core.FsSession(
        kernel=args.kernel, machine="virt",
        append="earlycon=pl011,0x09000000 console=ttyAMA0",
        backend=args.backend,
    )
    s.run(args.at)

    print(f"PC={s.pc:#x}  EL={s.current_el}  insns={s.insn_count:,}")
    print(f"{'Register':<18} {'Value':>18}")
    print("-" * 38)
    for name in SYSREGS:
        val = s.sysreg(name)
        if val is not None:
            print(f"{name:<18} {val:#018x}")


if __name__ == "__main__":
    main()
