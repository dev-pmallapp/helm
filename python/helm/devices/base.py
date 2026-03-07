"""Base device and bus classes."""

from __future__ import annotations
from typing import Optional, List


class Device:
    """Base class for all device models.

    Parameters
    ----------
    name : str
        Device name.
    region_size : int
        MMIO region size in bytes.
    base_address : int
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

    def to_dict(self) -> dict:
        return {
            "type": self.__class__.__name__,
            "name": self.name,
            "region_size": self.region_size,
            "base_address": self.base_address,
            "irq": self.irq,
        }

    def __repr__(self) -> str:
        return f"{self.__class__.__name__}({self.name!r}, base=0x{self.base_address:x})"


class Bus(Device):
    """A bus that routes accesses to child devices."""

    def __init__(
        self,
        name: str,
        bridge_latency: int = 0,
        window_size: int = 0x1_0000_0000,
        base_address: int = 0,
    ) -> None:
        super().__init__(name, region_size=window_size, base_address=base_address)
        self.bridge_latency = bridge_latency
        self.children: List[Device] = []

    def attach(self, device: Device, base: int = 0) -> "Bus":
        """Attach a child device. Returns self for chaining."""
        device.base_address = base
        self.children.append(device)
        return self

    def to_dict(self) -> dict:
        d = super().to_dict()
        d["bridge_latency"] = self.bridge_latency
        d["children"] = [c.to_dict() for c in self.children]
        return d


class ApbBus(Bus):
    """AMBA APB bus — low-power peripherals."""
    def __init__(self, name: str = "apb", **kwargs) -> None:
        kwargs.setdefault("bridge_latency", 3)  # bridge + 2 APB cycles
        kwargs.setdefault("window_size", 0x10_0000)
        super().__init__(name, **kwargs)


class AhbBus(Bus):
    """AMBA AHB bus — high-performance."""
    def __init__(self, name: str = "ahb", **kwargs) -> None:
        kwargs.setdefault("bridge_latency", 0)
        super().__init__(name, **kwargs)


class PciBus(Bus):
    """PCI root complex."""
    def __init__(self, name: str = "pci0", **kwargs) -> None:
        kwargs.setdefault("bridge_latency", 1)
        kwargs.setdefault("window_size", 0x1000_0000)
        super().__init__(name, **kwargs)


class UsbBus(Bus):
    """USB host controller."""
    def __init__(self, name: str = "usb0", **kwargs) -> None:
        kwargs.setdefault("bridge_latency", 10)
        kwargs.setdefault("window_size", 0x100_0000)
        super().__init__(name, **kwargs)
