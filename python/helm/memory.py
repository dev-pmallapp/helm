"""
Cache and MemorySystem — memory hierarchy configuration.
"""

from __future__ import annotations
from typing import Optional


class Cache:
    """A single cache level.

    Parameters
    ----------
    size : str
        Human-readable size (e.g. ``"32KB"``, ``"8MB"``).
    assoc : int
        Set associativity.
    latency : int
        Access latency in cycles.
    line_size : int
        Cache-line size in bytes.
    """

    def __init__(
        self,
        size: str = "32KB",
        *,
        assoc: int = 8,
        latency: int = 1,
        line_size: int = 64,
    ) -> None:
        self.size = size
        self.assoc = assoc
        self.latency = latency
        self.line_size = line_size

    def to_dict(self) -> dict:
        return {
            "size": self.size,
            "associativity": self.assoc,
            "latency_cycles": self.latency,
            "line_size": self.line_size,
        }

    def __repr__(self) -> str:
        return f"Cache({self.size}, {self.assoc}-way, {self.latency}cyc)"


class MemorySystem:
    """Hierarchical memory subsystem.

    Parameters
    ----------
    l1i, l1d, l2, l3 : Cache, optional
        Cache levels.  ``None`` means the level is absent.
    dram_latency : int
        Main-memory access latency in cycles.
    """

    def __init__(
        self,
        *,
        l1i: Optional[Cache] = None,
        l1d: Optional[Cache] = None,
        l2: Optional[Cache] = None,
        l3: Optional[Cache] = None,
        dram_latency: int = 100,
    ) -> None:
        self.l1i = l1i
        self.l1d = l1d
        self.l2 = l2
        self.l3 = l3
        self.dram_latency = dram_latency

    def to_dict(self) -> dict:
        return {
            "l1i": self.l1i.to_dict() if self.l1i else None,
            "l1d": self.l1d.to_dict() if self.l1d else None,
            "l2": self.l2.to_dict() if self.l2 else None,
            "l3": self.l3.to_dict() if self.l3 else None,
            "dram_latency_cycles": self.dram_latency,
        }

    def __repr__(self) -> str:
        levels = []
        if self.l1i:
            levels.append(f"L1i={self.l1i.size}")
        if self.l1d:
            levels.append(f"L1d={self.l1d.size}")
        if self.l2:
            levels.append(f"L2={self.l2.size}")
        if self.l3:
            levels.append(f"L3={self.l3.size}")
        return f"MemorySystem({', '.join(levels)}, dram={self.dram_latency}cyc)"
