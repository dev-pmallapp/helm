#!/usr/bin/env python3
"""Boot a Linux kernel on the ARM virt platform.

The virt machine provides a GICv2, PL011 UARTs, and VirtIO MMIO slots
— the same layout as QEMU's ``-machine virt``.

Usage:
    helm-system-aarch64 examples/fs/virt.py --kernel /path/to/Image
    helm-system-aarch64 --kernel /path/to/Image     # embedded mode
"""
import argparse
import sys
import time

import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="HELM FS — ARM virt platform")
    p.add_argument("--kernel", default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--initrd")
    p.add_argument("--bios")
    p.add_argument("--dtb")
    p.add_argument("--append", default="earlycon=pl011,0x09000000 console=ttyAMA0")
    p.add_argument("--memory", "-m", default="256M")
    p.add_argument("--smp", type=int, default=1)
    p.add_argument("--serial", default="stdio", choices=["stdio", "null"])
    p.add_argument("--backend", default="jit", choices=["jit", "interp"])
    p.add_argument("--timing", default="fe", choices=["fe", "ape", "cae"])
    p.add_argument("--max-insns", "-n", type=int, default=0)
    p.add_argument("--device", action="append", default=[])
    p.add_argument("--drive", action="append", default=[])
    p.add_argument("--sd")
    p.add_argument("--plugin", action="append", default=[])
    p.add_argument("--sysmap")
    p.add_argument("--dump-dtb")
    p.add_argument("--dump-config", action="store_true")
    p.add_argument("--monitor", action="store_true")
    return p.parse_args()


def build_platform(args):
    """Build the ARM virt platform from Python using _helm_core device APIs."""
    irq = _helm_core.IrqSignal()
    platform = _helm_core.Platform("arm-virt")

    # GIC at 0x0800_0000
    gic = _helm_core.create_device("gic", max_irqs=256, irq_signal=irq)
    platform.add_device("gic", 0x0800_0000, gic)

    # APB bus for peripherals at 0x0900_0000
    uart0 = _helm_core.create_device("pl011", name="uart0", serial=args.serial)
    uart1 = _helm_core.create_device("pl011", name="uart1", serial="null")
    apb = _helm_core.create_device("apb-bus", name="apb", window=0x10_0000)
    apb.attach_child(0x0000, 0x1000, uart0)
    apb.attach_child(0x1000, 0x1000, uart1)
    platform.add_device("apb", 0x0900_0000, apb)

    # Add extra --device specs
    base = 0x0B00_0000
    for dev_spec in args.device:
        parts = dev_spec.split(",")
        dev_type = parts[0]
        kwargs = {}
        dev_base = base
        for part in parts[1:]:
            if "=" in part:
                k, v = part.split("=", 1)
                if k in ("base", "addr"):
                    dev_base = int(v, 0)
                else:
                    kwargs[k] = v
        try:
            dev = _helm_core.create_device(dev_type, **kwargs)
            platform.add_device(dev_type, dev_base, dev)
        except RuntimeError as e:
            print(f"[virt] warning: {e}", file=sys.stderr)
        base += 0x1000

    return platform, irq


def main():
    args = parse_args()

    if args.dump_config:
        platform, _irq = build_platform(args)
        for name, base in platform.device_list():
            print(f"  {name} @ 0x{base:08x}")
        return

    print(f"[virt] kernel={args.kernel}  memory={args.memory}  backend={args.backend}")
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
    print(f"[virt] entry PC={s.pc:#x}")

    limit = args.max_insns if args.max_insns > 0 else 2**63
    t0 = time.monotonic()
    s.run(limit)
    wall = time.monotonic() - t0
    mips = s.insn_count / wall / 1e6 if wall > 0.001 else 0

    st = s.stats()
    print(f"[virt] {s.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")
    print(f"[virt] PC={s.pc:#x}  EL={s.current_el}  IRQs={st.get('irq_count', 0)}")


if __name__ == "__main__":
    main()
