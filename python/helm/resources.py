"""
helm.resources -- gem5-style resource management.

Resources are simulation assets (kernels, binaries, firmware images) that
the simulator needs at runtime.  Each resource has a unique string ID and
a known local path under the project ``assets/`` tree.

The canonical way to get a resource is::

    from helm.resources import obtain_resource, Resource

    kernel = obtain_resource("linux-rpi-kernel")
    sim.load_kernel(kernel.path("vmlinuz-rpi"))

    fish = obtain_resource("fish-shell")
    sim.load_elf(fish.path("fish"))

If the asset is missing locally, ``obtain_resource`` will download it from
the upstream URL recorded in ``scripts/resources.json`` and verify its
SHA-256 checksum before returning.

Design follows gem5's ``obtain_resource()`` contract:
- Caller asks for a resource by ID
- System returns a local path, downloading if necessary
- Checksums are verified on every download
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import Optional

__all__ = [
    "obtain_resource",
    "resource_path",
    "Resource",
    "list_resources",
    "HelmResourceError",
]


# ── Errors ───────────────────────────────────────────────────────────────────

class HelmResourceError(Exception):
    """Raised when a resource cannot be obtained or verified."""


# ── Path resolution ──────────────────────────────────────────────────────────

def _project_root() -> Path:
    """Walk up from this file to find the project root (contains Cargo.toml)."""
    p = Path(__file__).resolve()
    for ancestor in p.parents:
        if (ancestor / "Cargo.toml").exists():
            return ancestor
    return p.parents[3]  # fallback: python/helm/resources.py -> root


_ROOT = _project_root()
_MANIFEST_PATH = _ROOT / "scripts" / "resources.json"
_ASSETS_DIR = _ROOT / "assets"


def _load_manifest() -> dict:
    if not _MANIFEST_PATH.exists():
        raise HelmResourceError(f"manifest not found: {_MANIFEST_PATH}")
    with open(_MANIFEST_PATH) as f:
        return json.load(f)


def _sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _fetch(url: str, dest: Path) -> None:
    """Download url to dest using wget or curl."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    if shutil.which("wget"):
        subprocess.run(
            ["wget", "-q", "--show-progress", "-O", str(dest), url],
            check=True,
        )
    elif shutil.which("curl"):
        subprocess.run(
            ["curl", "-fL", "--progress-bar", "-o", str(dest), url],
            check=True,
        )
    else:
        raise HelmResourceError("neither wget nor curl found")


# ── Canonical directory layout ───────────────────────────────────────────────
#
# assets/
#   aarch64/
#     binaries/          SE-mode test workloads (fish, inflate_test, ...)
#     boot/
#       linux/           kernel, initramfs, System.map, config, modloop, dtbs
#       l4re/            L4Re pre-built ELFs, ramdisks, l4image tool
#     fs/                full RPi image (firmware, u-boot, overlays, ...)
#     l4re/              (duplicate of boot/l4re for backward compat)
#   riscv/
#     bin/               RISC-V static binaries (busybox, static-sh)
#

# ── Resource ─────────────────────────────────────────────────────────────────

class Resource:
    """Handle to a downloaded and verified resource.

    Attributes
    ----------
    id : str
        Manifest resource ID.
    description : str
        Human-readable description.
    category : str
        One of: boot, binary, fs, tool.
    architecture : str
        Target architecture (aarch64, riscv64, x86_64).
    base_dir : Path
        Root directory where this resource's files live.
    """

    def __init__(self, entry: dict, assets_dir: Path):
        self.id: str = entry["id"]
        self.description: str = entry.get("description", "")
        self.category: str = entry.get("category", "")
        self.architecture: str = entry.get("architecture", "")
        self._entry = entry
        self._assets_dir = assets_dir
        self._source = entry["source"]

        src_type = self._source["type"]
        if src_type == "file":
            self.base_dir = (assets_dir / self._source["dest"]).parent
        elif src_type in ("archive", "apk"):
            self.base_dir = assets_dir / self._source["extract_to"]
        else:
            self.base_dir = assets_dir

    def path(self, filename: Optional[str] = None) -> str:
        """Return the absolute path to a file within this resource.

        Parameters
        ----------
        filename : str, optional
            Relative filename within the resource directory.  If omitted,
            returns the primary file for single-file resources, or the
            base directory for multi-file resources.

        Returns
        -------
        str
            Absolute filesystem path.

        Raises
        ------
        FileNotFoundError
            If the resolved path does not exist.
        """
        if filename is not None:
            p = self.base_dir / filename
        elif self._source["type"] == "file":
            p = self._assets_dir / self._source["dest"]
        else:
            p = self.base_dir

        if not p.exists():
            raise FileNotFoundError(
                f"resource '{self.id}' file not found: {p}\n"
                f"Run: scripts/manage-assets.sh download {self.id}"
            )
        return str(p)

    @property
    def is_present(self) -> bool:
        """True if the resource's primary file(s) exist locally."""
        try:
            self.path()
            return True
        except FileNotFoundError:
            return False

    def __repr__(self) -> str:
        state = "present" if self.is_present else "missing"
        return f"Resource(id={self.id!r}, state={state})"


