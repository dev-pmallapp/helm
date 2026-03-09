#!/usr/bin/env python3
"""Boot a Linux kernel on the Raspberry Pi 3 (BCM2837) platform.

The rpi3 machine provides a BCM system timer, mailbox, GPIO,
PL011 UART0, and BCM Mini UART1.

Usage:
    helm-system-aarch64 examples/fs/rpi3.py
    helm-system-aarch64 examples/fs/rpi3.py -- --kernel kernel8.img
"""
import argparse
import sys
import time

import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def parse_args(argv=None):
    p = argparse.ArgumentParser(description="HELM FS — Raspberry Pi 3")
    p.add_argument("--kernel",  default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--append",  default="earlycon=pl011,0x3F201000 console=ttyAMA0")
    p.add_argument("--memory",  default="256M")
    p.add_argument("--serial",  default="stdio", choices=["stdio", "null"])
    p.add_argument("--backend", default="jit", choices=["jit", "interp"])
    p.add_argument("--timing",  default="fe", choices=["fe", "ape", "cae"])
    p.add_argument("--dtb",     default=None)
    p.add_argument("--sysmap",  default=None)
    p.add_argument("--max-insns", type=int, default=500_000_000)
    return p.parse_args(argv)


def main():
    args = parse_args()

    print(f"[rpi3] kernel={args.kernel}  memory={args.memory}  backend={args.backend}")
    s = _helm_core.FsSession(
        kernel=args.kernel,
        machine="rpi3",
        append=args.append,
        memory_size=args.memory,
        serial=args.serial,
        timing=args.timing,
        backend=args.backend,
        dtb=args.dtb,
        sysmap=args.sysmap,
    )
    print(f"[rpi3] entry PC={s.pc:#x}")

    t0 = time.monotonic()
    s.run(args.max_insns)
    wall = time.monotonic() - t0
    mips = s.insn_count / wall / 1e6 if wall > 0.001 else 0

    st = s.stats()
    print(f"[rpi3] {s.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")
    print(f"[rpi3] PC={s.pc:#x}  EL={s.current_el}  IRQs={st.get('irq_count', 0)}")


if __name__ == "__main__":
    main()
