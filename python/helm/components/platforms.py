"""Pre-built platform templates."""

from helm.platform import Platform
from helm.isa import RiscV, X86, Arm
from helm.components.cores import SmallOoOCore, BigOoOCore
from helm.components.memory import TypicalDesktop, ServerMemory


def SingleCoreRiscV(name: str = "single-rv64") -> Platform:
    """Single RISC-V OoO core with a typical desktop memory hierarchy."""
    return Platform(
        name=name,
        isa=RiscV(),
        cores=[SmallOoOCore("rv-core-0")],
        memory=TypicalDesktop(),
    )


def QuadCoreX86(name: str = "quad-x86") -> Platform:
    """Four-core x86-64 platform with server-class memory."""
    return Platform(
        name=name,
        isa=X86(),
        cores=[BigOoOCore(f"x86-core-{i}") for i in range(4)],
        memory=ServerMemory(),
    )


def DualCoreArm(name: str = "dual-arm") -> Platform:
    """Two-core AArch64 platform."""
    return Platform(
        name=name,
        isa=Arm(),
        cores=[SmallOoOCore(f"arm-core-{i}") for i in range(2)],
        memory=TypicalDesktop(),
    )
