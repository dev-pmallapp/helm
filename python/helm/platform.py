"""
Platform — top-level system configuration, analogous to gem5's Board/System.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, List, Optional

if TYPE_CHECKING:
    from helm.core import Core
    from helm.device import Device
    from helm.memory import MemorySystem
    from helm.isa import IsaBase
    from helm.timing import TimingMode


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
    devices : list[Device], optional
        Devices mapped on the system bus.
    timing : TimingMode, optional
        Simulation accuracy level.
    """

    def __init__(
        self,
        name: str,
        isa: "IsaBase",
        cores: "List[Core]",
        memory: "MemorySystem",
        devices: "Optional[List[Device]]" = None,
        timing: "Optional[TimingMode]" = None,
    ) -> None:
        self.name = name
        self.isa = isa
        self.cores = list(cores)
        self.memory = memory
        self.devices: List[Device] = list(devices or [])
        from helm.timing import TimingMode as _TM
        self.timing = timing or _TM.functional()

    def add_device(self, device: "Device") -> "Platform":
        """Attach a device to the platform bus. Returns self for chaining."""
        self.devices.append(device)
        return self

    def to_dict(self) -> dict:
        """Serialise to a plain dict (matches Rust ``PlatformConfig``)."""
        return {
            "name": self.name,
            "isa": self.isa.kind,
            "exec_mode": "SyscallEmulation",
            "cores": [c.to_dict() for c in self.cores],
            "memory": self.memory.to_dict(),
            "devices": [d.to_dict() for d in self.devices],
            "timing": self.timing.to_dict(),
        }

    def __repr__(self) -> str:
        return (
            f"Platform(name={self.name!r}, isa={self.isa}, "
            f"cores={len(self.cores)}, devices={len(self.devices)}, "
            f"timing={self.timing})"
        )
