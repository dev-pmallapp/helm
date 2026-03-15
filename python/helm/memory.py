"""Memory hierarchy configuration."""

from __future__ import annotations
from typing import Optional


class Cache:
    """Cache level descriptor."""

    def __init__(
        self,
        size: str = "32KB",
        assoc: int = 8,
        latency: int = 4,
        line_size: int = 64,
    ) -> None:
        self.size      = size
        self.assoc     = assoc
        self.latency   = latency
        self.line_size = line_size

    def to_dict(self) -> dict:
        return {
            "size":      self.size,
            "assoc":     self.assoc,
            "latency":   self.latency,
            "line_size": self.line_size,
        }

    def __repr__(self) -> str:
        return f"Cache({self.size}, assoc={self.assoc}, lat={self.latency})"


class MemorySystem:
    """Memory hierarchy: caches + DRAM.

    Parameters
    ----------
    dram_latency : int
        DRAM access latency in cycles.
    l1i, l1d, l2, l3 : Cache, optional
        Cache levels.  ``None`` means absent.
    """

    def __init__(
        self,
        dram_latency: int = 100,
        l1i: Optional[Cache] = None,
        l1d: Optional[Cache] = None,
        l2:  Optional[Cache] = None,
        l3:  Optional[Cache] = None,
    ) -> None:
        self.dram_latency = dram_latency
        self.l1i = l1i or Cache("32KB",  8, 4)
        self.l1d = l1d or Cache("32KB",  8, 4)
        self.l2  = l2  or Cache("256KB", 8, 12)
        self.l3  = l3

    def to_dict(self) -> dict:
        return {
            "dram_latency": self.dram_latency,
            "l1i": self.l1i.to_dict() if self.l1i else None,
            "l1d": self.l1d.to_dict() if self.l1d else None,
            "l2":  self.l2.to_dict()  if self.l2  else None,
            "l3":  self.l3.to_dict()  if self.l3  else None,
        }