# ── Download logic ───────────────────────────────────────────────────────────

def _download_file(entry: dict, assets_dir: Path) -> None:
    src = entry["source"]
    dest = assets_dir / src["dest"]
    expected = src["sha256"]

    if dest.exists() and _sha256_file(dest) == expected:
        return  # already present and verified

    _fetch(src["url"], dest)

    actual = _sha256_file(dest)
    if actual != expected:
        dest.unlink(missing_ok=True)
        raise HelmResourceError(
            f"sha256 mismatch for {entry['id']}: "
            f"expected {expected}, got {actual}"
        )

    # Make ELF files executable
    if dest.suffix in (".elf", "") and os.path.getsize(dest) > 0:
        try:
            with open(dest, "rb") as f:
                if f.read(4) == b"\x7fELF":
                    dest.chmod(dest.stat().st_mode | 0o111)
        except OSError:
            pass

    # Post-install hook
    post = src.get("post_install")
    if post:
        subprocess.run(
            post,
            shell=True,
            check=True,
            env={**os.environ, "ASSETS_DIR": str(assets_dir)},
        )


def _download_archive(entry: dict, assets_dir: Path) -> None:
    src = entry["source"]
    extract_to = assets_dir / src["extract_to"]
    expected = src.get("sha256")
    url = src["url"]

    # Quick presence check: if extract dir has files, skip
    if extract_to.is_dir() and any(extract_to.iterdir()):
        return

    with tempfile.NamedTemporaryFile(delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        _fetch(url, tmp_path)

        if expected:
            actual = _sha256_file(tmp_path)
            if actual != expected:
                raise HelmResourceError(
                    f"sha256 mismatch for archive {entry['id']}: "
                    f"expected {expected}, got {actual}"
                )

        extract_to.mkdir(parents=True, exist_ok=True)

        with tarfile.open(str(tmp_path)) as tf:
            tf.extractall(str(extract_to))

        # Rename extracted files if mapping is provided
        extract_files = src.get("extract_files", {})
        for src_name, dst_name in extract_files.items():
            src_path = extract_to / src_name
            dst_path = extract_to / dst_name
            if src_path.exists():
                dst_path.parent.mkdir(parents=True, exist_ok=True)
                shutil.move(str(src_path), str(dst_path))

        # Keep archive if requested
        if src.get("keep_archive", False):
            archive_name = Path(url).name
            shutil.move(str(tmp_path), str(extract_to / archive_name))
        else:
            tmp_path.unlink(missing_ok=True)
    except Exception:
        tmp_path.unlink(missing_ok=True)
        raise


def _download_apk(entry: dict, assets_dir: Path) -> None:
    src = entry["source"]
    extract_to = assets_dir / src["extract_to"]
    url = src["url"]
    files_spec = src.get("files", {})

    # Check if sentinel file exists
    if files_spec:
        first_dest = next(iter(files_spec.values()))["dest"]
        sentinel = extract_to / first_dest
        if sentinel.exists():
            return

    with tempfile.NamedTemporaryFile(delete=False, suffix=".apk") as tmp:
        tmp_path = Path(tmp.name)

    try:
        _fetch(url, tmp_path)

        tmp_dir = Path(tempfile.mkdtemp())
        with tarfile.open(str(tmp_path)) as tf:
            tf.extractall(str(tmp_dir))

        extract_to.mkdir(parents=True, exist_ok=True)

        for apk_path, spec in files_spec.items():
            src_path = tmp_dir / apk_path
            if not src_path.exists():
                # Try searching by basename
                matches = list(tmp_dir.rglob(Path(apk_path).name))
                src_path = matches[0] if matches else src_path

            dest_path = extract_to / spec["dest"]
            if src_path.exists():
                shutil.copy2(str(src_path), str(dest_path))

        # Also extract dtbs if present
        for dtb_dir_name in ("dtbs-lts", "dtbs-rpi"):
            dtb_src = tmp_dir / "boot" / dtb_dir_name
            if dtb_src.is_dir():
                dtb_dst = extract_to / dtb_dir_name
                if dtb_dst.exists():
                    shutil.rmtree(str(dtb_dst))
                shutil.copytree(str(dtb_src), str(dtb_dst))

        shutil.rmtree(str(tmp_dir))
        tmp_path.unlink(missing_ok=True)
    except Exception:
        tmp_path.unlink(missing_ok=True)
        raise


def _download_resource(entry: dict, assets_dir: Path) -> None:
    src_type = entry["source"]["type"]
    if src_type == "file":
        _download_file(entry, assets_dir)
    elif src_type == "archive":
        _download_archive(entry, assets_dir)
    elif src_type == "apk":
        _download_apk(entry, assets_dir)
    else:
        raise HelmResourceError(f"unknown source type: {src_type}")


# ── Public API ───────────────────────────────────────────────────────────────

def obtain_resource(
    resource_id: str,
    *,
    download: bool = True,
) -> Resource:
    """Obtain a resource by ID, downloading if necessary.

    This is the primary entry point, modeled after gem5's
    ``obtain_resource()``.

    Parameters
    ----------
    resource_id : str
        The manifest ID (e.g. ``"linux-rpi-kernel"``, ``"fish-shell"``).
    download : bool
        If True (default), download missing resources automatically.
        If False, raise ``FileNotFoundError`` for missing resources.

    Returns
    -------
    Resource
        A handle with ``.path()`` to get absolute filesystem paths.

    Raises
    ------
    HelmResourceError
        If the resource ID is unknown or download/verification fails.
    FileNotFoundError
        If ``download=False`` and the resource is not present locally.

    Examples
    --------
    >>> kernel = obtain_resource("linux-rpi-kernel")
    >>> kernel.path("vmlinuz-rpi")
    '/home/user/helm-ng/assets/aarch64/boot/linux/vmlinuz-rpi'

    >>> fish = obtain_resource("fish-shell")
    >>> fish.path("fish")
    '/home/user/helm-ng/assets/aarch64/binaries/fish'

    >>> l4re = obtain_resource("l4re-hello")
    >>> l4re.path()
    '/home/user/helm-ng/assets/aarch64/boot/l4re/l4re_hello-2_arm_virt.elf'
    """
    manifest = _load_manifest()
    entry = None
    for r in manifest["resources"]:
        if r["id"] == resource_id:
            entry = r
            break

    if entry is None:
        available = [r["id"] for r in manifest["resources"]]
        raise HelmResourceError(
            f"unknown resource: {resource_id!r}\n"
            f"available: {', '.join(available)}"
        )

    resource = Resource(entry, _ASSETS_DIR)

    if not resource.is_present:
        if not download:
            raise FileNotFoundError(
                f"resource {resource_id!r} not found locally; "
                f"run: scripts/manage-assets.sh download {resource_id}"
            )
        print(
            f"[helm.resources] downloading {resource_id}: "
            f"{resource.description}",
            file=sys.stderr,
        )
        _download_resource(entry, _ASSETS_DIR)

    return resource


def resource_path(resource_id: str, filename: Optional[str] = None) -> str:
    """Convenience: obtain resource and return path in one call.

    Parameters
    ----------
    resource_id : str
        Manifest resource ID.
    filename : str, optional
        File within the resource to return.

    Returns
    -------
    str
        Absolute path to the file.
    """
    return obtain_resource(resource_id).path(filename)


def list_resources(
    category: Optional[str] = None,
    architecture: Optional[str] = None,
) -> list[Resource]:
    """List all known resources, optionally filtered.

    Parameters
    ----------
    category : str, optional
        Filter by category (boot, binary, fs, tool).
    architecture : str, optional
        Filter by architecture (aarch64, riscv64, x86_64).
    """
    manifest = _load_manifest()
    results = []
    for entry in manifest["resources"]:
        if category and entry.get("category") != category:
            continue
        if architecture and entry.get("architecture") != architecture:
            continue
        results.append(Resource(entry, _ASSETS_DIR))
    return results
