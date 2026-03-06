#!/usr/bin/env python3
"""
SE-mode AArch64 simulation — Cortex-A class presets.

Provides named board profiles instead of raw CPU parameters.

Usage:
    helm-arm configs/example/se/arm-v7+.py -- --board rpi3 \\
        --binary assets/binaries/fish --options "--no-config -c 'echo hello'"

    helm-arm configs/example/se/arm-v7+.py -- --board neoverse \\
        --binary ./server-workload --l2cache

Available boards:
    rpi3       — Raspberry Pi 3 (quad Cortex-A53)
    rpi4       — Raspberry Pi 4 (quad Cortex-A72)
    big-little — big.LITTLE (2× A72 + 4× A53)
    neoverse   — Arm Neoverse N1 server (quad N1)
"""

import sys, os
_root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..")
sys.path.insert(0, _root)
sys.path.insert(0, os.path.join(_root, "python"))

import argparse
import json
import shlex

from configs.common.caches import L1ICache, L1DCache, L2Cache
from configs.components.boards import (
    RaspberryPi3, RaspberryPi4, ArmBigLittle, NeoVerseServer,
)

_BOARDS = {
    "rpi3": RaspberryPi3,
    "rpi4": RaspberryPi4,
    "big-little": ArmBigLittle,
    "neoverse": NeoVerseServer,
}

parser = argparse.ArgumentParser(
    description="HELM SE — ARM board profiles",
    formatter_class=argparse.ArgumentDefaultsHelpFormatter,
)

parser.add_argument("--board", default="rpi4", choices=list(_BOARDS),
                    help="Board / SoC profile")
parser.add_argument("--binary", "-b", required=True,
                    help="Path to AArch64 static binary")
parser.add_argument("--cmd", default=None, help="Override argv[0]")
parser.add_argument("--options", "-o", default="",
                    help="Guest binary arguments (shell-quoted)")
parser.add_argument("--env", nargs="*", default=None,
                    help="Environment variables (KEY=VAL ...)")
parser.add_argument("--max-insns", type=int, default=50_000_000)
parser.add_argument("--l2cache", action="store_true", default=False,
                    help="Override: add L2 cache if board lacks one")
parser.add_argument("--plugin", action="append", default=[])

args = parser.parse_args()

# Build board
board = _BOARDS[args.board]()

# Workload
binary = args.binary
cmd = args.cmd or os.path.basename(binary)
options_tokens = shlex.split(args.options) if args.options else []
argv = [cmd] + options_tokens
envp = args.env if args.env is not None else [
    "HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm",
]

# Assemble config
plat = board.to_dict()

config = {
    "binary": binary,
    "argv": argv,
    "envp": envp,
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

print(json.dumps(config))
