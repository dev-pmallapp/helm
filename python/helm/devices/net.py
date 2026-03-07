"""Network devices."""
from helm.devices.base import Device
from helm.backends.net import NetBackend, NullNetBackend


class VirtioNet(Device):
    """VirtIO network device."""
    def __init__(self, name: str = "nic0", *, backend: NetBackend = None,
                 mac: str = "52:54:00:12:34:56", base_address: int = 0, irq: int = None):
        super().__init__(name, region_size=0x200, base_address=base_address, irq=irq)
        self.backend = backend or NullNetBackend()
        self.mac = mac

    def to_dict(self):
        d = super().to_dict()
        d["backend"] = self.backend.to_dict()
        d["mac"] = self.mac
        return d
