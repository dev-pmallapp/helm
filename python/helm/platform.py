"""Platform — top-level machine descriptor (gem5-style SimObject root)."""

from __future__ import annotations
from typing import List, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from helm.core import Core
    from helm.memory import MemorySystem


class Platform:
    """Describe the simulated machine.

    Parameters
    ----------
    name : str
        Human-readable identifier.
    isa : str
        ``"aarch64"`` (default), ``"riscv64"``.
    cores : list[Core]
        CPU cores.
    memory : MemorySystem
        Memory hierarchy.
    mem_mib : int
        Guest RAM in MiB.

    Examples
    --------
    ::

        from helm import Platform, Core, MemorySystem

        p = Platform(
            name="arm-a57",
            isa="aarch64",
            cores=[Core("a57", width=3, rob_size=60)],
            memory=MemorySystem(dram_latency=50),
        )
    """

    def __init__(
        self,
        name: str = "helm-platform",
        isa: str = "aarch64",
        cores: Optional[List["Core"]] = None,
        memory: Optional["MemorySystem"] = None,
        mem_mib: int = 512,
    ) -> None:
        from helm.core import Core as _Core
        from helm.memory import MemorySystem as _Mem

        self.name    = name
        self.isa     = isa
        self.cores   = cores  or [_Core()]
        self.memory  = memory or _Mem()
        self.mem_mib = mem_mib

    def to_dict(self) -> dict:
        return {
            "name":    self.name,
            "isa":     self.isa,
            "cores":   [c.to_dict() for c in self.cores],
            "memory":  self.memory.to_dict(),
            "mem_mib": self.mem_mib,
        }

    def __repr__(self) -> str:
        return (
            f"Platform({self.name!r}, isa={self.isa!r}, "
            f"cores={len(self.cores)}, mem={self.mem_mib}MiB)"
        )
