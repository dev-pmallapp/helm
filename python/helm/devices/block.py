"""Block devices."""
from helm.devices.base import Device
from helm.backends.block import BlockBackend, MemoryBlockBackend


class VirtioBlk(Device):
    """VirtIO block device."""
    def __init__(self, name: str = "disk0", *, backend: BlockBackend = None,
                 base_address: int = 0, irq: int = None):
        super().__init__(name, region_size=0x200, base_address=base_address, irq=irq)
        self.backend = backend or MemoryBlockBackend(0)

    def to_dict(self):
        d = super().to_dict()
        d["backend"] = self.backend.to_dict()
        return d
