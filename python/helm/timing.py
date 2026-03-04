"""
TimingMode — select the simulation accuracy level.

HELM provides three accuracy levels:

- **FE** (Functional Emulation): IPC=1, maximum speed, no timing.
  Like QEMU — run binaries fast with no microarchitectural detail.
- **APE** (Approximate Emulation): Cache latencies, device stalls,
  optional simplified pipeline.  Like Simics.
- **CAE** (Cycle-Accurate Emulation): Full pipeline stages,
  dependencies, speculation.  Like gem5 O3CPU.

The execution mode is orthogonal:

- **SE** (Syscall Emulation): Run user-mode binaries with Linux
  syscalls emulated (like qemu-user or gem5 SE mode).
"""

from __future__ import annotations


class TimingMode:
    """Describes the simulation accuracy level.

    Use the class methods::

        mode = TimingMode.fe()    # L0: functional, fastest
        mode = TimingMode.ape()   # L1-L2: approximate
        mode = TimingMode.cae()   # L3: cycle-accurate
    """

    def __init__(self, level: str, **params: int) -> None:
        self.level = level
        self.params = params

    # -- Tier factories --------------------------------------------------

    @classmethod
    def fe(cls) -> "TimingMode":
        """FE: Functional Emulation.  IPC=1, no memory modelling.  100-1000 MIPS."""
        return cls("FE")

    @classmethod
    def ape(cls, **kwargs: int) -> "TimingMode":
        """APE: Approximate Emulation.  Cache latencies, device delays.  1-100 MIPS."""
        return cls("APE", **kwargs)

    @classmethod
    def cae(cls) -> "TimingMode":
        """CAE: Cycle-Accurate Emulation.  Full pipeline detail.  0.1-1 MIPS."""
        return cls("CAE")

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
