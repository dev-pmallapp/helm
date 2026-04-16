"""
helmutil -- convenience shim for helm-ng example scripts.

Re-exports the full ``helm.*`` API surface so example scripts can do::

    import helmutil
    _helm_ng = helmutil.import_helm_ng()
    kernel = helmutil.default_kernel()
    result = helmutil.compare_lockstep(sim_i, sim_j)

All functionality lives in ``python/helm/`` modules; this file just
ensures ``python/`` is on ``sys.path`` and re-exports everything.
"""

from __future__ import annotations

import os
import sys
import types
from pathlib import Path

# Ensure python/ is importable
_python_dir = str(Path(__file__).resolve().parent.parent / "python")
if _python_dir not in sys.path:
    sys.path.insert(0, _python_dir)

# ── Re-export from helm.util ────────────────────────────────────────────────
from helm.util import (                        # noqa: E402
    ROOT,
    project_root,
    require_launcher,
    import_helm_ng,
    to_bytes,
    RAM_BASE,
    UART_BASE,
    GICD_BASE,
    GICC_BASE,
    GICR_BASE,
    DEFAULT_APPEND,
)

# ── Re-export from helm.resources ───────────────────────────────────────────
from helm.resources import (                   # noqa: E402
    obtain_resource,
    resource_path,
    list_resources,
)

# ── Re-export from helm.simulate ────────────────────────────────────────────
from helm.simulate import simulate, SimResult  # noqa: E402

# ── Re-export from helm.stats ───────────────────────────────────────────────
from helm.stats import (                       # noqa: E402
    dump_stats,
    format_stats,
    format_stats_json,
)

# ── Re-export from helm.debug ───────────────────────────────────────────────
from helm.debug import (                       # noqa: E402
    snapshot_regs,
    reg_diff,
    compare_lockstep,
    CompareResult,
    COMPARE_REGS,
)


# ── Asset defaults (convenience wrappers) ───────────────────────────────────

def default_kernel():
    """Return path to default Linux kernel, or None."""
    try:
        return obtain_resource("linux-rpi-kernel", download=False).path("vmlinuz-rpi")
    except (FileNotFoundError, Exception):
        pass
    try:
        return obtain_resource("linux-lts-kernel", download=False).path("vmlinuz-lts")
    except (FileNotFoundError, Exception):
        return None


def default_initrd():
    """Return path to default initramfs, or None."""
    try:
        return obtain_resource("linux-rpi-kernel", download=False).path("initramfs-rpi")
    except (FileNotFoundError, Exception):
        pass
    try:
        return obtain_resource("linux-lts-kernel", download=False).path("initramfs-lts")
    except (FileNotFoundError, Exception):
        return None


def default_binary():
    """Return path to default SE binary (fish shell)."""
    try:
        return obtain_resource("fish-shell", download=False).path("fish")
    except (FileNotFoundError, Exception):
        return str(ROOT / "assets" / "aarch64" / "binaries" / "fish")


def default_l4re_image(resource_id="l4re-hello"):
    """Return path to an L4Re pre-built ELF image."""
    try:
        return obtain_resource(resource_id, download=False).path()
    except (FileNotFoundError, Exception):
        return str(ROOT / "assets" / "aarch64" / "boot" / "l4re" /
                   f"{resource_id.replace('-', '_')}_arm_virt.elf")


def resolve_assets_dir():
    """Return the boot assets directory."""
    try:
        res = obtain_resource("linux-rpi-kernel", download=False)
        return Path(res.path("vmlinuz-rpi")).parent
    except (FileNotFoundError, Exception):
        pass
    for p in [ROOT / "assets" / "aarch64" / "boot" / "linux",
              ROOT / "assets" / "aarch64" / "boot"]:
        if p.is_dir():
            return p
    return ROOT / "assets" / "aarch64" / "boot" / "linux"


def default_boot_asset(*candidates):
    """Resolve a boot asset filename."""
    assets = resolve_assets_dir()
    for c in candidates:
        p = assets / c
        if p.is_file():
            return str(p)
    return str(assets / candidates[0])


# ── boot_rpi_full loader ────────────────────────────────────────────────────

_boot_module = None


def load_boot_module():
    """Load examples/fs/boot_rpi_full.py as a module (cached)."""
    global _boot_module
    if _boot_module is not None:
        return _boot_module
    p = ROOT / "examples" / "fs" / "boot_rpi_full.py"
    m = types.ModuleType("boot_rpi_full")
    m.__file__ = str(p)
    exec(compile(p.read_text(), str(p), "exec"), m.__dict__)
    _boot_module = m
    return m


# ── Run loop (delegates to helm.simulate) ────────────────────────────────────

def run_chunks(sim, max_insns, chunk=10_000_000, jit=False):
    """Run simulation in chunks, return stop reason. Use simulate() for richer control."""
    result = simulate(sim, max_insns=max_insns, chunk=chunk, jit=jit, progress=False)
    return result.stop_reason
