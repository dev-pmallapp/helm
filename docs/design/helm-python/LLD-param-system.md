# helm-python — LLD: Param Type System

> Low-level design for the `Param.*` descriptor classes that type-check and convert Python configuration values.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-factory.md`](./LLD-factory.md)

---

## Table of Contents

1. [Design Overview](#1-design-overview)
2. [Param Base Class](#2-param-base-class)
3. [Param Type Reference](#3-param-type-reference)
4. [Unit Conversion at elaborate() Time](#4-unit-conversion-at-elaborate-time)
5. [AttrValue Encoding](#5-attrvalue-encoding)
6. [Error Handling](#6-error-handling)

---

## 1. Design Overview

The `Param.*` system defines typed descriptors for component configuration fields. Each descriptor class:

1. **Type-checks at set time** (Python `__set__` on the descriptor) — catches obvious mistakes immediately with a Python traceback at the line of assignment.
2. **Stores the raw Python value** — no conversion at this stage.
3. **Converts to Rust `AttrValue`** at `elaborate()` time — unit conversion (e.g., "32KiB" → 32768) happens here, using the full parameter context.

This split is deliberate (Q98): fail fast on type errors (wrong Python type), fail at elaborate on semantic errors (out-of-range value, unsatisfiable constraint).

### Validation Split Summary

| Check | When | Where |
|---|---|---|
| Python type (e.g., `str` passed to `Param.Int`) | At `__set__` | Python descriptor |
| Format validity (e.g., "32KiB" is a valid size string) | At `__set__` | Python descriptor |
| Range check (e.g., cache size must be power-of-two) | At `elaborate()` | Rust side |
| Cross-param consistency (e.g., assoc ≤ size/line_size) | At `elaborate()` | Rust side |
| Unit conversion (e.g., ns → cycles) | At `elaborate()` | Rust side (with MicroarchProfile.clock_hz) |

---

## 2. Param Base Class

All `Param.*` types inherit from `ParamDescriptor`. The descriptor protocol (`__get__`/`__set__`) is used so that class-level field definitions automatically apply to instances without requiring `__init__` boilerplate.

```python
# helm_ng/params.py

from __future__ import annotations
from typing import Any, ClassVar, Type

class ParamDescriptor:
    """
    Base class for all Param.* descriptor types.

    Subclasses must implement:
      - _validate(value) -> normalized_value  (raises TypeError or ValueError on bad input)
      - to_rust_value(value) -> int | float | bool | str  (converts to Rust-compatible primitive)
      - TYPE_NAME: ClassVar[str]  (human-readable name for error messages)
    """

    TYPE_NAME: ClassVar[str] = "unknown"

    def __init__(self, default=None):
        self._default = default
        self._name: str | None = None   # set by __set_name__

    def __set_name__(self, owner, name: str):
        self._name = name

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self   # class-level access returns the descriptor itself
        return obj.__dict__.get(self._name, self._default)

    def __set__(self, obj, value):
        try:
            normalized = self._validate(value)
        except (TypeError, ValueError) as e:
            field = self._name or self.TYPE_NAME
            raise type(e)(
                f"Param.{self.TYPE_NAME} '{field}': {e}"
            ) from None
        obj.__dict__[self._name] = normalized

    def _validate(self, value) -> Any:
        raise NotImplementedError

    def to_rust_value(self, value) -> int | float | bool | str:
        """Convert a stored value to a Rust-compatible primitive."""
        return value
