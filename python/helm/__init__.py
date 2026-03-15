"""
helm — Python configuration layer for the helm-ng simulator.

Gem5-style: Python describes the machine, Rust simulates it.

Usage::

    import helm

    sim = helm.build_simulation(isa="aarch64", mode="se")
    sim.load_elf("./hello", ["hello"], ["HOME=/tmp"])
    while not sim.has_exited:
        sim.run(10_000_000)
    print(f"exit {sim.exit_code} after {sim.insn_count:,} insns")
"""

# Re-export the native _helm_ng functions at the top level
# so users can do `helm.build_simulation(...)` or `helm.Simulation`.
import _helm_ng
from _helm_ng import Simulation, build_simulation

from helm.platform import Platform
from helm.core import Core
from helm.memory import Cache, MemorySystem

__version__ = "0.1.0"

__all__ = [
    "Simulation",
    "build_simulation",
    "Platform",
    "Core",
    "Cache",
    "MemorySystem",
]
