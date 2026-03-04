"""
HELM — Hybrid Emulation Layer for Microarchitecture

Python configuration layer.  Users compose platforms, cores, memory
hierarchies, and devices, attach plugins, then run simulations.

Plugins are enabled via ``sim.add_plugin()``::

    from helm import Simulation
    from helm.plugins import InsnCount, CacheSim

    sim = Simulation(platform, binary="./test", mode="se")
    sim.add_plugin(InsnCount())
    sim.add_plugin(CacheSim(l1d_size="32KB"))
    results = sim.run()
"""

from helm.platform import Platform
from helm.core import Core
from helm.memory import Cache, MemorySystem
from helm.predictor import BranchPredictor
from helm.device import Device
from helm.timing import TimingMode
from helm.simulation import Simulation

__version__ = "0.1.0"

__all__ = [
    "Platform",
    "Core",
    "Cache",
    "MemorySystem",
    "BranchPredictor",
    "Device",
    "TimingMode",
    "Simulation",
]
