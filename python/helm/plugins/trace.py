"""Trace and profiling plugins."""

from __future__ import annotations
from typing import Dict, List, Optional
from helm.plugins.base import PluginBase


class InsnCount(PluginBase):
    """Count instructions per vCPU.  Minimal overhead."""

    def __init__(self) -> None:
        super().__init__("insn-count")
        self._counts: Dict[int, int] = {}

    @property
    def enabled_callbacks(self) -> set:
        return {"insn"}

    def on_insn(self, vcpu: int, vaddr: int, mnemonic: str) -> None:
        self._counts[vcpu] = self._counts.get(vcpu, 0) + 1

    @property
    def total(self) -> int:
        return sum(self._counts.values())

    @property
    def per_vcpu(self) -> Dict[int, int]:
        return dict(self._counts)

    def atexit(self) -> None:
        self.results = {"total": self.total, "per_vcpu": self.per_vcpu}


class ExecLog(PluginBase):
    """Log every executed instruction.

    Parameters
    ----------
    output : str, optional
        File path to write the trace.  Defaults to stdout.
    max_lines : int
        Maximum number of lines to record.
    regs : bool
        Include register state (future).
    """

    def __init__(
        self,
        output: Optional[str] = None,
        max_lines: int = 1_000_000,
        regs: bool = False,
    ) -> None:
        super().__init__("execlog", output=output, max_lines=max_lines, regs=regs)
        self.lines: List[str] = []
        self._max = max_lines

    @property
    def enabled_callbacks(self) -> set:
        return {"insn"}

    def on_insn(self, vcpu: int, vaddr: int, mnemonic: str) -> None:
        if len(self.lines) < self._max:
            self.lines.append(f"{vcpu}  0x{vaddr:016x}  {mnemonic}")

    def atexit(self) -> None:
        output = self.args.get("output")
        if output:
            with open(output, "w") as f:
                f.write("\n".join(self.lines))
        self.results = {"lines": len(self.lines)}


class HotBlocks(PluginBase):
    """Rank basic blocks by execution count."""

    def __init__(self, top_n: int = 20) -> None:
        super().__init__("hotblocks", top_n=top_n)
        self._counts: Dict[int, int] = {}

    @property
    def enabled_callbacks(self) -> set:
        return {"tb"}

    def on_tb(self, vcpu: int, pc: int, insn_count: int) -> None:
        self._counts[pc] = self._counts.get(pc, 0) + 1

    def top(self, n: int = 20) -> List[tuple]:
        return sorted(self._counts.items(), key=lambda x: -x[1])[:n]

    def atexit(self) -> None:
        self.results = {"top": self.top(self.args.get("top_n", 20))}


class HowVec(PluginBase):
    """Instruction-class histogram."""

    CATEGORIES = {
        "IntAlu": ["ADD", "SUB", "MOV", "AND", "ORR", "EOR", "CMP", "CMN", "TST",
                    "MVN", "NEG", "BIC", "ORN", "UBFM", "SBFM", "BFM"],
        "IntMul": ["MUL", "MADD", "MSUB", "SMULL", "UMULL", "SMULH", "UMULH"],
        "IntDiv": ["UDIV", "SDIV"],
        "Branch": ["B", "BL", "BR", "BLR", "RET", "CBZ", "CBNZ", "TBZ", "TBNZ"],
        "Load": ["LDR", "LDP", "LDRB", "LDRH", "LDRSW", "LDRSB", "LDRSH", "LDXR", "LDAXR"],
        "Store": ["STR", "STP", "STRB", "STRH", "STXR", "STLXR"],
        "FpAlu": ["FADD", "FSUB", "FMUL", "FDIV", "FMOV", "FCVT", "FABS", "FNEG"],
        "Atomic": ["SWP", "LDADD", "CAS", "LDCLR", "LDSET", "LDUMAX", "LDSMAX"],
        "Syscall": ["SVC"],
    }

    def __init__(self) -> None:
        super().__init__("howvec")
        self._counts: Dict[str, int] = {}

    @property
    def enabled_callbacks(self) -> set:
        return {"insn"}

    def on_insn(self, vcpu: int, vaddr: int, mnemonic: str) -> None:
        cat = self._classify(mnemonic)
        self._counts[cat] = self._counts.get(cat, 0) + 1

    def _classify(self, mnemonic: str) -> str:
        m = mnemonic.upper().split("_")[0]
        for cat, prefixes in self.CATEGORIES.items():
            if any(m.startswith(p) for p in prefixes):
                return cat
        return "Other"

    def atexit(self) -> None:
        self.results = dict(self._counts)


class SyscallTrace(PluginBase):
    """Log syscall entry and return."""

    def __init__(self) -> None:
        super().__init__("syscall-trace")
        self.entries: List[Dict] = []

    @property
    def enabled_callbacks(self) -> set:
        return {"syscall", "syscall_ret"}

    def on_syscall(self, vcpu: int, number: int, args: list) -> None:
        self.entries.append({
            "type": "entry", "vcpu": vcpu, "nr": number, "args": args,
        })

    def on_syscall_ret(self, vcpu: int, number: int, ret: int) -> None:
        self.entries.append({
            "type": "return", "vcpu": vcpu, "nr": number, "ret": ret,
        })

    def atexit(self) -> None:
        self.results = {"count": len(self.entries)}