```

---

## 3. Param Type Reference

### Param.MemorySize

Accepts a memory size as a string (`"32KiB"`, `"256MiB"`, `"1GiB"`), a decimal string (`"32768"`), or a plain integer (`32768`). Validates that the value is positive and a power of two. Stores as `int` (bytes).

**Accepted formats:**
- `"32KiB"` / `"32 KiB"` — kibibytes (1024)
- `"64KB"` / `"64 KB"` — same as KiB in this system (1024, not 1000)
- `"256MiB"` — mebibytes
- `"1GiB"` — gibibytes
- `"32768"` — decimal string
- `32768` — plain Python int

**Rust `AttrValue`:** `AttrValue::Int(bytes: i64)`.

```python
class MemorySize(ParamDescriptor):
    TYPE_NAME = "MemorySize"

    _SUFFIXES = {
        "b": 1, "kb": 1024, "kib": 1024,
        "mb": 1024**2, "mib": 1024**2,
        "gb": 1024**3, "gib": 1024**3,
        "tb": 1024**4, "tib": 1024**4,
    }

    def _validate(self, value) -> int:
        if isinstance(value, int):
            bytes_ = value
        elif isinstance(value, str):
            bytes_ = self._parse_str(value)
        else:
            raise TypeError(f"expected str or int, got {type(value).__name__}")
        if bytes_ <= 0:
            raise ValueError(f"size must be positive, got {bytes_}")
        # Power-of-two check deferred to Rust elaborate() for range check
        return bytes_

    def _parse_str(self, s: str) -> int:
        s = s.strip()
        # Try plain integer string first
        try:
            return int(s)
        except ValueError:
            pass
        # Try suffix form
        for suffix, mult in sorted(self._SUFFIXES.items(), key=lambda x: -len(x[0])):
            if s.lower().rstrip().endswith(suffix):
                num_part = s[:-(len(suffix))].strip()
                try:
                    return int(float(num_part) * mult)
                except ValueError:
                    raise ValueError(f"cannot parse size string: {s!r}")
        raise ValueError(f"cannot parse size string: {s!r}")

    def to_rust_value(self, value) -> int:
        return value  # already bytes (int)
```

### Param.Int

Accepts a plain Python `int`. Stores as `int`. No conversion.

```python
class Int(ParamDescriptor):
    TYPE_NAME = "Int"

    def _validate(self, value) -> int:
        if not isinstance(value, int) or isinstance(value, bool):
            raise TypeError(f"expected int, got {type(value).__name__}")
        return value

    def to_rust_value(self, value) -> int:
        return value
```

### Param.Addr

Accepts a Python `int` (address). Stores as `int`. May be passed as hex literal (`0x8000_0000`).

```python
class Addr(ParamDescriptor):
    TYPE_NAME = "Addr"

    def _validate(self, value) -> int:
        if not isinstance(value, int) or isinstance(value, bool):
            raise TypeError(f"expected int (address), got {type(value).__name__}")
        if value < 0:
            raise ValueError(f"address must be non-negative, got {value:#x}")
        return value

    def to_rust_value(self, value) -> int:
        return value
```

### Param.Bool

Accepts `True` or `False`. Rejects strings and integers (explicit type safety).

```python
class Bool(ParamDescriptor):
    TYPE_NAME = "Bool"

    def _validate(self, value) -> bool:
        if not isinstance(value, bool):
            raise TypeError(f"expected bool, got {type(value).__name__}")
        return value

    def to_rust_value(self, value) -> bool:
        return value
```

### Param.Cycles

Accepts a plain Python `int`. Represents a count of clock cycles — no conversion at Python side.

**Rust `AttrValue`:** `AttrValue::Int(cycles: i64)`. The Rust side uses this value directly as a cycle count.

```python
class Cycles(ParamDescriptor):
    TYPE_NAME = "Cycles"

    def _validate(self, value) -> int:
        if not isinstance(value, int) or isinstance(value, bool):
            raise TypeError(f"expected int (cycles), got {type(value).__name__}")
        if value < 0:
            raise ValueError(f"cycle count must be non-negative, got {value}")
        return value

    def to_rust_value(self, value) -> int:
        return value
