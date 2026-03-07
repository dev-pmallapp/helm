"""Mailbox devices."""
from helm.devices.base import Device

class BcmMailbox(Device):
    """BCM2837 VideoCore mailbox."""
    def __init__(self, name: str = "mailbox", **kwargs):
        super().__init__(name, region_size=0x1000, **kwargs)
