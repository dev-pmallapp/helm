"""
helmutil -- shared utilities for helm-ng example scripts.

Eliminates boilerplate that was duplicated across every example:
launcher guard, project root discovery, native extension import,
resource-based asset defaults, and JIT comparison helpers.

Usage from any example script::

    import helmutil
    _helm_ng = helmutil.import_helm_ng()

    # Asset defaults
    kernel = helmutil.default_kernel()
    binary = helmutil.default_binary()

    # JIT comparison helpers
    diffs = helmutil.reg_diff(
        helmutil.snapshot_regs(sim_a),
        helmutil.snapshot_regs(sim_b),
    )
"""

from __future__ import annotations

import importlib.util
import os
import sys
import types
from pathlib import Path
from typing import Optional

__all__ = [
    "require_launcher",
    "project_root",
    "import_helm_ng",
    "default_kernel",
    "default_initrd",
    "default_binary",
    "default_l4re_image",
    "snapshot_regs",
    "reg_diff",
    "run_chunks",
    "load_boot_module",
    "obtain_resource",
    "resolve_assets_dir",
    "default_boot_asset",
]


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


# ── Project root ─────────────────────────────────────────────────────────────

def _find_root() -> Path:
    """Walk up from this file to find the workspace root (has Cargo.toml)."""
    p = Path(__file__).resolve()
    for ancestor in p.parents:
        if (ancestor / "Cargo.toml").exists():
            return ancestor
    return p.parent  # fallback: examples/ directory


ROOT = _find_root()


def project_root() -> Path:
    """Return the helm-ng workspace root."""
    return ROOT


# ── Ensure python/ is importable ─────────────────────────────────────────────

_python_dir = str(ROOT / "python")
if _python_dir not in sys.path:
    sys.path.insert(0, _python_dir)

from helm.resources import obtain_resource  # noqa: E402


# ── Native extension import ──────────────────────────────────────────────────

def _preferred_build() -> Optional[str]:
    """If the running interpreter lives under target/{debug,release},
    prefer that build variant's .so."""
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

    Searches the preferred build directory first, then release, then debug.
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

    # Last resort
    import _helm_ng
    return _helm_ng


# ── Asset defaults (via obtain_resource) ─────────────────────────────────────

def default_kernel() -> Optional[str]:
    """Return the path to the default Linux kernel image, or None."""
    try:
        return obtain_resource("linux-rpi-kernel", download=False).path("vmlinuz-rpi")
    except (FileNotFoundError, Exception):
        pass
    try:
        return obtain_resource("linux-lts-kernel", download=False).path("vmlinuz-lts")
    except (FileNotFoundError, Exception):
        return None


def default_initrd() -> Optional[str]:
    """Return the path to the default initramfs, or None."""
    try:
        return obtain_resource("linux-rpi-kernel", download=False).path("initramfs-rpi")
    except (FileNotFoundError, Exception):
        pass
    try:
        return obtain_resource("linux-lts-kernel", download=False).path("initramfs-lts")
    except (FileNotFoundError, Exception):
        return None


def default_binary() -> str:
    """Return the path to the default SE-mode binary (fish shell)."""
    try:
        return obtain_resource("fish-shell", download=False).path("fish")
    except (FileNotFoundError, Exception):
        return str(ROOT / "assets" / "aarch64" / "binaries" / "fish")


def default_l4re_image(resource_id: str = "l4re-hello") -> str:
    """Return the path to an L4Re pre-built ELF image."""
    try:
        return obtain_resource(resource_id, download=False).path()
    except (FileNotFoundError, Exception):
        return str(ROOT / "assets" / "aarch64" / "boot" / "l4re" /
                   f"{resource_id.replace('-', '_')}_arm_virt.elf")


def resolve_assets_dir() -> Path:
    """Return the directory containing kernel/initramfs boot assets."""
    try:
        res = obtain_resource("linux-rpi-kernel", download=False)
        return Path(res.path("vmlinuz-rpi")).parent
    except (FileNotFoundError, Exception):
        pass
    try:
        res = obtain_resource("linux-lts-kernel", download=False)
        return Path(res.path("vmlinuz-lts")).parent
    except (FileNotFoundError, Exception):
        pass
    for p in [
        ROOT / "assets" / "aarch64" / "boot" / "linux",
        ROOT / "assets" / "aarch64" / "boot",
    ]:
        if p.is_dir():
            return p
    return ROOT / "assets" / "aarch64" / "boot" / "linux"


def default_boot_asset(*candidates: str) -> str:
    """Resolve a boot asset filename from the assets directory."""
    assets = resolve_assets_dir()
    for c in candidates:
        p = assets / c
        if p.is_file():
            return str(p)
    return str(assets / candidates[0])


# ── JIT comparison helpers ───────────────────────────────────────────────────

COMPARE_REGS = ["pc", "sp", "nzcv"] + [f"x{i}" for i in range(31)]


def snapshot_regs(sim) -> dict:
    """Capture current register values from a simulation instance."""
    return {r: getattr(sim, r, None) for r in COMPARE_REGS
            if getattr(sim, r, None) is not None}


def reg_diff(a: dict, b: dict) -> list[str]:
    """Return human-readable list of register differences."""
    diffs = []
    for k in sorted(set(a) | set(b)):
        va, vb = a.get(k), b.get(k)
        if va != vb:
            if isinstance(va, int) and isinstance(vb, int):
                diffs.append(f"  {k}: a={va:#018x}  b={vb:#018x}")
            else:
                diffs.append(f"  {k}: a={va}  b={vb}")
    return diffs


# ── Run loop helper ──────────────────────────────────────────────────────────

def run_chunks(sim, max_insns: int, chunk: int = 10_000_000,
               jit: bool = False) -> str:
    """Run simulation in chunks, return final stop reason."""
    run_fn = sim.run_jit if jit else sim.run
    remaining = max_insns
    stop = "quantum"
    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop = run_fn(n)
        remaining -= n
        if stop != "quantum":
            break
    return stop


# ── boot_rpi_full loader ─────────────────────────────────────────────────────

_boot_module = None


def load_boot_module():
    """Load examples/fs/boot_rpi_full.py as a module (cached).

    This is the same pattern the existing debug scripts use to access
    _resolve_dtb_path and other FS boot helpers.
    """
    global _boot_module
    if _boot_module is not None:
        return _boot_module
    p = ROOT / "examples" / "fs" / "boot_rpi_full.py"
    m = types.ModuleType("boot_rpi_full")
    m.__file__ = str(p)
    exec(compile(p.read_text(), str(p), "exec"), m.__dict__)
    _boot_module = m
    return m


# ── Address constants (arm-virt) ─────────────────────────────────────────────

RAM_BASE  = 0x4000_0000
UART_BASE = 0x0900_0000
GICD_BASE = 0x0800_0000
GICC_BASE = 0x0801_0000
GICR_BASE = 0x080A_0000

DEFAULT_APPEND = (
    f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 "
    "loglevel=8 printk.prefer_direct=1"
)
