#!/usr/bin/env python3
"""Full-system simulation configuration — like gem5's configs/example/fs.py.

Usage::

    python -m helm.configs.fs --platform realview-pb --kernel zImage
    python -m helm.configs.fs --platform rpi3 --kernel kernel8.img --disk rootfs.img
    python -m helm.configs.fs --platform arm-virt --kernel Image --device virtio-blk,file=disk.img
"""

from __future__ import annotations

import argparse
import json
import sys


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="HELM Full-System Simulation",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    p.add_argument(
        "--platform",
        choices=["realview-pb", "realview", "rpi3", "raspi3", "arm-virt", "virt"],
        default="arm-virt",
        help="Machine type (default: arm-virt)",
    )
    p.add_argument("--kernel", required=True, help="Path to kernel image")
    p.add_argument("--dtb", default=None, help="Path to device tree blob")
    p.add_argument("--disk", default=None, help="Path to disk image")
    p.add_argument("--sysmap", default=None, help="Path to System.map")
    p.add_argument("--memory", default="256M", help="RAM size (default: 256M)")
    p.add_argument(
        "--serial",
        default="stdio",
        choices=["stdio", "null", "file"],
        help="UART0 backend (default: stdio)",
    )
    p.add_argument(
        "--timing",
        default="fe",
        choices=["fe", "ape", "cae"],
        help="Timing model (default: fe)",
    )
    p.add_argument(
        "--backend",
        default="jit",
        choices=["jit", "interp"],
        help="Execution backend (default: jit)",
    )
    p.add_argument(
        "--append",
        default="",
        help="Kernel command line (default: empty)",
    )
    p.add_argument(
        "--device",
        action="append",
        default=[],
        metavar="SPEC",
        help="Extra device: type,key=val,... (e.g. virtio-blk,file=extra.img)",
    )
    p.add_argument(
        "--max-cycles",
        type=int,
        default=10_000_000,
        help="Maximum simulation cycles",
    )
    p.add_argument(
        "--dump-config",
        action="store_true",
        help="Print platform config JSON and exit",
    )
    return p


def main(argv: list[str] = None) -> None:
    args = build_parser().parse_args(argv)

    if args.dump_config:
        config = {
            "kernel": args.kernel,
            "machine": args.platform,
            "append": args.append,
            "memory_size": args.memory,
            "serial": args.serial,
            "timing": args.timing,
            "backend": args.backend,
            "dtb": args.dtb,
            "sysmap": args.sysmap,
            "max_cycles": args.max_cycles,
        }
        print(json.dumps(config, indent=2))
        return

    from helm.session import FsSession

    print(f"[HELM] Platform: {args.platform}")
    print(f"[HELM] Kernel: {args.kernel}")
    print(f"[HELM] Timing: {args.timing}")
    print(f"[HELM] Backend: {args.backend}")
    print(f"[HELM] Memory: {args.memory}")

    s = FsSession(
        args.kernel,
        machine=args.platform,
        append=args.append,
        memory_size=args.memory,
        serial=args.serial,
        timing=args.timing,
        backend=args.backend,
        dtb=args.dtb,
        sysmap=args.sysmap,
    )
    result = s.run(args.max_cycles)
    stats = s.stats()
    print(f"[HELM] Result: {result}")
    print(f"[HELM] Instructions: {stats.get('insn_count', 0)}")
    print(f"[HELM] Virtual cycles: {stats.get('virtual_cycles', 0)}")
    print(f"[HELM] IRQs: {stats.get('irq_count', 0)}")


def _parse_device_spec(platform, spec: str) -> None:
    """Parse 'type,key=val,key=val' and attach to platform."""
    parts = spec.split(",")
    dev_type = parts[0]
    params = {}
    for part in parts[1:]:
        if "=" in part:
            k, v = part.split("=", 1)
            params[k] = v

    # Device type dispatch
    if dev_type == "virtio-blk":
        from helm.devices.block import VirtioBlk
        from helm.backends.block import FileBlockBackend

        path = params.get("file", "")
        blk = VirtioBlk(params.get("name", "disk"), backend=FileBlockBackend(path))
        base = int(params.get("base", "0x0A000000"), 0)
        blk.base_address = base
        platform.add_device(blk)
    elif dev_type == "virtio-net":
        from helm.devices.net import VirtioNet

        nic = VirtioNet(params.get("name", "nic"))
        base = int(params.get("base", "0x0A000200"), 0)
        nic.base_address = base
        platform.add_device(nic)
    else:
        print(f"[HELM] Warning: unknown device type '{dev_type}'", file=sys.stderr)


if __name__ == "__main__":
    main()
