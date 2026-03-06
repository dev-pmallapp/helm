#!/usr/bin/env python3
"""
SE-mode AArch64 simulation — generic Armv8-A.

Usage:
    helm-arm configs/example/se/arm-v8.py -- --binary ./my-arm-elf
    helm-arm configs/example/se/arm-v8.py -- --binary assets/binaries/fish \\
        --options "--no-config -c 'echo hello'" --cpu-type o3 --l2cache

Mirrors gem5 usage::

    build/ARM/gem5.opt configs/example/se.py --cmd=./hello --cpu-type=DerivO3CPU

CPU type mapping:
    atomic  → gem5 AtomicSimpleCPU    (IPC=1, functional)
    timing  → gem5 TimingSimpleCPU    (memory timing)
    minor   → gem5 MinorCPU           (in-order pipelined)
    o3      → gem5 DerivO3CPU         (out-of-order)
    big     → wide aggressive core    (Apple M-class)
"""

import sys, os
_root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..")
sys.path.insert(0, _root)
sys.path.insert(0, os.path.join(_root, "python"))

import argparse
from configs.common.options import add_common_options, add_se_options
from configs.common.simulation import run_se

parser = argparse.ArgumentParser(
    description="HELM SE — Armv8-A",
    formatter_class=argparse.ArgumentDefaultsHelpFormatter,
)
add_common_options(parser)
add_se_options(parser)

# Set ARM-specific defaults
parser.set_defaults(binary="assets/binaries/fish")
parser.set_defaults(options="--no-config -c 'echo hello'")

args = parser.parse_args()
run_se(args)
