"""Runtime DTB builder — Python-side mirror of helm-device's RuntimeDtb.

Allows Python scripts to:
1. Load / generate a base DTB
2. Add devices at any point (including mid-simulation hot-plug)
3. Re-serialize to bytes for injection into guest RAM

Usage::

    from helm.dtb import RuntimeDtb, DeviceSpec

    dtb = RuntimeDtb.from_platform(platform, ram_size="1G", num_cpus=4)

    # CLI-style device addition
    dtb.add_device(DeviceSpec.parse("virtio-net-device,addr=0xa000000"))

    # Or add a raw Python device
    dtb.add_device_obj(my_uart, base=0x9040000)

    # Get the blob for guest RAM
    blob = dtb.to_bytes()

    # Hot-plug at runtime (regenerates the blob)
    dtb.add_device(DeviceSpec.parse("virtio-blk-device,addr=0xa000200"))
    new_blob = dtb.to_bytes()
"""

from __future__ import annotations

import struct
from typing import Any, Dict, List, Optional, Tuple

from helm.devices.base import Device


class DeviceSpec:
    """Parsed ``-device`` / ``-driver`` specification.

    Format: ``TYPE[,key=val,...]``
    """

    def __init__(self, type_name: str, properties: Optional[Dict[str, str]] = None):
        self.type_name = type_name
        self.properties: Dict[str, str] = properties or {}

    @classmethod
    def parse(cls, spec: str) -> "DeviceSpec":
        parts = spec.split(",")
        type_name = parts[0] if parts else ""
        props: Dict[str, str] = {}
        for part in parts[1:]:
            if "=" in part:
                k, v = part.split("=", 1)
                props[k] = v
        return cls(type_name, props)

    def get(self, key: str, default: Optional[str] = None) -> Optional[str]:
        return self.properties.get(key, default)

    def get_int(self, key: str, default: int = 0) -> int:
        val = self.properties.get(key)
        if val is None:
            return default
        if val.startswith("0x") or val.startswith("0X"):
            return int(val, 16)
        return int(val)

    def __repr__(self) -> str:
        props = ",".join(f"{k}={v}" for k, v in self.properties.items())
        return f"DeviceSpec({self.type_name!r}, {{{props}}})"


class RuntimeDtb:
    """Mutable device-tree that can be patched and re-serialized at any time.

    Mirrors the Rust ``RuntimeDtb`` in ``helm-device/src/fdt.rs``.
    """

    def __init__(self) -> None:
        self._nodes: List[Dict[str, Any]] = []
        self._extra_specs: List[DeviceSpec] = []
        self._ram_base: int = 0x4000_0000
        self._ram_size: int = 256 * 1024 * 1024
        self._num_cpus: int = 1
        self._bootargs: str = ""
        self._initrd: Optional[Tuple[int, int]] = None
        self._gic_dist: int = 0x0800_0000
        self._gic_cpu: int = 0x0801_0000
        self._gic_version: int = 2
        self._next_spi: int = 32
        self._platform_name: str = "helm-virt"
        self._base_blob: Optional[bytes] = None

    @classmethod
    def from_platform(
        cls,
        platform,
        *,
        ram_size: str = "256M",
        num_cpus: int = 1,
        bootargs: str = "",
        base_dtb: Optional[bytes] = None,
    ) -> "RuntimeDtb":
        """Create from a Python platform object."""
        dtb = cls()
        dtb._platform_name = getattr(platform, "name", "helm-virt")
        dtb._num_cpus = num_cpus
        dtb._bootargs = bootargs
        dtb._base_blob = base_dtb
        dtb._ram_size = _parse_ram_size(ram_size)

        if hasattr(platform, "devices"):
            for dev in platform.devices:
                if hasattr(dev, "to_dict"):
                    dtb._nodes.append(dev.to_dict())
        return dtb

    def add_device(self, spec: DeviceSpec) -> None:
        """Add a device from a parsed spec (hot-plug friendly)."""
        self._extra_specs.append(spec)

    def add_device_obj(self, device: Device, base: int = 0) -> None:
        """Add a Python Device object."""
        device.base_address = base
        if hasattr(device, "to_dict"):
            self._nodes.append(device.to_dict())

    def remove_device(self, name: str) -> bool:
        """Remove a device node by name."""
        before = len(self._nodes)
        self._nodes = [n for n in self._nodes if n.get("name") != name]
        return len(self._nodes) != before

    def to_config(self) -> Dict[str, Any]:
        """Export the current state as a JSON-serializable dict.

        This dict is consumed by the Rust CLI when launched from Python.
        """
        return {
            "platform": self._platform_name,
            "ram_base": self._ram_base,
            "ram_size": self._ram_size,
            "num_cpus": self._num_cpus,
            "bootargs": self._bootargs,
            "initrd": self._initrd,
            "gic_dist_base": self._gic_dist,
            "gic_cpu_base": self._gic_cpu,
            "gic_version": self._gic_version,
            "devices": self._nodes,
            "extra_specs": [
                {"type": s.type_name, **s.properties}
                for s in self._extra_specs
            ],
        }

    def to_bytes(self) -> bytes:
        """Serialize the device tree to a DTB blob.

        For full fidelity, prefer piping ``to_config()`` through the Rust
        ``RuntimeDtb``.  This pure-Python path generates a minimal valid
        blob suitable for testing and development.
        """
        import json as _json
        cfg = self.to_config()
        cfg_json = _json.dumps(cfg)
        return _minimal_dtb(cfg_json.encode(), self._platform_name)


