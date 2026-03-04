"""
Device — base class for user-defined MMIO devices.

Developers subclass :class:`Device` to create custom peripherals
for their simulated platform.  This mirrors the Rust-side
``MemoryMappedDevice`` trait.

Example::

    from helm.device import Device

    class Timer(Device):
        def __init__(self):
            super().__init__("my-timer", region_size=0x100)
            self.counter = 0

        def read(self, offset, size):
            if offset == 0x00:
                return self.counter
            return 0

        def write(self, offset, size, value):
            if offset == 0x00:
                self.counter = value
"""

from __future__ import annotations
from typing import Optional


class Device:
    """Base class for Python-side device models.

    Parameters
    ----------
    name : str
        Device name (e.g. ``"uart0"``).
    region_size : int
        Size of the MMIO region in bytes.
    base_address : int, optional
        Where on the bus this device is mapped.
    irq : int, optional
        Interrupt line number.
    """

    def __init__(
        self,
        name: str,
        region_size: int = 0x1000,
        base_address: int = 0,
        irq: Optional[int] = None,
    ) -> None:
        self.name = name
        self.region_size = region_size
        self.base_address = base_address
        self.irq = irq

    # -- Override these in subclasses ------------------------------------

    def read(self, offset: int, size: int) -> int:
        """Read a register.  ``offset`` is relative to ``base_address``."""
        return 0

    def write(self, offset: int, size: int, value: int) -> None:
        """Write a register."""

    def reset(self) -> None:
        """Reset device to power-on state."""

    def init(self) -> None:
        """Called once before simulation starts."""

    # -- Serialisation ---------------------------------------------------

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "region_size": self.region_size,
            "base_address": self.base_address,
            "irq": self.irq,
            "type": self.__class__.__name__,
        }

    def __repr__(self) -> str:
        return (
            f"{self.__class__.__name__}({self.name!r}, "
            f"base=0x{self.base_address:x}, size=0x{self.region_size:x})"
        )
