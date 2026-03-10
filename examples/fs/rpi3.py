#!/usr/bin/env python3
"""Boot a Linux kernel on the Raspberry Pi 3 (BCM2837) platform.

The rpi3 machine provides a BCM system timer, mailbox, GPIO,
PL011 UART0, and BCM Mini UART1.

Usage:
    helm-system-aarch64 examples/fs/rpi3.py --kernel kernel8.img
"""
import argparse
import sys
import time

import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="HELM FS — Raspberry Pi 3")
    p.add_argument("--kernel", default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--initrd")
    p.add_argument("--dtb")
    p.add_argument("--append", default="earlycon=pl011,0x3F201000 console=ttyAMA0")
    p.add_argument("--memory", "-m", default="256M")
    p.add_argument("--serial", default="stdio", choices=["stdio", "null"])
    p.add_argument("--backend", default="jit", choices=["jit", "interp"])
    p.add_argument("--timing", default="fe", choices=["fe", "ape", "cae"])
    p.add_argument("--max-insns", "-n", type=int, default=0)
    p.add_argument("--sysmap")
    return p.parse_args()


def build_platform(args):
    """Build the Raspberry Pi 3 platform from Python."""
    irq = _helm_core.IrqSignal()
    platform = _helm_core.Platform("rpi3")

    # System timer
    sys_timer = _helm_core.create_device("bcm-sys-timer", name="sys-timer")
    platform.add_device("sys-timer", 0x3F00_3000, sys_timer)

    # Mailbox
    mailbox = _helm_core.create_device("bcm-mailbox", name="mailbox")
    platform.add_device("mailbox", 0x3F00_B880, mailbox)

    # GPIO
    gpio = _helm_core.create_device("bcm-gpio", name="gpio")
    platform.add_device("gpio", 0x3F20_0000, gpio)

    # PL011 UART0 (full UART)
    uart0 = _helm_core.create_device("pl011", name="uart0", serial=args.serial)
    platform.add_device("uart0", 0x3F20_1000, uart0)

    # Mini UART (UART1)
    uart1 = _helm_core.create_device("bcm-mini-uart", name="uart1", serial="null")
    platform.add_device("uart1", 0x3F21_5000, uart1)

    return platform, irq


def main():
    args = parse_args()

    print(f"[rpi3] kernel={args.kernel}  memory={args.memory}  backend={args.backend}")
    platform, irq = build_platform(args)

    s = _helm_core.FsSession.from_platform(
        platform,
        kernel=args.kernel,
        memory_size=args.memory,
        backend=args.backend,
        timing=args.timing,
        append=args.append,
        dtb=args.dtb,
        initrd=args.initrd,
        sysmap=args.sysmap,
        serial=args.serial,
        irq_signal=irq,
    )
    print(f"[rpi3] entry PC={s.pc:#x}")

    limit = args.max_insns if args.max_insns > 0 else 2**63
    t0 = time.monotonic()
    s.run(limit)
    wall = time.monotonic() - t0
    mips = s.insn_count / wall / 1e6 if wall > 0.001 else 0

    st = s.stats()
    print(f"[rpi3] {s.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")
    print(f"[rpi3] PC={s.pc:#x}  EL={s.current_el}  IRQs={st.get('irq_count', 0)}")


if __name__ == "__main__":
    main()
