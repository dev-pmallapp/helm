"""Simulation runner — assembles config dict and emits JSON for helm-arm.

Mirrors gem5's ``Simulation.py``.
"""

from __future__ import annotations

import json
import os
import shlex
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import argparse

from configs.common.cpu import cpu_factory
from configs.common.caches import L1ICache, L1DCache, L2Cache


def build_config(args: "argparse.Namespace") -> dict:
    """Turn parsed CLI args into the JSON config dict for helm-arm."""

    # CPU + timing
    core, timing = cpu_factory(args.cpu_type, "cpu0")
    cores = []
    for i in range(args.num_cpus):
        c, _ = cpu_factory(args.cpu_type, f"cpu{i}")
        cores.append(c.to_dict())

    # Caches
    memory = {"dram_latency_cycles": args.dram_latency}
    if args.caches:
        memory["l1i"] = L1ICache(args.l1i_size, assoc=args.l1i_assoc,
                                  line_size=args.cacheline_size).to_dict()
        memory["l1d"] = L1DCache(args.l1d_size, assoc=args.l1d_assoc,
                                  line_size=args.cacheline_size).to_dict()
    if args.l2cache:
        memory["l2"] = L2Cache(args.l2_size, assoc=args.l2_assoc,
                                line_size=args.cacheline_size).to_dict()

    # Workload
    binary = args.binary
    cmd = args.cmd or os.path.basename(binary)
    options_tokens = shlex.split(args.options) if args.options else []
    argv = [cmd] + options_tokens

    envp = args.env if args.env is not None else [
        "HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin",
        "LANG=C", "USER=helm",
    ]

    return {
        "binary": binary,
        "argv": argv,
        "envp": envp,
        "max_insns": args.max_insns,
        "platform": {
            "name": f"helm-{args.cpu_type}",
            "isa": "aarch64",
            "cores": cores,
            "memory": memory,
            "timing": timing.to_dict(),
        },
        "plugins": args.plugin,
    }


def run_se(args: "argparse.Namespace"):
    """Build config from args and print JSON for helm-arm to consume."""
    config = build_config(args)
    print(json.dumps(config))
