"""CLI option helpers — mirrors gem5's ``Options.py``.

Provides ``add_common_options`` and ``add_se_options`` that attach
argparse groups to a parser, keeping individual configs short.
"""

import argparse
from configs.common.cpu import list_cpu_types


def add_common_options(parser: argparse.ArgumentParser):
    """Options shared across all simulation modes."""

    # CPU
    cpu_grp = parser.add_argument_group("CPU")
    cpu_grp.add_argument("--cpu-type", default="atomic",
                         choices=list_cpu_types(),
                         help="CPU model preset")
    cpu_grp.add_argument("--num-cpus", type=int, default=1,
                         help="Number of CPU cores")

    # Caches
    cache_grp = parser.add_argument_group("Caches")
    cache_grp.add_argument("--caches", action="store_true", default=True,
                           help="Enable L1 caches")
    cache_grp.add_argument("--no-caches", dest="caches", action="store_false")
    cache_grp.add_argument("--l2cache", action="store_true", default=False,
                           help="Enable unified L2 cache")
    cache_grp.add_argument("--l1d-size", default="64KB")
    cache_grp.add_argument("--l1i-size", default="32KB")
    cache_grp.add_argument("--l1d-assoc", type=int, default=4)
    cache_grp.add_argument("--l1i-assoc", type=int, default=4)
    cache_grp.add_argument("--l2-size", default="256KB")
    cache_grp.add_argument("--l2-assoc", type=int, default=8)
    cache_grp.add_argument("--cacheline-size", type=int, default=64)

    # Memory
    mem_grp = parser.add_argument_group("Memory")
    mem_grp.add_argument("--mem-size", default="512MB",
                         help="Physical memory size")
    mem_grp.add_argument("--dram-latency", type=int, default=100,
                         help="DRAM access latency in cycles")

    # Simulation
    sim_grp = parser.add_argument_group("Simulation")
    sim_grp.add_argument("--max-insns", type=int, default=50_000_000,
                         help="Maximum instructions to execute")
    sim_grp.add_argument("--plugin", action="append", default=[],
                         help="Enable a plugin (repeatable)")


def add_se_options(parser: argparse.ArgumentParser):
    """Syscall-emulation specific options."""
    se_grp = parser.add_argument_group("SE workload")
    se_grp.add_argument("--binary", "-b",
                        help="Path to AArch64 static binary")
    se_grp.add_argument("--cmd", default=None,
                        help="Override argv[0]")
    se_grp.add_argument("--options", "-o", default="",
                        help="Arguments to the guest binary (shell-quoted)")
    se_grp.add_argument("--env", nargs="*", default=None,
                        help="Environment variables (KEY=VAL ...)")
