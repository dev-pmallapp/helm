"""
HELM — Hybrid Emulation Layer for Microarchitecture

Python configuration layer.  Users compose platforms, cores, and memory
hierarchies using the classes below, then hand them to `Simulation.run()`
which delegates to the Rust engine via PyO3.

Usage example::

    from helm import Platform, Core, Cache, MemorySystem, Simulation
    from helm.isa import RiscV

    core = Core("ooo-core", width=4, rob_size=128)
    mem  = MemorySystem(
        l1i=Cache("32KB", assoc=8, latency=1),
        l1d=Cache("32KB", assoc=8, latency=1),
        l2=Cache("256KB", assoc=4, latency=10),
    )
    platform = Platform("my-experiment", isa=RiscV(), cores=[core], memory=mem)
    results  = Simulation(platform, binary="./a.out").run()
    print(results)
"""

from helm.platform import Platform
from helm.core import Core
from helm.memory import Cache, MemorySystem
from helm.predictor import BranchPredictor
from helm.simulation import Simulation

__version__ = "0.1.0"

__all__ = [
    "Platform",
    "Core",
    "Cache",
    "MemorySystem",
    "BranchPredictor",
    "Simulation",
]
