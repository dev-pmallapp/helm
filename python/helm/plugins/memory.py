"""Memory analysis plugins."""

from __future__ import annotations
from typing import Dict
from helm.plugins.base import PluginBase


class CacheSim(PluginBase):
    """Multi-level cache simulation via memory-access callbacks.

    Parameters
    ----------
    l1d_size : str
        L1 data cache size (e.g. ``"32KB"``).
    l1d_assoc : int
        L1 data cache associativity.
    l2_size : str
        L2 cache size.
    l2_assoc : int
        L2 cache associativity.
    line_size : int
        Cache line size in bytes.
    """

    def __init__(
        self,
        l1d_size: str = "32KB",
        l1d_assoc: int = 8,
        l2_size: str = "256KB",
        l2_assoc: int = 4,
        line_size: int = 64,
    ) -> None:
        super().__init__(
            "cache",
            l1d_size=l1d_size,
            l1d_assoc=l1d_assoc,
            l2_size=l2_size,
            l2_assoc=l2_assoc,
            line_size=line_size,
        )
        self._l1d_hits = 0
        self._l1d_misses = 0
        self._l2_hits = 0
        self._l2_misses = 0
        # A real implementation would build set-associative tag arrays here.

    @property
    def enabled_callbacks(self) -> set:
        return {"mem"}

    def on_mem(self, vcpu: int, vaddr: int, size: int, is_store: bool) -> None:
        # Stub: count everything as L1D hit.
        # Real implementation does tag lookup + LRU.
        self._l1d_hits += 1

    @property
    def l1d_hit_rate(self) -> float:
        total = self._l1d_hits + self._l1d_misses
        return self._l1d_hits / total if total > 0 else 0.0

    @property
    def l2_hit_rate(self) -> float:
        total = self._l2_hits + self._l2_misses
        return self._l2_hits / total if total > 0 else 0.0

    def atexit(self) -> None:
        self.results = {
            "l1d_hits": self._l1d_hits,
            "l1d_misses": self._l1d_misses,
            "l1d_hit_rate": self.l1d_hit_rate,
            "l2_hits": self._l2_hits,
            "l2_misses": self._l2_misses,
            "l2_hit_rate": self.l2_hit_rate,
        }
