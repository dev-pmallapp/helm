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


class Bus(Device):
    """A bus that routes accesses to child devices, enabling hierarchical topologies.

    Each bus level adds ``bridge_latency`` stall cycles to every access
    that crosses it, modelling real bus/protocol overhead (PCI, USB, SPI).

    Because ``Bus`` is a ``Device``, buses nest naturally::

        pci = PciBus("pci0")
        pci.attach(gpu)
        pci.attach(nic)
        platform = Platform(..., devices=[uart, pci])
    """

    def __init__(
        self,
        name: str,
        bridge_latency: int = 0,
        window_size: int = 0x1_0000_0000,
        base_address: int = 0,
    ) -> None:
        super().__init__(name, region_size=window_size, base_address=base_address)
        self.bridge_latency = bridge_latency
        self.children: list[Device] = []

    def attach(self, device: "Device") -> "Bus":
        """Attach a child device to this bus.  Returns self for chaining."""
        self.children.append(device)
        return self

    def to_dict(self) -> dict:
        d = super().to_dict()
        d["bridge_latency"] = self.bridge_latency
        d["children"] = [c.to_dict() for c in self.children]
        return d

    def __repr__(self) -> str:
        kids = ", ".join(c.name for c in self.children)
        return (
            f"{self.__class__.__name__}({self.name!r}, "
            f"latency={self.bridge_latency}, children=[{kids}])"
        )


class PciBus(Bus):
    """PCI root complex: 1-cycle crossing latency, 256 MB default window."""

    def __init__(self, name: str = "pci0", **kwargs) -> None:
        kwargs.setdefault("bridge_latency", 1)
        kwargs.setdefault("window_size", 0x1000_0000)
        super().__init__(name, **kwargs)


class UsbBus(Bus):
    """USB host controller: 10-cycle protocol overhead, 16 MB window."""

    def __init__(self, name: str = "usb0", **kwargs) -> None:
        kwargs.setdefault("bridge_latency", 10)
        kwargs.setdefault("window_size", 0x100_0000)
        super().__init__(name, **kwargs)


class AcceleratorDevice(Device):
    """LLVM-IR hardware accelerator exposed as an MMIO device.

    The accelerator's C/C++ source is compiled to LLVM IR and executed
    in a cycle-accurate manner by ``helm-llvm``.  The CPU triggers it
    by writing to the CONTROL register; elapsed cycles appear as device
    stall in the timing model.

    Parameters
    ----------
    name : str
        Device name.
    ir_file : str
        Path to the ``.ll`` LLVM IR file.
    base_address : int
        MMIO base address on the bus.
    **fu_config
        Functional unit overrides (e.g. ``int_adders=4``).
    """

    def __init__(
        self,
        name: str,
        ir_file: str,
        base_address: int = 0,
        **fu_config: int,
    ) -> None:
        super().__init__(name, region_size=0x100, base_address=base_address)
        self.ir_file = ir_file
        self.fu_config = fu_config

    def to_dict(self) -> dict:
        d = super().to_dict()
        d["ir_file"] = self.ir_file
        d["fu_config"] = self.fu_config
        return d
