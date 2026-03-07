"""System registers."""
from helm.devices.base import Device

class RealViewSysRegs(Device):
    """RealView Platform Baseboard system registers."""
    def __init__(self, name: str = "sysregs", *, board_id: int = 0x0178_0000, **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)
        self.board_id = board_id

    def to_dict(self):
        d = super().to_dict()
        d["board_id"] = self.board_id
        return d
