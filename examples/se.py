#!/usr/bin/env python3
"""
HELM SE-mode configuration — gem5-style.

Usage:
    helm-arm examples/se.py                          # defaults
    helm-arm examples/se.py -- --binary ./my-arm-elf
    helm-arm examples/se.py -- --cpu-type detailed --l1d-size 64KB

This script mirrors gem5's configs/example/se.py:
  - Builds a complete platform (CPU, caches, memory, timing)
  - Configures the workload (binary, argv, envp)
  - Selects accuracy level (FE/APE/CAE)
  - Enables plugins (instruction count, cache sim, etc.)
  - Emits JSON config consumed by helm-arm

gem5 equivalences:
  gem5 AtomicSimpleCPU   → TimingModel.fe()
  gem5 TimingSimpleCPU   → TimingModel.ape()
  gem5 DerivO3CPU        → TimingModel.ape(detailed params)
  gem5 SE mode           → mode="se"
"""

from __future__ import annotations

import argparse
import json
import os
import sys

# ── Argument parsing (mirrors gem5 se.py options) ────────────────────────

parser = argparse.ArgumentParser(
    description="HELM SE-mode simulation configuration",
    formatter_class=argparse.ArgumentDefaultsHelpFormatter,
)

# Workload
parser.add_argument("--binary", "-b", default="assets/binaries/fish",
                    help="Path to the AArch64 static binary to execute")
parser.add_argument("--cmd", "-c", default=None,
                    help="Override argv[0] (defaults to binary basename)")
parser.add_argument("--options", "-o", default="--no-config -c 'echo hello'",
                    help="Arguments passed to the guest binary")
parser.add_argument("--env", nargs="*", default=None,
                    help="Environment variables (KEY=VAL). "
                         "Defaults to HOME=/tmp TERM=dumb etc.")

# CPU / Timing
parser.add_argument("--cpu-type", default="atomic",
                    choices=["atomic", "timing", "detailed"],
                    help="CPU model: atomic=FE, timing=APE, detailed=APE+params")
parser.add_argument("--num-cpus", type=int, default=1,
                    help="Number of CPU cores (currently only 1 supported in SE)")

# Pipeline (detailed mode)
parser.add_argument("--width", type=int, default=4,
                    help="Issue/dispatch width")
parser.add_argument("--rob-size", type=int, default=192,
                    help="Reorder buffer entries")
parser.add_argument("--iq-size", type=int, default=64,
                    help="Issue queue entries")
parser.add_argument("--lq-size", type=int, default=32,
                    help="Load queue entries")
parser.add_argument("--sq-size", type=int, default=32,
                    help="Store queue entries")
parser.add_argument("--bp-type", default="tournament",
                    choices=["static", "bimodal", "gshare", "tage", "tournament"],
                    help="Branch predictor type")

# Caches
parser.add_argument("--caches", action="store_true", default=True,
                    help="Enable L1 caches")
parser.add_argument("--no-caches", dest="caches", action="store_false",
                    help="Disable L1 caches")
parser.add_argument("--l2cache", action="store_true", default=False,
                    help="Enable unified L2 cache")
parser.add_argument("--l1d-size", default="64KB", help="L1 data cache size")
parser.add_argument("--l1i-size", default="32KB", help="L1 instruction cache size")
parser.add_argument("--l1d-assoc", type=int, default=4, help="L1D associativity")
parser.add_argument("--l1i-assoc", type=int, default=4, help="L1I associativity")
parser.add_argument("--l2-size", default="256KB", help="L2 cache size")
parser.add_argument("--l2-assoc", type=int, default=8, help="L2 associativity")
parser.add_argument("--cacheline-size", type=int, default=64,
                    help="Cache line size in bytes")

# Memory
parser.add_argument("--mem-size", default="512MB",
                    help="Physical memory size (informational)")
parser.add_argument("--dram-latency", type=int, default=100,
                    help="DRAM access latency in cycles")

# Simulation control
parser.add_argument("--max-insns", type=int, default=50_000_000,
                    help="Maximum instructions to execute")

# Plugins
parser.add_argument("--plugin", action="append", default=[],
                    help="Enable a plugin (insn-count, execlog, hotblocks, "
                         "cache, syscall-trace). Repeatable.")

args = parser.parse_args()

# ── Build the configuration ──────────────────────────────────────────────

# Workload
binary = args.binary
cmd = args.cmd or os.path.basename(binary)
import shlex; options_tokens = shlex.split(args.options) if args.options else []
argv = [cmd] + options_tokens

if args.env is not None:
    envp = args.env
else:
    envp = [
        "HOME=/tmp",
        "TERM=dumb",
        "PATH=/usr/bin:/bin",
        "LANG=C",
        "USER=helm",
    ]

# Timing model
timing = {"level": "FE"}
if args.cpu_type == "timing":
    timing = {"level": "APE"}
elif args.cpu_type == "detailed":
    timing = {
        "level": "APE",
        "int_alu_latency": 1,
        "int_mul_latency": 3,
        "int_div_latency": 12,
        "fp_alu_latency": 4,
        "fp_mul_latency": 5,
        "fp_div_latency": 15,
        "load_latency": 4,
        "store_latency": 1,
        "branch_penalty": 10,
        "l1_latency": 3,
        "l2_latency": 12,
        "l3_latency": 40,
        "dram_latency": args.dram_latency,
    }

# Caches
caches = {}
if args.caches:
    caches["l1i"] = {
        "size": args.l1i_size,
        "associativity": args.l1i_assoc,
        "latency_cycles": 1,
        "line_size": args.cacheline_size,
    }
    caches["l1d"] = {
        "size": args.l1d_size,
        "associativity": args.l1d_assoc,
        "latency_cycles": 4,
        "line_size": args.cacheline_size,
    }
if args.l2cache:
    caches["l2"] = {
        "size": args.l2_size,
        "associativity": args.l2_assoc,
        "latency_cycles": 12,
        "line_size": args.cacheline_size,
    }

# Core
core = {
    "name": "cpu0",
    "width": args.width,
    "rob_size": args.rob_size,
    "iq_size": args.iq_size,
    "lq_size": args.lq_size,
    "sq_size": args.sq_size,
    "branch_predictor": {"kind": args.bp_type.capitalize()},
}

# Memory system
memory = {
    "dram_latency_cycles": args.dram_latency,
    **caches,
}

# ── Assemble final config ────────────────────────────────────────────────

config = {
    "binary": binary,
    "argv": argv,
    "envp": envp,
    "max_insns": args.max_insns,
    "platform": {
        "name": f"helm-se-{args.cpu_type}",
        "isa": "aarch64",
        "cores": [core] * args.num_cpus,
        "memory": memory,
        "timing": timing,
    },
    "plugins": args.plugin,
}

# ── Emit JSON for helm-arm ───────────────────────────────────────────────

print(json.dumps(config))
