"""
helm.util -- general-purpose helpers for helm-ng scripts.

Equivalent to gem5's ``m5.util``: project paths, native extension
loading, unit conversion, and common constants.
"""

from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path
from typing import Optional

__all__ = [
    "project_root",
    "import_helm_ng",
    "require_launcher",
    "to_bytes",
    "RAM_BASE",
    "UART_BASE",
    "GICD_BASE",
    "GICC_BASE",
    "GICR_BASE",
    "DEFAULT_APPEND",
]


# ── Project root ─────────────────────────────────────────────────────────────

def _find_root() -> Path:
    """Walk up from this file to find the workspace root (has Cargo.toml)."""
    p = Path(__file__).resolve()
    for ancestor in p.parents:
        if (ancestor / "Cargo.toml").exists():
            return ancestor
    return p.parents[2]  # fallback: python/helm/util.py -> root


ROOT = _find_root()


def project_root() -> Path:
    """Return the helm-ng workspace root."""
    return ROOT


# ── Launcher guard ───────────────────────────────────────────────────────────

def require_launcher() -> None:
    """Abort if not running inside helm-aarch64 / helm-system-aarch64."""
    if getattr(sys, "_helm_launcher", None) not in {
        "helm-aarch64",
        "helm-system-aarch64",
    }:
        raise SystemExit(
            "This script must be run via helm-aarch64 or "
            "helm-system-aarch64, not directly via python."
        )


# ── Native extension import ──────────────────────────────────────────────────

def _preferred_build() -> Optional[str]:
    """Detect which build variant the running binary came from."""
    try:
        exe = Path(sys.executable).resolve()
        rel = exe.relative_to(ROOT)
        parts = rel.parts
        if len(parts) >= 2 and parts[0] == "target" and parts[1] in ("debug", "release"):
            return parts[1]
    except (OSError, ValueError):
        pass
    return None


def import_helm_ng():
    """Import and return the ``_helm_ng`` native extension module.

    Prefers the build variant matching the running binary, then
    release, then debug.
    """
    try:
        import _helm_ng
        return _helm_ng
    except ImportError:
        pass

    preferred = _preferred_build()
    order = []
    if preferred:
        order.append(preferred)
    for b in ("release", "debug"):
        if b not in order:
            order.append(b)

    for build in order:
        so = ROOT / "target" / build / "lib_helm_ng.so"
        if so.is_file():
            spec = importlib.util.spec_from_file_location("_helm_ng", so)
            if spec and spec.loader:
                mod = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(mod)
                return mod

    import _helm_ng
    return _helm_ng


# ── Unit conversion ──────────────────────────────────────────────────────────

_SIZE_SUFFIXES = {
    "": 1,
    "b": 1,
    "k": 1 << 10, "kb": 1 << 10, "kib": 1 << 10,
    "m": 1 << 20, "mb": 1 << 20, "mib": 1 << 20,
    "g": 1 << 30, "gb": 1 << 30, "gib": 1 << 30,
    "t": 1 << 40, "tb": 1 << 40, "tib": 1 << 40,
}


def to_bytes(value) -> int:
    """Convert a size string like ``"512MiB"`` to an integer byte count.

    Accepts plain ints, int-like strings, and strings with K/M/G/T
    suffixes (case-insensitive, with or without ``iB``/``B``).
    """
    if isinstance(value, int):
        return value
    s = str(value).strip()
    for suffix, mult in sorted(_SIZE_SUFFIXES.items(), key=lambda x: -len(x[0])):
        if s.lower().endswith(suffix) and len(suffix) < len(s):
            num_part = s[: len(s) - len(suffix)].strip()
            try:
                return int(num_part) * mult
            except ValueError:
                continue
    return int(s)


# ── ARM virt address constants ───────────────────────────────────────────────

RAM_BASE  = 0x4000_0000
UART_BASE = 0x0900_0000
GICD_BASE = 0x0800_0000
GICC_BASE = 0x0801_0000
GICR_BASE = 0x080A_0000

DEFAULT_APPEND = (
    f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 "
    "loglevel=8 printk.prefer_direct=1"
)
