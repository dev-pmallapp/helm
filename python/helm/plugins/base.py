"""Base class for all HELM plugins."""

from __future__ import annotations
from typing import Any, Dict, Optional


class PluginBase:
    """Base class for HELM plugins.

    Subclass this to create custom analysis plugins in Python.

    Parameters
    ----------
    name : str
        Plugin name (used for lookup via ``sim.plugin(name)``).
    args : dict
        Plugin-specific configuration.
    """

    def __init__(self, name: str, **args: Any) -> None:
        self.name = name
        self.args = args
        self.results: Dict[str, Any] = {}

    def on_insn(self, vcpu: int, vaddr: int, mnemonic: str) -> None:
        """Called on every instruction execution (if enabled)."""

    def on_mem(self, vcpu: int, vaddr: int, size: int, is_store: bool) -> None:
        """Called on every memory access (if enabled)."""

    def on_syscall(self, vcpu: int, number: int, args: list) -> None:
        """Called on syscall entry."""

    def on_syscall_ret(self, vcpu: int, number: int, ret: int) -> None:
        """Called on syscall return."""

    def on_tb(self, vcpu: int, pc: int, insn_count: int) -> None:
        """Called on every translated-block execution."""

    def atexit(self) -> None:
        """Called at simulation end.  Populate self.results here."""

    @property
    def enabled_callbacks(self) -> set:
        """Which callbacks this plugin needs.  Override to opt in.

        Returns a set of strings: ``"insn"``, ``"mem"``, ``"syscall"``,
        ``"syscall_ret"``, ``"tb"``.
        """
        return set()

    def to_dict(self) -> dict:
        return {"name": self.name, "args": self.args}

    def __repr__(self) -> str:
        if self.args:
            a = ", ".join(f"{k}={v!r}" for k, v in self.args.items())
            return f"{self.__class__.__name__}({a})"
        return f"{self.__class__.__name__}()"
