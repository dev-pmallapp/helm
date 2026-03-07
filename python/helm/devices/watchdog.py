"""Watchdog devices."""
from helm.devices.base import Device

class Sp805(Device):
    """SP805 watchdog timer."""
    def __init__(self, name: str = "watchdog", **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)
