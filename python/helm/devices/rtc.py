"""RTC devices."""
from helm.devices.base import Device

class Pl031Rtc(Device):
    """PL031 real-time clock."""
    def __init__(self, name: str = "rtc", *, initial_time: int = 0, **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)
        self.initial_time = initial_time

    def to_dict(self):
        d = super().to_dict()
        d["initial_time"] = self.initial_time
        return d