```

### Param.Nanoseconds

Accepts a Python `int` or `float` representing a duration in nanoseconds. Stored as `float`.

**At `elaborate()` time on the Rust side:** converted to cycles via `MicroarchProfile.clock_hz`:
```
cycles = round(nanoseconds * clock_hz / 1_000_000_000)
```
This conversion requires `clock_hz`, which is only available at elaborate time, not at Python attribute-set time.

**Rust `AttrValue`:** `AttrValue::Float(ns: f64)`. The Rust side performs the conversion.

```python
class Nanoseconds(ParamDescriptor):
    TYPE_NAME = "Nanoseconds"

    def _validate(self, value) -> float:
        if not isinstance(value, (int, float)):
            raise TypeError(f"expected int or float (nanoseconds), got {type(value).__name__}")
        if value < 0:
            raise ValueError(f"nanoseconds must be non-negative, got {value}")
        return float(value)

    def to_rust_value(self, value) -> float:
        return value
```

### Param.Hz

Accepts a Python `int` or `float` representing a frequency in Hertz. Stored as `float`.

**At `elaborate()` time:** used to configure `MicroarchProfile.clock_hz`. Also accepted as MHz/GHz strings by a helper (optional extension).

**Rust `AttrValue`:** `AttrValue::Float(hz: f64)`.

```python
class Hz(ParamDescriptor):
    TYPE_NAME = "Hz"

    def _validate(self, value) -> float:
        if isinstance(value, str):
            value = self._parse_hz_str(value)
        elif not isinstance(value, (int, float)):
            raise TypeError(f"expected int, float, or str (Hz), got {type(value).__name__}")
        if value <= 0:
            raise ValueError(f"frequency must be positive, got {value}")
        return float(value)

    def _parse_hz_str(self, s: str) -> float:
        s = s.strip()
        multipliers = {"ghz": 1e9, "mhz": 1e6, "khz": 1e3, "hz": 1.0}
        for suffix, mult in sorted(multipliers.items(), key=lambda x: -len(x[0])):
            if s.lower().endswith(suffix):
                return float(s[:-(len(suffix))].strip()) * mult
        return float(s)  # plain number string

    def to_rust_value(self, value) -> float:
        return value
```

### Param.Isa

Accepts an `Isa` enum value. Stores the enum member. Converts to string on the Rust side.

```python
from .enums import Isa as IsaEnum

class Isa(ParamDescriptor):
    TYPE_NAME = "Isa"

    def _validate(self, value) -> IsaEnum:
        if not isinstance(value, IsaEnum):
            raise TypeError(f"expected Isa enum, got {type(value).__name__}")
        return value

    def to_rust_value(self, value) -> str:
        return value.value   # e.g. "riscv", "aarch64"
```

### Param.ExecMode

Accepts an `ExecMode` enum value.

```python
from .enums import ExecMode as ExecModeEnum

class ExecMode(ParamDescriptor):
    TYPE_NAME = "ExecMode"

    def _validate(self, value) -> ExecModeEnum:
        if not isinstance(value, ExecModeEnum):
            raise TypeError(f"expected ExecMode enum, got {type(value).__name__}")
        return value

    def to_rust_value(self, value) -> str:
        return value.value   # e.g. "functional", "syscall", "system"
```

### Param.Timing

Accepts a `Timing` enum value.

```python
from .enums import Timing as TimingEnum

class Timing(ParamDescriptor):
    TYPE_NAME = "Timing"

    def _validate(self, value) -> TimingEnum:
        if not isinstance(value, TimingEnum):
            raise TypeError(f"expected Timing enum, got {type(value).__name__}")
        return value

    def to_rust_value(self, value) -> str:
        return value.value   # e.g. "virtual", "interval", "accurate"
```

### Enum Definitions

```python
# helm_ng/enums.py

from enum import Enum

class Isa(str, Enum):
    RiscV   = "riscv"
    AArch64 = "aarch64"
    AArch32 = "aarch32"

class ExecMode(str, Enum):
    Functional = "functional"
    Syscall    = "syscall"
    System     = "system"

class Timing(str, Enum):
    Virtual  = "virtual"
    Interval = "interval"
    Accurate = "accurate"
