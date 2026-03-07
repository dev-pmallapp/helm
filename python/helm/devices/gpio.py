"""GPIO devices."""
from helm.devices.base import Device

class Pl061Gpio(Device):
    """PL061 GPIO controller (8 pins)."""
    def __init__(self, name: str = "gpio", **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)

class BcmGpio(Device):
    """BCM2837 GPIO (54 pins)."""
    def __init__(self, name: str = "gpio", **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)
