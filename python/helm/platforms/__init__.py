"""Pre-built platform configurations.

Each builder returns a ``Platform`` descriptor for the SE-mode config
path.  For FS-mode, pass the machine name directly to ``FsSession``
which creates the Rust-side device topology::

    from helm.session import FsSession
    s = FsSession("vmlinuz", machine="virt")

Available FS machines (backed by Rust device models):

- ``"virt"`` / ``"arm-virt"`` — QEMU-style ARM virt (GIC + PL011)
- ``"realview-pb"`` / ``"realview"`` — ARM RealView Platform Baseboard
- ``"rpi3"`` / ``"raspi3"`` — Raspberry Pi 3 (BCM2837)
"""

from helm.platforms.arm_virt import arm_virt
from helm.platforms.realview_pb import realview_pb
from helm.platforms.rpi3 import rpi3

#: Machine names accepted by ``FsSession(machine=...)``.
MACHINES = ["virt", "arm-virt", "realview-pb", "realview", "rpi3", "raspi3"]


def list_machines():
    """Return available FS machine names (backed by Rust device models).

    When the native engine is available this delegates to the Rust
    ``list_platforms()`` binding; otherwise returns the hardcoded list.
    """
    try:
        from helm._helm_core import list_platforms
        return list_platforms()
    except ImportError:
        return list(MACHINES)