```

---

## 4. Unit Conversion at elaborate() Time

The Rust side performs all unit conversions that require a `MicroarchProfile`. The profile is instantiated from JSON at elaborate time and provides `clock_hz`.

```rust
// crates/helm-engine/src/params.rs

use helm_timing::MicroarchProfile;

/// Convert an AttrValue to cycles using the provided profile.
/// Called during World::instantiate() for each Cycles/Nanoseconds/Hz param.
pub fn to_cycles(value: &AttrValue, profile: &MicroarchProfile) -> Result<u64, ConfigError> {
    match value {
        AttrValue::Int(cycles) => {
            // Param.Cycles — already in cycles
            if *cycles < 0 {
                return Err(ConfigError::ParamRange {
                    field: "cycles",
                    reason: "must be non-negative",
                });
            }
            Ok(*cycles as u64)
        }
        AttrValue::Float(ns) => {
            // Param.Nanoseconds — convert to cycles
            // cycles = round(ns * clock_hz / 1e9)
            let cycles = (ns * profile.clock_hz as f64 / 1_000_000_000.0).round() as u64;
            Ok(cycles)
        }
        _ => Err(ConfigError::ParamType {
            field: "cycles_or_ns",
            expected: "Int or Float",
        }),
    }
}
```

### Conversion Examples at elaborate() Time

| Param type | Python value | Stored AttrValue | Rust cycles (at 3 GHz) |
|---|---|---|---|
| `Param.Cycles` | `4` | `Int(4)` | 4 |
| `Param.Nanoseconds` | `1.33` | `Float(1.33)` | `round(1.33 * 3e9 / 1e9)` = 4 |
| `Param.Hz` | `3_000_000_000` | `Float(3e9)` | stored as profile.clock_hz |

---

## 5. AttrValue Encoding

The `AttrValue` Rust enum is the universal type for crossing the Python → Rust param boundary:

```rust
// crates/helm-engine/src/attr.rs

#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<AttrValue>),
}
```

The mapping from `Param.*` type to `AttrValue` variant:

| Param type | AttrValue variant |
|---|---|
| `Param.Int` | `Int(i64)` |
| `Param.MemorySize` | `Int(bytes as i64)` |
| `Param.Addr` | `Int(addr as i64)` |
| `Param.Cycles` | `Int(cycles as i64)` |
| `Param.Bool` | `Bool(bool)` |
| `Param.Nanoseconds` | `Float(ns as f64)` |
| `Param.Hz` | `Float(hz as f64)` |
| `Param.Isa` | `Str("riscv" | "aarch64" | "aarch32")` |
| `Param.ExecMode` | `Str("functional" | "syscall" | "system")` |
| `Param.Timing` | `Str("virtual" | "interval" | "accurate")` |

---

## 6. Error Handling

### Python-side errors (at set time)

All `Param.*` descriptor `__set__` methods raise standard Python exceptions:

- `TypeError` — wrong Python type (e.g., string passed to `Param.Int`)
- `ValueError` — correct type but invalid value (e.g., negative MemorySize)

These exceptions include a field name prefix so the user can locate the bad assignment:

```
TypeError: Param.MemorySize 'size': expected str or int, got list
```

### Rust-side errors (at elaborate time)

Range checks and cross-param consistency checks are done in Rust and returned as `HelmError::Config(ConfigError::ParamRange { field, reason })`. These are mapped to `HelmConfigError` in Python:

```python
from helm_ng import Simulation, Cpu, L1Cache
import pytest

def test_bad_cache_size():
    sim = Simulation(root=Cpu(icache=L1Cache(size="3KiB")))  # non-power-of-two
    with pytest.raises(HelmConfigError, match="power-of-two"):
        sim.elaborate()    # error raised here, not at L1Cache(size="3KiB")
```

Note: `"3KiB"` (3072 bytes) passes Python-side validation (it is a valid size string) but fails Rust-side validation (cache sizes must be powers of two).

---

*For the factory function and plugin loader, see [`LLD-factory.md`](./LLD-factory.md). For tests covering each param type, see [`TEST.md`](./TEST.md).*
