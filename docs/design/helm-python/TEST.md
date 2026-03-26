# helm-python — Test Plan

> Test strategy and test cases for the `helm-python` crate and `helm` Python package.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-param-system.md`](./LLD-param-system.md) · [`LLD-instantiate.md`](./LLD-instantiate.md)

---

## Table of Contents

1. [Test Categories](#1-test-categories)
2. [SimObject Hierarchy Tests](#2-simobject-hierarchy-tests)
3. [System Instantiation Tests](#3-system-instantiation-tests)
4. [Parameter Validation Tests](#4-parameter-validation-tests)
5. [Memory Map Tests](#5-memory-map-tests)
6. [Port Wiring Tests](#6-port-wiring-tests)
7. [Device Introspection Tests](#7-device-introspection-tests)
8. [Backward Compatibility Tests](#8-backward-compatibility-tests)
9. [Pre-Built Board Tests](#9-pre-built-board-tests)
10. [GIL and Concurrency Tests](#10-gil-and-concurrency-tests)
11. [Exception Mapping Tests](#11-exception-mapping-tests)
12. [Test Infrastructure](#12-test-infrastructure)

---

## 1. Test Categories

| Category | Location | Runner | Scope |
|---|---|---|---|
| SimObject hierarchy | `tests/python/test_simobject.py` | `pytest` | Child assignment, state tracking |
| System instantiation | `tests/python/test_instantiate.py` | `pytest` | Full instantiate flow |
| Parameter validation | `tests/python/test_params.py` | `pytest` | Type checking, size parsing |
| Memory map | `tests/python/test_memory_map.py` | `pytest` | add_map, overlap detection |
| Port wiring | `tests/python/test_ports.py` | `pytest` | PortRef, resolution |
| Device introspection | `tests/python/test_introspection.py` | `pytest` | Post-instantiate property access |
| Backward compat | `tests/python/test_compat.py` | `pytest` | build_simulation() |
| Pre-built boards | `tests/python/test_boards.py` | `pytest` | ArmVirt |
| GIL concurrency | `tests/python/test_gil.py` | `pytest` | Thread safety |
| Exceptions | `tests/python/test_exceptions.py` | `pytest` | Error mapping |
| Rust unit tests | `src/*.rs #[cfg(test)]` | `cargo test` | Internal Rust logic |

---

## 2. SimObject Hierarchy Tests

```python
# tests/python/test_simobject.py

import pytest
import helm


class TestSimObjectBase:
    def test_create_simobject(self):
        """SimObject can be created with a name."""
        obj = helm.SimObject("test")
        assert obj.name == "test"
        assert not obj.instantiated

    def test_child_assignment(self):
        """Assigning a SimObject as an attribute registers it as a child."""
        system = helm.System("sys", timing="virtual", mode="se")
        cpu = helm.Cpu("cpu0", isa="aarch64")
        system.cpu = cpu
        assert system.cpu.name == "cpu0"

    def test_child_access_nonexistent(self):
        """Accessing a nonexistent child raises AttributeError."""
        system = helm.System("sys", timing="virtual", mode="se")
        with pytest.raises(AttributeError, match="no child 'missing'"):
            _ = system.missing

    def test_multiple_children(self):
        """Multiple children can be assigned."""
        system = helm.System("sys", timing="virtual", mode="fs")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        system.gic = helm.GicV2("gic0")
        system.uart = helm.Pl011("uart0")
        assert system.cpu.name == "cpu0"
        assert system.gic.name == "gic0"
        assert system.uart.name == "uart0"


class TestSimObjectFreezing:
    def test_mutation_after_instantiate_raises(self):
        """Cannot add children after instantiate()."""
        system = helm.System("sys", timing="virtual", mode="se")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        system.ram = helm.Ram("ram0", size="64MiB")
        system.instantiate()

        with pytest.raises(RuntimeError, match="after instantiate"):
            system.extra = helm.Cpu("cpu1", isa="aarch64")

    def test_instantiated_flag(self):
        """After instantiate(), .instantiated returns True."""
        system = helm.System("sys", timing="virtual", mode="se")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        system.ram = helm.Ram("ram0", size="64MiB")
        assert not system.instantiated
        system.instantiate()
        assert system.instantiated
```

---

## 3. System Instantiation Tests

```python
# tests/python/test_instantiate.py

import pytest
import helm


def make_se_system():
    """Create a minimal SE system."""
    system = helm.System("se", timing="virtual", mode="se")
    system.cpu = helm.Cpu("cpu0", isa="aarch64")
    system.ram = helm.Ram("ram0", size="64MiB")
    return system


def make_fs_system():
    """Create a minimal FS system with GIC + UART."""
    system = helm.System("virt", timing="virtual", mode="fs")
    system.cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
    system.gic = helm.GicV2("gic0", num_irqs=96)
    system.uart = helm.Pl011("uart0")
    system.ram = helm.Ram("ram0", size="512MiB")
    system.mem = helm.MemorySpace("phys_mem")
    system.mem.add_map(0x4000_0000, system.ram, "512MiB")
    system.mem.add_map(0x0800_0000, system.gic, 0x1_0000, bank=0)
    system.mem.add_map(0x0801_0000, system.gic, 0x1_0000, bank=1)
    system.mem.add_map(0x0900_0000, system.uart, 0x1000)
    system.uart.irq = system.gic.spi(33)
    return system


class TestInstantiateSE:
    def test_basic_instantiate(self):
        """Minimal SE system instantiates without error."""
        system = make_se_system()
        system.instantiate()
        assert system.instantiated

    def test_run_zero_instructions(self):
        """Run with 0 instructions returns immediately."""
        system = make_se_system()
        system.instantiate()
        result = system.run(0)
        assert result == "quantum"

    def test_run_before_instantiate_raises(self):
        """run() before instantiate() raises RuntimeError."""
        system = make_se_system()
        with pytest.raises(RuntimeError, match="instantiate"):
            system.run(100)

    def test_double_instantiate_raises(self):
        """Calling instantiate() twice raises."""
        system = make_se_system()
        system.instantiate()
        with pytest.raises(RuntimeError):
            system.instantiate()

    def test_insn_count_starts_zero(self):
        """Instruction count starts at zero."""
        system = make_se_system()
        system.instantiate()
        assert system.insn_count == 0


class TestInstantiateFS:
    def test_fs_instantiate(self):
        """FS system with GIC + UART instantiates."""
        system = make_fs_system()
        system.instantiate()
        assert system.instantiated

    def test_fs_requires_mem(self):
        """FS mode without MemorySpace child raises."""
        system = helm.System("virt", timing="virtual", mode="fs")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        with pytest.raises(helm.HelmConfigError, match="mem"):
            system.instantiate()

    def test_fs_requires_cpu(self):
        """FS mode without Cpu child raises."""
        system = helm.System("virt", timing="virtual", mode="fs")
        system.mem = helm.MemorySpace("phys_mem")
        with pytest.raises(helm.HelmConfigError, match="cpu"):
            system.instantiate()


class TestTimingVariants:
    def test_virtual_timing(self):
        system = make_se_system()
        system.timing = "virtual"
        system.instantiate()

    def test_interval_timing(self):
        system = make_se_system()
        system.timing = "interval"
        system.instantiate()

    def test_accurate_timing(self):
        system = make_se_system()
        system.timing = "accurate"
        system.instantiate()

    def test_unknown_timing_raises(self):
        system = make_se_system()
        system.timing = "supercycle"
        with pytest.raises(helm.HelmConfigError, match="timing"):
            system.instantiate()
```

---

## 4. Parameter Validation Tests

```python
# tests/python/test_params.py

import pytest
import helm


class TestCpuParams:
    def test_valid_isa(self):
        cpu = helm.Cpu("cpu0", isa="aarch64")
        assert cpu.isa == "aarch64"

    def test_valid_model(self):
        cpu = helm.Cpu("cpu0", model="cortex-a55")
        assert cpu.model == "cortex-a55"

    def test_width_int(self):
        cpu = helm.Cpu("cpu0")
        cpu.width = 8
        assert cpu.width == 8

    def test_width_rejects_string(self):
        cpu = helm.Cpu("cpu0")
        with pytest.raises(TypeError):
            cpu.width = "four"

    def test_width_rejects_negative(self):
        cpu = helm.Cpu("cpu0")
        with pytest.raises(OverflowError):
            cpu.width = -1


class TestRamParams:
    def test_size_string(self):
        ram = helm.Ram("ram0", size="1GiB")
        assert ram.size == "1GiB"

    def test_size_mib(self):
        ram = helm.Ram("ram0", size="256MiB")
        assert ram.size == "256MiB"


class TestGicV2Params:
    def test_num_irqs_default(self):
        gic = helm.GicV2("gic0")
        assert gic.num_irqs == 96

    def test_num_irqs_custom(self):
        gic = helm.GicV2("gic0", num_irqs=256)
        assert gic.num_irqs == 256


class TestCacheParams:
    def test_defaults(self):
        cache = helm.Cache("l1d")
        assert cache.size == "32KiB"
        assert cache.assoc == 8
        assert cache.latency == 4
        assert cache.line_size == 64

    def test_custom(self):
        cache = helm.Cache("l2", size="256KiB", assoc=16, latency=12)
        assert cache.size == "256KiB"
        assert cache.assoc == 16
        assert cache.latency == 12
```

---

## 5. Memory Map Tests

```python
# tests/python/test_memory_map.py

import pytest
import helm


class TestMemorySpace:
    def test_add_map_int_size(self):
        """add_map accepts integer size."""
        mem = helm.MemorySpace("phys_mem")
        ram = helm.Ram("ram0", size="64MiB")
        mem.add_map(0x4000_0000, ram, 0x400_0000)  # 64 MiB

    def test_add_map_string_size(self):
        """add_map accepts string size."""
        mem = helm.MemorySpace("phys_mem")
        ram = helm.Ram("ram0", size="64MiB")
        mem.add_map(0x4000_0000, ram, "64MiB")

    def test_add_map_with_bank(self):
        """add_map accepts bank parameter."""
        mem = helm.MemorySpace("phys_mem")
        gic = helm.GicV2("gic0")
        mem.add_map(0x0800_0000, gic, 0x1_0000, bank=0)
        mem.add_map(0x0801_0000, gic, 0x1_0000, bank=1)

    def test_overlap_detection(self):
        """Overlapping map entries raise at instantiate."""
        system = helm.System("virt", timing="virtual", mode="fs")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        system.ram = helm.Ram("ram0", size="64MiB")
        system.mem = helm.MemorySpace("phys_mem")

        # These two overlap
        system.mem.add_map(0x1000, system.ram, 0x2000)
        extra_ram = helm.Ram("ram1", size="64MiB")
        system.extra_ram = extra_ram
        system.mem.add_map(0x1800, extra_ram, 0x100)

        with pytest.raises(helm.HelmConfigError, match="overlap"):
            system.instantiate()
```

---

## 6. Port Wiring Tests

```python
# tests/python/test_ports.py

import pytest
import helm


class TestPortRef:
    def test_gic_spi_returns_portref(self):
        """gic.spi(N) returns a PortRef."""
        gic = helm.GicV2("gic0")
        ref = gic.spi(33)
        assert isinstance(ref, helm.PortRef)
        assert "spi[33]" in repr(ref)

    def test_assign_portref_to_device(self):
        """Device irq accepts a PortRef."""
        uart = helm.Pl011("uart0")
        gic = helm.GicV2("gic0")
        uart.irq = gic.spi(33)
        assert uart.irq is not None

    def test_unresolved_port_raises(self):
        """PortRef referencing nonexistent child raises at instantiate."""
        system = helm.System("virt", timing="virtual", mode="fs")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        system.ram = helm.Ram("ram0", size="64MiB")
        system.uart = helm.Pl011("uart0")
        system.mem = helm.MemorySpace("phys_mem")
        system.mem.add_map(0x4000_0000, system.ram, "64MiB")
        system.mem.add_map(0x0900_0000, system.uart, 0x1000)

        # Wire to a GIC that's not a child of system
        orphan_gic = helm.GicV2("gic_orphan")
        system.uart.irq = orphan_gic.spi(33)

        with pytest.raises(helm.HelmConfigError, match="unresolved"):
            system.instantiate()

    def test_none_irq_is_valid(self):
        """Device with irq=None instantiates (IRQ not wired)."""
        system = helm.System("virt", timing="virtual", mode="fs")
        system.cpu = helm.Cpu("cpu0", isa="aarch64")
        system.ram = helm.Ram("ram0", size="64MiB")
        system.uart = helm.Pl011("uart0")
        system.mem = helm.MemorySpace("phys_mem")
        system.mem.add_map(0x4000_0000, system.ram, "64MiB")
        system.mem.add_map(0x0900_0000, system.uart, 0x1000)
        # uart.irq is None — no wiring
        system.instantiate()  # should not raise
```

---

## 7. Device Introspection Tests

```python
# tests/python/test_introspection.py

import pytest
import helm


def make_instantiated_system():
    system = helm.System("se", timing="virtual", mode="se")
    system.cpu = helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")
    system.ram = helm.Ram("ram0", size="64MiB")
    system.instantiate()
    return system


class TestCpuIntrospection:
    def test_pc_readable(self):
        system = make_instantiated_system()
        pc = system.cpu.pc
        assert isinstance(pc, int)

    def test_sp_readable(self):
        system = make_instantiated_system()
        sp = system.cpu.sp
        assert isinstance(sp, int)

    def test_xn_readable(self):
        system = make_instantiated_system()
        x0 = system.cpu.xn(0)
        assert isinstance(x0, int)

    def test_nzcv_readable(self):
        system = make_instantiated_system()
        nzcv = system.cpu.nzcv
        assert isinstance(nzcv, int)

    def test_pc_before_instantiate_raises(self):
        cpu = helm.Cpu("cpu0", isa="aarch64")
        with pytest.raises(RuntimeError, match="instantiate"):
            _ = cpu.pc
```

---

## 8. Backward Compatibility Tests

```python
# tests/python/test_compat.py

import pytest
import helm


class TestBuildSimulation:
    def test_basic_se(self):
        """build_simulation() creates a working SE simulation."""
        sim = helm.build_simulation(isa="aarch64", mode="se", timing="virtual")
        result = sim.run(0)
        assert result == "quantum"

    def test_returns_system(self):
        """build_simulation() returns a System object."""
        sim = helm.build_simulation(isa="aarch64", mode="se")
        assert isinstance(sim, helm.System)

    def test_has_insn_count(self):
        """Returned object has insn_count property."""
        sim = helm.build_simulation(isa="aarch64", mode="se")
        assert sim.insn_count == 0

    def test_custom_mem_size(self):
        """build_simulation() accepts mem_mib parameter."""
        sim = helm.build_simulation(isa="aarch64", mode="se", mem_mib=1024)
        result = sim.run(0)
        assert result == "quantum"

    def test_fs_mode(self):
        """build_simulation() in FS mode creates system with devices."""
        sim = helm.build_simulation(isa="aarch64", mode="fs",
                                     timing="virtual", mem_mib=512)
        assert sim.instantiated
```

---

## 9. Pre-Built Board Tests

```python
# tests/python/test_boards.py

import pytest
from helm.boards import ArmVirt


class TestArmVirt:
    def test_create(self):
        """ArmVirt() creates without error."""
        board = ArmVirt()
        assert board.system is not None

    def test_custom_mem(self):
        """ArmVirt accepts custom memory size."""
        board = ArmVirt(mem="1GiB")

    def test_custom_cpu_model(self):
        """ArmVirt accepts custom CPU model."""
        board = ArmVirt(cpu_model="cortex-a73")

    def test_instantiate(self):
        """ArmVirt.instantiate() works."""
        board = ArmVirt(mem="128MiB")
        board.instantiate()

    def test_has_gic(self):
        """ArmVirt has a GIC child."""
        board = ArmVirt()
        assert board.system.gic is not None
        assert isinstance(board.system.gic, helm.GicV2)

    def test_has_uart(self):
        """ArmVirt has a UART child."""
        board = ArmVirt()
        assert board.system.uart is not None
        assert isinstance(board.system.uart, helm.Pl011)

    def test_address_constants(self):
        """ArmVirt uses QEMU-compatible addresses."""
        assert ArmVirt.GIC_DIST  == 0x0800_0000
        assert ArmVirt.GIC_CPUIF == 0x0801_0000
        assert ArmVirt.UART0     == 0x0900_0000
        assert ArmVirt.RAM_BASE  == 0x4000_0000
```

---

## 10. GIL and Concurrency Tests

```python
# tests/python/test_gil.py

import threading
import pytest
import helm


def test_run_releases_gil():
    """system.run() releases the GIL: another thread can run concurrently."""
    system = helm.System("se", timing="virtual", mode="se")
    system.cpu = helm.Cpu("cpu0", isa="aarch64")
    system.ram = helm.Ram("ram0", size="64MiB")
    system.instantiate()

    ran_concurrently = [False]

    def background():
        ran_concurrently[0] = True

    t = threading.Thread(target=background)
    t.start()
    system.run(10_000_000)
    t.join(timeout=5.0)

    assert not t.is_alive()
    assert ran_concurrently[0]


def test_spy_session_no_deadlock():
    """HelmSpy properties can be read without deadlock."""
    system = helm.System("se", timing="virtual", mode="se")
    system.cpu = helm.Cpu("cpu0", isa="aarch64")
    system.ram = helm.Ram("ram0", size="64MiB")
    system.instantiate()

    spy = system.spy()
    system.run(10_000)
    _ = spy.insn_count  # must not deadlock
```

---

## 11. Exception Mapping Tests

```python
# tests/python/test_exceptions.py

import pytest
import helm


def test_exception_hierarchy():
    """All helm exceptions inherit from HelmError."""
    assert issubclass(helm.HelmConfigError, helm.HelmError)
    assert issubclass(helm.HelmMemFault, helm.HelmError)
    assert issubclass(helm.HelmDeviceError, helm.HelmError)
    assert issubclass(helm.HelmCheckpointError, helm.HelmError)


def test_config_error_on_bad_isa():
    """Unknown ISA raises HelmConfigError."""
    system = helm.System("se", timing="virtual", mode="se")
    system.cpu = helm.Cpu("cpu0", isa="mips")
    system.ram = helm.Ram("ram0", size="64MiB")
    with pytest.raises(helm.HelmConfigError, match="isa"):
        system.instantiate()


def test_config_error_on_bad_mode():
    """Unknown mode raises HelmConfigError."""
    system = helm.System("se", timing="virtual", mode="turbo")
    system.cpu = helm.Cpu("cpu0", isa="aarch64")
    system.ram = helm.Ram("ram0", size="64MiB")
    with pytest.raises(helm.HelmConfigError, match="mode"):
        system.instantiate()
```

---

## 12. Test Infrastructure

### pytest Configuration

```toml
# runtime/helm-python/tests/python/pytest.ini
[pytest]
testpaths = .
python_files = test_*.py
python_classes = Test*
python_functions = test_*
addopts = -v --tb=short
```

### conftest.py

```python
# runtime/helm-python/tests/python/conftest.py

import pytest
import helm


@pytest.fixture
def se_system():
    """Provide a freshly instantiated minimal SE system."""
    system = helm.System("se", timing="virtual", mode="se")
    system.cpu = helm.Cpu("cpu0", isa="aarch64")
    system.ram = helm.Ram("ram0", size="64MiB")
    system.instantiate()
    return system


@pytest.fixture
def fs_system():
    """Provide a freshly instantiated FS system with GIC + UART."""
    from helm.boards import ArmVirt
    board = ArmVirt(mem="128MiB")
    board.instantiate()
    return board.system
```

### Running Tests

```bash
# Build the Rust extension
cd /path/to/helm-ng
maturin develop --manifest-path runtime/helm-python/Cargo.toml

# Run all Python tests
pytest runtime/helm-python/tests/python/ -v

# Run specific test file
pytest runtime/helm-python/tests/python/test_simobject.py -v

# Run with coverage
pytest runtime/helm-python/tests/python/ --cov=helm --cov-report=term-missing

# Run Rust unit tests
cargo test -p helm-python
```
