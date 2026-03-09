"""
TimingModel — select the simulation accuracy level.

HELM provides three accuracy levels:

- **FE** (Functional Emulation): IPC=1, maximum speed, no timing.
  Like QEMU — run binaries fast with no microarchitectural detail.
- **ITE** (Interval-Timing Emulation): Cache latencies, device stalls,
  optional simplified pipeline.  Based on the *interval simulation*
  methodology (Genbrugge et al., HPCA 2010).  Comparable to Sniper
  and Simics timing modes.
- **CAE** (Cycle-Accurate Emulation): Full pipeline stages,
  dependencies, speculation.  Like gem5 O3CPU.

The execution mode is orthogonal:

- **SE** (Syscall Emulation): Run user-mode binaries with Linux
  syscalls emulated (like qemu-user or gem5 SE mode).
- **FS** (Full System): Boot a kernel image with devices, MMU, and
  interrupts.
- **HAE** (Hardware-Assisted Emulation): Near-native execution via KVM.
"""

from __future__ import annotations


class TimingModel:
    """Describes the simulation accuracy level.

    Use the class methods::

        mode = TimingModel.fe()    # L0: functional, fastest
        mode = TimingModel.ite()   # L1-L2: interval-timing
        mode = TimingModel.cae()   # L3: cycle-accurate
    """

    def __init__(self, level: str, **params: int) -> None:
        self.level = level
        self.params = params

    # -- Tier factories --------------------------------------------------

    @classmethod
    def fe(cls) -> "TimingModel":
        """FE: Functional Emulation.  IPC=1, no memory modelling.  100-1000 MIPS."""
        return cls("FE")

    @classmethod
    def ite(
        cls,
        *,
        int_alu_latency: int = 1,
        int_mul_latency: int = 3,
        int_div_latency: int = 12,
        fp_alu_latency: int = 4,
        fp_mul_latency: int = 5,
        fp_div_latency: int = 15,
        load_latency: int = 4,
        store_latency: int = 1,
        branch_penalty: int = 10,
        l1_latency: int = 3,
        l2_latency: int = 12,
        l3_latency: int = 40,
        dram_latency: int = 200,
    ) -> "TimingModel":
        """ITE: Interval-Timing Emulation.  Cache latencies, device delays.  1-100 MIPS."""
        return cls(
            "ITE",
            int_alu_latency=int_alu_latency,
            int_mul_latency=int_mul_latency,
            int_div_latency=int_div_latency,
            fp_alu_latency=fp_alu_latency,
            fp_mul_latency=fp_mul_latency,
            fp_div_latency=fp_div_latency,
            load_latency=load_latency,
            store_latency=store_latency,
            branch_penalty=branch_penalty,
            l1_latency=l1_latency,
            l2_latency=l2_latency,
            l3_latency=l3_latency,
            dram_latency=dram_latency,
        )

    # Backward compat alias
    ape = ite

    @classmethod
    def cae(cls) -> "TimingModel":
        """CAE: Cycle-Accurate Emulation.  Full pipeline detail.  0.1-1 MIPS."""
        return cls("CAE")

    # -- Serialisation ---------------------------------------------------

    def to_dict(self) -> dict:
        if self.params:
            return {"level": self.level, **self.params}
        return {"level": self.level}

    def __repr__(self) -> str:
        if self.params:
            p = ", ".join(f"{k}={v}" for k, v in self.params.items())
            return f"TimingModel.{self.level.lower()}({p})"
        return f"TimingModel.{self.level.lower()}()"
