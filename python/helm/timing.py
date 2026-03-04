"""
TimingMode — select the simulation accuracy level.
"""

from __future__ import annotations


class TimingMode:
    """Describes the timing accuracy level for the simulation.

    Use the class methods to select a level::

        mode = TimingMode.functional()        # IPC=1, fastest
        mode = TimingMode.stall_annotated()   # cache latencies
        mode = TimingMode.microarchitectural()# OoO pipeline
        mode = TimingMode.cycle_accurate()    # full detail
    """

    def __init__(self, level: str, **params: int) -> None:
        self.level = level
        self.params = params

    @classmethod
    def functional(cls) -> "TimingMode":
        """IPC=1, no memory modelling.  100-1000 MIPS."""
        return cls("Functional")

    @classmethod
    def stall_annotated(cls, **kwargs: int) -> "TimingMode":
        """Cache hit/miss latencies, device delays.  10-100 MIPS."""
        return cls("StallAnnotated", **kwargs)

    @classmethod
    def microarchitectural(cls) -> "TimingMode":
        """OoO pipeline, branch prediction, detailed caches.  1-10 MIPS."""
        return cls("Microarchitectural")

    @classmethod
    def cycle_accurate(cls) -> "TimingMode":
        """Cycle-by-cycle pipeline, bypass network.  0.1-1 MIPS."""
        return cls("CycleAccurate")

    def to_dict(self) -> dict:
        if self.params:
            return {"level": self.level, **self.params}
        return {"level": self.level}

    def __repr__(self) -> str:
        if self.params:
            p = ", ".join(f"{k}={v}" for k, v in self.params.items())
            return f"TimingMode.{self.level.lower()}({p})"
        return f"TimingMode.{self.level.lower()}()"