def _parse_ram_size(s: str) -> int:
    s = s.strip()
    if not s:
        return 256 * 1024 * 1024
    multipliers = {"G": 1 << 30, "g": 1 << 30, "M": 1 << 20, "m": 1 << 20,
                   "K": 1 << 10, "k": 1 << 10}
    if s[-1] in multipliers:
        return int(s[:-1]) * multipliers[s[-1]]
    return int(s)


def _minimal_dtb(config_blob: bytes, name: str) -> bytes:
    """Build a tiny but valid DTB that embeds the config as a /chosen property.

    The Rust loader extracts this and rebuilds the full tree.
    """
    FDT_MAGIC = 0xD00DFEED
    FDT_BEGIN_NODE = 1
    FDT_END_NODE = 2
    FDT_PROP = 3
    FDT_END = 9

    strings = b"helm,config\x00compatible\x00"
    str_off_config = 0
    str_off_compat = 12

    struct_buf = bytearray()

    def _pad4(b: bytearray) -> None:
        while len(b) % 4:
            b.append(0)

    # root node
    struct_buf += struct.pack(">I", FDT_BEGIN_NODE)
    struct_buf += b"\x00"
    _pad4(struct_buf)

    # compatible property
    compat_val = f"helm,{name}\x00".encode()
    struct_buf += struct.pack(">III", FDT_PROP, len(compat_val), str_off_compat)
    struct_buf += compat_val
    _pad4(struct_buf)

    # helm,config property (carries the JSON)
    val = config_blob + b"\x00"
    struct_buf += struct.pack(">III", FDT_PROP, len(val), str_off_config)
    struct_buf += val
    _pad4(struct_buf)

    # end root + end
    struct_buf += struct.pack(">I", FDT_END_NODE)
    struct_buf += struct.pack(">I", FDT_END)

    hdr_size = 40
    mem_rsv = b"\x00" * 16  # empty reservation map
    off_mem_rsv = hdr_size
    off_struct = off_mem_rsv + len(mem_rsv)
    off_strings = off_struct + len(struct_buf)
    total = off_strings + len(strings)

    hdr = struct.pack(
        ">10I",
        FDT_MAGIC, total, off_struct, off_strings, off_mem_rsv,
        17, 16, 0, len(strings), len(struct_buf),
    )

    return hdr + mem_rsv + bytes(struct_buf) + strings
