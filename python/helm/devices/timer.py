"""Timer devices."""
from helm.devices.base import Device

class Sp804Timer(Device):
    """SP804 dual timer."""
    def __init__(self, name: str = "timer", **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)

class BcmSysTimer(Device):
    """BCM2837 system timer."""
    def __init__(self, name: str = "sys-timer", **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)
