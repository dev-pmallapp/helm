"""UART devices — PL011 and BCM Mini UART."""
from helm.devices.base import Device
from helm.backends.char import CharBackend, NullBackend


class Pl011(Device):
    """ARM PL011 UART."""
    def __init__(self, name: str = "uart0", *, backend: CharBackend = None,
                 base_address: int = 0, irq: int = None):
        super().__init__(name, region_size=0x1000, base_address=base_address, irq=irq)
        self.backend = backend or NullBackend()

    def to_dict(self):
        d = super().to_dict()
        d["backend"] = self.backend.to_dict()
        return d


class BcmMiniUart(Device):
    """BCM2837 Mini UART (16550-like)."""
    def __init__(self, name: str = "uart1", *, backend: CharBackend = None,
                 base_address: int = 0, irq: int = None):
        super().__init__(name, region_size=0x1000, base_address=base_address, irq=irq)
        self.backend = backend or NullBackend()

    def to_dict(self):
        d = super().to_dict()
        d["backend"] = self.backend.to_dict()
        return d
