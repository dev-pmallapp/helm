"""Interrupt controllers."""
from helm.devices.base import Device

class Gic(Device):
    """ARM GICv2 interrupt controller."""
    def __init__(self, name: str = "gic", *, num_irqs: int = 96, **kwargs):
        super().__init__(name, region_size=0x2000, **kwargs)
        self.num_irqs = num_irqs

    def to_dict(self):
        d = super().to_dict()
        d["num_irqs"] = self.num_irqs
        return d
