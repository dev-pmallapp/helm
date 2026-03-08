"""Suspendable SE-mode simulation session.

Provides the pause → inspect → hot-load plugin → continue workflow
without requiring the Rust native bindings.  When the native engine
is available it delegates to :class:`helm_engine::SeSession`; otherwise
it runs a pure-Python stub that exercises the same API.

Examples
--------
::

    from helm.session import SeSession
    from helm.plugins import FaultDetect

    s = SeSession("./binary", ["binary"])

    # Phase 1: warm-up (no plugins, full speed)
    s.run(1_000_000)

    # Phase 2: hot-load debug plugin
    s.add_plugin(FaultDetect())

    # Phase 3: run to a specific PC
    reason = s.run_until_pc(0x411120)
    print(f"stopped: {reason}, PC={s.pc:#x}, insns={s.insn_count}")

    # Phase 4: continue with plugin active
    s.run(10_000_000)
    s.finish()
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum, auto
from typing import List, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from helm.plugins.base import PluginBase


class StopReason(Enum):
    """Why a ``run*`` call returned."""

    INSN_LIMIT = auto()
    BREAKPOINT = auto()
    EXITED = auto()
    ERROR = auto()


@dataclass
class StopResult:
    """Details of why execution stopped."""

    reason: StopReason
    pc: int = 0
    exit_code: int = 0
    message: str = ""

    def __repr__(self) -> str:
        if self.reason == StopReason.EXITED:
            return f"StopResult(EXITED, code={self.exit_code})"
        if self.reason == StopReason.BREAKPOINT:
            return f"StopResult(BREAKPOINT, pc={self.pc:#x})"
        if self.reason == StopReason.ERROR:
            return f"StopResult(ERROR, {self.message!r})"
        return f"StopResult(INSN_LIMIT)"


class SeSession:
    """Suspendable syscall-emulation session.

    Parameters
    ----------
    binary : str
        Path to the AArch64 ELF binary.
    argv : list[str]
        Argument vector (``argv[0]`` is typically the binary name).
    envp : list[str], optional
        Environment variables.
    """

    def __init__(
        self,
        binary: str,
        argv: Optional[List[str]] = None,
        envp: Optional[List[str]] = None,
    ) -> None:
        import os

        self.binary = binary
        self.argv = argv or [os.path.basename(binary)]
        self.envp = envp or [
            "HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin",
            "LANG=C", "USER=helm",
        ]
        self._plugins: List[PluginBase] = []
        self._insn_count: int = 0
        self._virtual_cycles: int = 0
        self._pc: int = 0
        self._exited: bool = False
        self._exit_code: int = 0
        self._native = None

        try:
            self._init_native()
        except ImportError:
            pass

    def _init_native(self) -> None:
        """Try to initialise a Rust-backed session."""
        from helm._helm_core import SeSession as _NativeSeSession
        self._native = _NativeSeSession(self.binary, self.argv, self.envp)

    # -- Plugin management -----------------------------------------------

    def add_plugin(self, plugin: "PluginBase") -> None:
        """Hot-load a plugin.  Takes effect on the next ``run*`` call.

        Can be called before the first run (equivalent to always-on)
        or between runs (the pause/load/continue pattern).
        """
        self._plugins.append(plugin)

    @property
    def plugins(self) -> "List[PluginBase]":
        return list(self._plugins)

    # -- Execution -------------------------------------------------------

    def run(self, max_insns: int = 10_000_000) -> StopResult:
        """Execute up to *max_insns* instructions, then return."""
        if self._native is not None:
            result = self._native.run(max_insns)
            self._pc = self._native.pc
            self._insn_count = self._native.insn_count
            self._virtual_cycles = self._native.virtual_cycles
            self._exited = self._native.has_exited
            if self._exited:
                self._exit_code = self._native.exit_code
                return StopResult(StopReason.EXITED, exit_code=self._exit_code)
            return StopResult(StopReason.INSN_LIMIT, pc=self._pc)
        return self._run_stub(max_insns, pc_break=None)

    def run_until_insns(self, total: int) -> StopResult:
        """Run until *total* instructions have executed since session start."""
        remaining = total - self._insn_count
        if remaining <= 0:
            return StopResult(StopReason.INSN_LIMIT)
        return self._run_stub(remaining, pc_break=None)

    def run_until_pc(
        self,
        target_pc: int,
        max_insns: int = 100_000_000,
    ) -> StopResult:
        """Run until PC equals *target_pc* (with a safety limit)."""
        return self._run_stub(max_insns, pc_break=target_pc)

    # -- Inspection ------------------------------------------------------

    @property
    def pc(self) -> int:
        return self._pc

    @property
    def insn_count(self) -> int:
        return self._insn_count

    @property
    def virtual_cycles(self) -> int:
        return self._virtual_cycles

    @property
    def has_exited(self) -> bool:
        return self._exited

    @property
    def exit_code(self) -> int:
        return self._exit_code

    # -- Teardown --------------------------------------------------------

    def finish(self) -> None:
        """Call ``atexit`` on all loaded plugins."""
        for p in self._plugins:
            p.atexit()

    # -- Stub implementation ---------------------------------------------

    def _run_stub(
        self,
        budget: int,
        pc_break: Optional[int],
    ) -> StopResult:
        """Pure-Python stub — simulates the API without executing code.

        When the native engine is available this path is replaced by
        delegation to the Rust ``SeSession``.
        """
        self._insn_count += budget
        self._virtual_cycles += budget
        return StopResult(StopReason.INSN_LIMIT)


class FsSession:
    """Suspendable full-system simulation session.

    Parameters
    ----------
    kernel : str
        Path to the kernel image.
    machine : str
        Machine type (default: "virt").
    append : str
        Kernel command line.
    sysmap : str, optional
        Path to System.map for symbol resolution.

    Examples
    --------
    ::

        from helm.session import FsSession

        s = FsSession("vmlinuz-rpi", machine="virt",
                       append="earlycon=pl011,0x09000000 console=ttyAMA0")
        s.run(100_000_000)
        print(f"PC={s.pc:#x}, insns={s.insn_count}")
        s.run_until_symbol("start_kernel")
    """

    def __init__(
        self,
        kernel: str,
        machine: str = "virt",
        append: str = "",
        sysmap: Optional[str] = None,
    ) -> None:
        self.kernel = kernel
        self.machine = machine
        self.append = append
        self._native = None

        try:
            from helm._helm_core import FsSession as _NativeFsSession
            self._native = _NativeFsSession(kernel, machine, append, sysmap)
        except ImportError:
            pass

    def run(self, max_insns: int = 100_000_000) -> StopResult:
        """Execute up to *max_insns* instructions, then return."""
        if self._native is not None:
            result = self._native.run(max_insns)
            return StopResult(StopReason.INSN_LIMIT, pc=self.pc)
        return StopResult(StopReason.INSN_LIMIT)

    def run_until_symbol(self, sym: str, max_insns: int = 500_000_000) -> StopResult:
        """Run until a named symbol is reached."""
        if self._native is not None:
            result = self._native.run_until_symbol(sym, max_insns)
            return StopResult(StopReason.BREAKPOINT, pc=self.pc)
        return StopResult(StopReason.INSN_LIMIT)

    def run_until_pc(self, target: int, max_insns: int = 500_000_000) -> StopResult:
        """Run until PC equals target."""
        if self._native is not None:
            result = self._native.run_until_pc(target, max_insns)
            return StopResult(StopReason.BREAKPOINT, pc=self.pc)
        return StopResult(StopReason.INSN_LIMIT)

    @property
    def pc(self) -> int:
        if self._native is not None:
            return self._native.pc
        return 0

    @property
    def insn_count(self) -> int:
        if self._native is not None:
            return self._native.insn_count
        return 0

    def xn(self, n: int) -> int:
        """Read general-purpose register Xn."""
        if self._native is not None:
            return self._native.xn(n)
        return 0

    def regs(self) -> dict:
        """Return all registers as a dict."""
        if self._native is not None:
            return self._native.regs()
        return {}
