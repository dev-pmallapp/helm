"""
Simulation — the main entry point for running HELM simulations from Python.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from helm.platform import Platform
    from helm.plugins.base import PluginBase


@dataclass
class SimResults:
    """Parsed simulation results returned by the Rust engine."""

    cycles: int = 0
    instructions_committed: int = 0
    branches: int = 0
    branch_mispredictions: int = 0
    virtual_cycles: int = 0
    cache_accesses: Dict[int, tuple] = field(default_factory=dict)
    raw: Dict[str, Any] = field(default_factory=dict)

    @property
    def ipc(self) -> float:
        if self.cycles == 0:
            return 0.0
        return self.instructions_committed / self.cycles

    @property
    def branch_mpki(self) -> float:
        if self.instructions_committed == 0:
            return 0.0
        return self.branch_mispredictions / (self.instructions_committed / 1000)

    def __repr__(self) -> str:
        return (
            f"SimResults(cycles={self.cycles}, "
            f"insns={self.instructions_committed}, "
            f"IPC={self.ipc:.3f}, "
            f"branch_mpki={self.branch_mpki:.2f})"
        )


class Simulation:
    """Configure and run a HELM simulation.

    Parameters
    ----------
    platform : Platform
        The platform description.
    binary : str
        Path to the guest binary to execute.
    mode : str
        Execution mode: ``"se"`` or ``"cae"``.
    max_cycles : int
        Maximum simulation cycles.

    Examples
    --------
    ::

        from helm import Simulation
        from helm.plugins import InsnCount, CacheSim

        sim = Simulation(platform, binary="./test", mode="se")
        sim.add_plugin(InsnCount())
        sim.add_plugin(CacheSim(l1d_size="32KB"))
        results = sim.run()

        print(sim.plugin("insn-count").total)
        print(sim.plugin("cache").l1d_hit_rate)
    """

    def __init__(
        self,
        platform: "Platform",
        binary: str,
        *,
        mode: str = "se",
        max_cycles: int = 1_000_000,
    ) -> None:
        self.platform = platform
        self.binary = binary
        self.mode = mode
        self.max_cycles = max_cycles
        self._plugins: List[PluginBase] = []

    # -- Plugin management -----------------------------------------------

    def add_plugin(self, plugin: "PluginBase") -> "Simulation":
        """Attach a plugin.  Returns self for chaining."""
        self._plugins.append(plugin)
        return self

    def plugin(self, name: str) -> "Optional[PluginBase]":
        """Look up an attached plugin by name."""
        for p in self._plugins:
            if p.name == name:
                return p
        return None

    @property
    def plugins(self) -> "List[PluginBase]":
        return list(self._plugins)

    # -- Run -------------------------------------------------------------

    def run(self) -> SimResults:
        """Execute the simulation and return results."""
        try:
            return self._run_native()
        except ImportError:
            return self._run_stub()

    def _run_native(self) -> SimResults:
        """Dispatch to the Rust engine via ``_helm_core``."""
        from helm._helm_core import (
            PlatformConfig as _PlatformConfig,
            CoreConfig as _CoreConfig,
            MemoryConfig as _MemoryConfig,
            CacheConfig as _CacheConfig,
            BranchPredictorConfig as _BPConfig,
            TimingModel as _TimingModel,
            run_simulation,
        )

        def _make_cache(c):
            if c is None:
                return None
            return _CacheConfig(c.size, c.assoc, c.latency, c.line_size)

        def _make_bp(bp):
            kind = bp.kind
            if kind == "Static":
                return _BPConfig.static_pred()
            elif kind == "Bimodal":
                return _BPConfig.bimodal(bp.params.get("table_size", 4096))
            elif kind == "GShare":
                return _BPConfig.gshare(bp.params.get("history_bits", 16))
            elif kind == "TAGE":
                return _BPConfig.tage(bp.params.get("history_length", 64))
            elif kind == "Tournament":
                return _BPConfig.tournament()
            return _BPConfig.static_pred()

        cores = [
            _CoreConfig(
                c.name, c.width, c.rob_size, c.iq_size,
                c.lq_size, c.sq_size, _make_bp(c.branch_predictor),
            )
            for c in self.platform.cores
        ]

        mem = _MemoryConfig(
            self.platform.memory.dram_latency,
            _make_cache(self.platform.memory.l1i),
            _make_cache(self.platform.memory.l1d),
            _make_cache(self.platform.memory.l2),
            _make_cache(self.platform.memory.l3),
        )

        mode_str = "se" if self.mode in ("se", "syscall") else "microarch"
        platform = _PlatformConfig(
            self.platform.name,
            self.platform.isa.kind.lower(),
            mode_str,
            cores,
            mem,
        )

        timing_mode = getattr(self.platform, 'timing', None)
        if timing_mode is not None:
            # Pass the full TimingMode params dict to the Rust engine
            timing_cfg = _TimingModel(
                timing_mode.level.lower(),
                **{k: v for k, v in timing_mode.params.items()},
            )
        else:
            timing_cfg = None
        result_json = run_simulation(platform, self.binary, self.max_cycles, timing_cfg)
        raw = json.loads(result_json)
        results = self._parse_results(raw)

        # Notify plugins
        for p in self._plugins:
            p.atexit()

        return results

    def _run_stub(self) -> SimResults:
        """Pure-Python fallback — validates config and returns empty results."""
        config_dict = self.platform.to_dict()
        config_dict["exec_mode"] = self.mode
        config_dict["plugins"] = [p.to_dict() for p in self._plugins]

        print(f"[HELM stub] Binary: {self.binary}")
        print(f"[HELM stub] Plugins: {[p.name for p in self._plugins]}")
        print(f"[HELM stub] Native engine not available — returning empty results.")

        # Notify plugins
        for p in self._plugins:
            p.atexit()

        return SimResults(raw=config_dict)

    @staticmethod
    def _parse_results(raw: dict) -> SimResults:
        return SimResults(
            cycles=raw.get("cycles", 0),
            instructions_committed=raw.get("instructions_committed", 0),
            branches=raw.get("branches", 0),
            branch_mispredictions=raw.get("branch_mispredictions", 0),
            virtual_cycles=raw.get("virtual_cycles", raw.get("cycles", 0)),
            cache_accesses=raw.get("cache_accesses", {}),
            raw=raw,
        )
