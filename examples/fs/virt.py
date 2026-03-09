#!/usr/bin/env python3
"""Boot a Linux kernel on the ARM virt platform.

The virt machine provides a GICv2, PL011 UARTs, and VirtIO MMIO slots
— the same layout as QEMU's ``-machine virt``.

Usage:
    helm-system-aarch64 examples/fs/virt.py
    helm-system-aarch64 examples/fs/virt.py -- --kernel /path/to/Image
    helm-system-aarch64 examples/fs/virt.py -- --memory 512M --backend interp
"""
import argparse
import sys
import time

import _helm_core

sys.stdout.reconfigure(line_buffering=True)

DEFAULTS = dict(
    kernel="assets/alpine/boot/vmlinuz-rpi",
    append="earlycon=pl011,0x09000000 console=ttyAMA0",
    memory="256M",
    serial="stdio",
    backend="jit",
    timing="fe",
    dtb=None,
    sysmap=None,
    max_insns=500_000_000,
)


def parse_args(argv=None):
    p = argparse.ArgumentParser(description="HELM FS — ARM virt platform")
    p.add_argument("--kernel",  default=DEFAULTS["kernel"])
    p.add_argument("--append",  default=DEFAULTS["append"])
    p.add_argument("--memory",  default=DEFAULTS["memory"])
    p.add_argument("--serial",  default=DEFAULTS["serial"], choices=["stdio", "null"])
    p.add_argument("--backend", default=DEFAULTS["backend"], choices=["jit", "interp"])
    p.add_argument("--timing",  default=DEFAULTS["timing"], choices=["fe", "ape", "cae"])
    p.add_argument("--dtb",     default=DEFAULTS["dtb"])
    p.add_argument("--sysmap",  default=DEFAULTS["sysmap"])
    p.add_argument("--max-insns", type=int, default=DEFAULTS["max_insns"])
    return p.parse_args(argv)


def main():
    args = parse_args()

    print(f"[virt] kernel={args.kernel}  memory={args.memory}  backend={args.backend}")
    s = _helm_core.FsSession(
        kernel=args.kernel,
        machine="virt",
        append=args.append,
        memory_size=args.memory,
        serial=args.serial,
        timing=args.timing,
        backend=args.backend,
        dtb=args.dtb,
        sysmap=args.sysmap,
    )
    print(f"[virt] entry PC={s.pc:#x}")

    t0 = time.monotonic()
    s.run(args.max_insns)
    wall = time.monotonic() - t0
    mips = s.insn_count / wall / 1e6 if wall > 0.001 else 0

    st = s.stats()
    print(f"[virt] {s.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")
    print(f"[virt] PC={s.pc:#x}  EL={s.current_el}  IRQs={st.get('irq_count', 0)}")


if __name__ == "__main__":
    main()
