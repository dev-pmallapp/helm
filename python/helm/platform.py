"""
Platform — top-level system configuration, analogous to gem5's Board/System.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, List, Optional

if TYPE_CHECKING:
    from helm.core import Core
    from helm.memory import MemorySystem
    from helm.isa import IsaBase


class Platform:
    """Describes a complete simulated system.

    Parameters
    ----------
    name : str
        Human-readable name for this platform configuration.
    isa : IsaBase
        The ISA frontend to use (e.g. ``RiscV()``, ``X86()``, ``Arm()``).
    cores : list[Core]
        One or more core configurations.
    memory : MemorySystem
        Memory hierarchy description.
    """

    def __init__(
        self,
        name: str,
        isa: "IsaBase",
        cores: "List[Core]",
        memory: "MemorySystem",
    ) -> None:
        self.name = name
        self.isa = isa
        self.cores = list(cores)
        self.memory = memory

    def to_dict(self) -> dict:
        """Serialise to a plain dict (matches Rust ``PlatformConfig``)."""
        return {
            "name": self.name,
            "isa": self.isa.kind,
            "exec_mode": "SyscallEmulation",  # default; overridden by Simulation
            "cores": [c.to_dict() for c in self.cores],
            "memory": self.memory.to_dict(),
        }

    def __repr__(self) -> str:
        return (
            f"Platform(name={self.name!r}, isa={self.isa}, "
            f"cores={len(self.cores)}, memory={self.memory})"
        )
