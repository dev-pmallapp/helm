"""Debug and fault-detection plugins."""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from helm.plugins.base import PluginBase


class FaultDetect(PluginBase):
    """Detect execution anomalies and produce diagnostic reports.

    By default the plugin is **active from the start**.  To defer
    activation (avoiding per-instruction callback overhead during
    warm-up), use one of the trigger parameters:

    Parameters
    ----------
    after_insns : int, optional
        Activate after this many instructions have executed.
    at_symbol : str, optional
        Activate when PC enters the named symbol (requires debug info).
    at_pc : int, optional
        Activate when PC reaches this address.
    ring : int
        PC history ring-buffer size (default 64).
    syscall_log : int
        Number of recent syscalls to keep (default 32).
    max_reports : int
        Cap on fault reports collected (default 8).

    Examples
    --------
    Always active::

        sim.add_plugin(FaultDetect())

    Activate after 1 M instructions (skip warm-up)::

        sim.add_plugin(FaultDetect(after_insns=1_000_000))

    Activate when execution reaches ``main``::

        sim.add_plugin(FaultDetect(at_symbol="main"))

    Activate at a specific PC::

        sim.add_plugin(FaultDetect(at_pc=0x411120))

    From the CLI / Python config JSON::

        # always on
        --plugin fault-detect

        # deferred
        --plugin fault-detect:after_insns=1000000
        --plugin fault-detect:at_pc=0x411120
    """

    def __init__(
        self,
        *,
        after_insns: Optional[int] = None,
        at_symbol: Optional[str] = None,
        at_pc: Optional[int] = None,
        ring: int = 64,
        syscall_log: int = 32,
        max_reports: int = 8,
    ) -> None:
        kwargs: Dict[str, Any] = {
            "ring": ring,
            "syscall_log": syscall_log,
            "max_reports": max_reports,
        }
        if after_insns is not None:
            kwargs["after_insns"] = after_insns
        if at_symbol is not None:
            kwargs["at_symbol"] = at_symbol
        if at_pc is not None:
            kwargs["at_pc"] = at_pc

        super().__init__("fault-detect", **kwargs)
        self._active = (
            after_insns is None and at_symbol is None and at_pc is None
        )
        self._after_insns = after_insns
        self._at_symbol = at_symbol
        self._at_pc = at_pc
        self._insn_count = 0
        self.reports: List[Dict[str, Any]] = []

    @property
    def enabled_callbacks(self) -> set:
        return {"insn", "syscall", "syscall_ret", "fault"}

    @property
    def active(self) -> bool:
        """Whether the plugin is currently collecting data."""
        return self._active

    def activate(self) -> None:
        """Manually force the plugin into active state."""
        self._active = True

    def on_insn(self, vcpu: int, vaddr: int, mnemonic: str) -> None:
        self._insn_count += 1
        if not self._active:
            if self._after_insns is not None and self._insn_count >= self._after_insns:
                self._active = True
            elif self._at_pc is not None and vaddr == self._at_pc:
                self._active = True
            else:
                return

    def on_fault(self, info: Dict[str, Any]) -> None:
        """Called by the engine when a fault is detected."""
        if self._active:
            self.reports.append(info)

    def atexit(self) -> None:
        self.results = {
            "reports": self.reports,
            "total_insns_seen": self._insn_count,
            "was_active": self._active,
        }
        if self.reports:
            for i, r in enumerate(self.reports, 1):
                print(f"[fault-detect] Report {i}/{len(self.reports)}: "
                      f"{r.get('kind', '?')} at PC={r.get('pc', 0):#x}")
