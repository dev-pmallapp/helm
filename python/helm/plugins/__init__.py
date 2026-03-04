"""
HELM plugin system — Python interface.

Plugins are enabled by adding them to a Simulation before running::

    from helm import Simulation
    from helm.plugins import InsnCount, ExecLog, CacheSim, HotBlocks

    sim = Simulation(platform, binary="./test", mode="se")
    sim.add_plugin(InsnCount())
    sim.add_plugin(ExecLog(output="trace.log"))
    sim.add_plugin(CacheSim(l1d_size="32KB"))
    results = sim.run()

    print(sim.plugin("insn-count").total)
    print(sim.plugin("cache").l1d_hit_rate)
"""

from helm.plugins.base import PluginBase
from helm.plugins.trace import InsnCount, ExecLog, HotBlocks, HowVec, SyscallTrace
from helm.plugins.memory import CacheSim

__all__ = [
    "PluginBase",
    "InsnCount",
    "ExecLog",
    "HotBlocks",
    "HowVec",
    "SyscallTrace",
    "CacheSim",
]
