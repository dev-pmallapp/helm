#!/usr/bin/env python3
"""
Full-system Raspberry Pi 3 — quad Cortex-A53.

NOTE: Full-system mode is not yet implemented in HELM.
This config demonstrates the intended API and can be used to
validate the platform description JSON.

Future usage::

    helm-arm --mode fs configs/example/fs/arm/rpi-3.py -- \\
        --kernel vmlinux --dtb bcm2710-rpi-3-b.dtb \\
        --disk sdcard.img

Equivalent gem5::

    build/ARM/gem5.opt configs/example/arm/fs_bigLITTLE.py \\
        --big-cpus 0 --little-cpus 4 --cpu-type atomic
"""

import sys, os
_root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "..")
sys.path.insert(0, _root)
sys.path.insert(0, os.path.join(_root, "python"))

import argparse
import json

from configs.components.boards import RaspberryPi3

parser = argparse.ArgumentParser(
    description="HELM FS — Raspberry Pi 3 (Cortex-A53 × 4)",
    formatter_class=argparse.ArgumentDefaultsHelpFormatter,
)
parser.add_argument("--kernel", default="vmlinux",
                    help="Path to kernel image")
parser.add_argument("--dtb", default="bcm2710-rpi-3-b.dtb",
                    help="Device-tree blob")
parser.add_argument("--disk", default=None,
                    help="SD card / disk image")
parser.add_argument("--max-insns", type=int, default=100_000_000)
parser.add_argument("--plugin", action="append", default=[])

args = parser.parse_args()

board = RaspberryPi3()
plat = board.to_dict()

config = {
    "mode": "fs",
    "kernel": args.kernel,
    "dtb": args.dtb,
    "disk": args.disk,
    "max_insns": args.max_insns,
    "platform": {
        "name": plat["name"],
        "isa": plat["isa"],
        "cores": plat["cores"],
        "memory": plat["memory"],
        "timing": board.timing.to_dict(),
    },
    "plugins": args.plugin,
}

print(json.dumps(config, indent=2))
