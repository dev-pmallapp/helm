"""
TimingMode — select the simulation accuracy level.

HELM provides three named accuracy tiers:

- **Express** (L0): Functional emulation, IPC=1, maximum speed.
  Like QEMU — run binaries fast with no microarchitectural detail.
- **Recon** (L1-L2): Reconnaissance-grade approximate timing.
  Like Simics — cache latencies, device stalls, optional simplified pipeline.
- **Signal** (L2-L3): Signal-accurate cycle-level detail.
  Like gem5 O3CPU — full pipeline stages, dependencies, speculation.
"""

from __future__ import annotations


class TimingMode:
    """Describes the simulation accuracy level.

    Use the class methods to select a tier::

        mode = TimingMode.express()    # L0: fastest, no timing
        mode = TimingMode.recon()      # L1-L2: approximate
        mode = TimingMode.signal()     # L2-L3: cycle-accurate
    """

    def __init__(self, level: str, **params: int) -> None:
        self.level = level
        self.params = params

    # -- Tier factories --------------------------------------------------

    @classmethod
    def express(cls) -> "TimingMode":
        """L0: IPC=1, no memory modelling.  100-1000 MIPS."""
        return cls("Express")

    @classmethod
    def recon(cls, **kwargs: int) -> "TimingMode":
        """L1-L2: Cache latencies, device delays, optional pipeline.  1-100 MIPS."""
        return cls("Recon", **kwargs)

    @classmethod
    def signal(cls) -> "TimingMode":
        """L2-L3: Cycle-accurate pipeline, bypass network.  0.1-1 MIPS."""
        return cls("Signal")

    # -- Serialisation ---------------------------------------------------

    def to_dict(self) -> dict:
        if self.params:
            return {"level": self.level, **self.params}
        return {"level": self.level}

    def __repr__(self) -> str:
        if self.params:
            p = ", ".join(f"{k}={v}" for k, v in self.params.items())
            return f"TimingMode.{self.level.lower()}({p})"
        return f"TimingMode.{self.level.lower()}()"
