"""
helm.aarch64 — AArch64-specific SimObject classes.

CPU models, devices, and pre-built boards for ARM AArch64.
"""

from helm.aarch64.cpu import A55Cpu, A73Cpu, A78Cpu, A510Cpu, A710Cpu, NeoverseN1Cpu
from helm.aarch64.devices import GicV2, Pl011

__all__ = [
    # CPU models
    "A55Cpu",
    "A73Cpu",
    "A78Cpu",
    "A510Cpu",
    "A710Cpu",
    "NeoverseN1Cpu",
    # Devices
    "GicV2",
    "Pl011",
]
