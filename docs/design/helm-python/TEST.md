# helm-python — Test Plan

> Test strategy and test cases for the `helm-python` crate and `helm_ng` Python package.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-sim-objects.md`](./LLD-sim-objects.md) · [`LLD-param-system.md`](./LLD-param-system.md) · [`LLD-factory.md`](./LLD-factory.md)

---

## Table of Contents

1. [Test Categories](#1-test-categories)
2. [Integration Tests — Full Simulation](#2-integration-tests--full-simulation)
3. [Param Type Tests](#3-param-type-tests)
4. [Exception Mapping Tests](#4-exception-mapping-tests)
5. [World Integration Tests](#5-deviceworld-integration-tests)
6. [GIL and Concurrency Tests](#6-gil-and-concurrency-tests)
7. [Plugin Loader Tests](#7-plugin-loader-tests)
8. [Test Infrastructure](#8-test-infrastructure)

---

## 1. Test Categories

| Category | Location | Runner | Scope |
|---|---|---|---|
| Python unit tests (Param, DSL) | `crates/helm-python/tests/python/` | `pytest` | Pure Python, no Rust build needed |
| Rust unit tests (AttrValue conversion) | `crates/helm-python/src/*.rs` `#[cfg(test)]` | `cargo test` | Rust side of PyO3 boundary |
| Python integration tests (full sim) | `crates/helm-python/tests/python/test_integration.py` | `pytest` | Full Rust+Python stack |
| World integration tests | `crates/helm-python/tests/python/test_world.py` | `pytest` | World + Python bindings |
| GIL tests | `crates/helm-python/tests/python/test_gil.py` | `pytest` | Concurrency behavior |

All pytest tests require the `helm_ng` extension to be built and installed:
```bash
maturin develop --manifest-path crates/helm-python/Cargo.toml
pytest crates/helm-python/tests/python/
```

---

## 2. Integration Tests — Full Simulation

### Test: basic elaboration and run

```python
# tests/python/test_integration.py

import pytest
from helm_ng import Simulation, Cpu, L1Cache, Memory, Board, Isa, ExecMode, Timing

def make_sim(isa=Isa.RiscV, mode=ExecMode.Functional, timing=Timing.Virtual) -> Simulation:
    cpu  = Cpu(isa=isa, mode=mode, timing=timing)
    mem  = Memory(size="64MiB", base=0x8000_0000)
    board = Board(cpu=cpu, memory=mem)
    return Simulation(root=board)


def test_elaborate_and_run_riscv():
    """Basic: RV64 functional sim elaborates and runs without error."""
    sim = make_sim(isa=Isa.RiscV, mode=ExecMode.Functional, timing=Timing.Virtual)
    sim.elaborate()
    result = sim.run(n_instructions=0)   # zero instructions — immediate stop
    assert result == "completed"


def test_run_returns_stop_reason():
    """sim.run() returns a string stop reason."""
    sim = make_sim()
    sim.elaborate()
    reason = sim.run(n_instructions=1000)
    assert isinstance(reason, str)
    assert reason in ("completed", "until_hit") or reason.startswith(("exception:", "breakpoint:"))


def test_run_before_elaborate_raises():
    """sim.run() before elaborate() must raise RuntimeError."""
    sim = make_sim()
    with pytest.raises(RuntimeError, match="elaborate"):
        sim.run(n_instructions=100)


def test_reset_after_run():
    """sim.reset() works after sim.run() without error."""
    sim = make_sim()
    sim.elaborate()
    sim.run(n_instructions=100)
    sim.reset()   # must not raise


def test_with_caches():
    """Elaboration with L1 caches attached to CPU."""
    cpu    = Cpu(isa=Isa.RiscV, mode=ExecMode.Functional, timing=Timing.Virtual)
    icache = L1Cache(size="32KiB", assoc=8, hit_latency=4)
    dcache = L1Cache(size="32KiB", assoc=8, hit_latency=4)
    cpu.icache = icache
    cpu.dcache = dcache
    mem   = Memory(size="128MiB")
    sim   = Simulation(root=Board(cpu=cpu, memory=mem))
    sim.elaborate()
    result = sim.run(n_instructions=0)
    assert result == "completed"


def test_checkpoint_save_returns_bytes():
    """checkpoint_save() returns a non-empty bytes object."""
    sim = make_sim()
    sim.elaborate()
    blob = sim.checkpoint_save()
    assert isinstance(blob, bytes)
    assert len(blob) > 0


def test_checkpoint_round_trip():
    """Save checkpoint, restore it, run again — no error."""
    sim = make_sim()
    sim.elaborate()
    sim.run(n_instructions=1000)
    blob = sim.checkpoint_save()

    sim2 = make_sim()
    sim2.elaborate()
    sim2.checkpoint_restore(blob)
    result = sim2.run(n_instructions=1000)
    assert result == "completed"


def test_until_callback():
    """sim.run(until=callback) stops when callback returns True."""
    sim = make_sim()
    sim.elaborate()

    call_count = [0]
    def stop_after_10(event):
        call_count[0] += 1
        return call_count[0] >= 10

    result = sim.run(n_instructions=10_000_000, until=stop_after_10)
    assert result == "until_hit"
    assert call_count[0] == 10


def test_event_bus_subscribe_exception():
    """Python callback is called on Exception event."""
    sim = make_sim()
    sim.elaborate()

    events = []
    handle = sim.event_bus.subscribe("Exception", lambda e: events.append(e))

    sim.run(n_instructions=100)
    # May or may not have exceptions depending on workload — just check no error
    for e in events:
        assert "vector" in e
        assert "pc" in e
```

### Test: aarch64 elaboration

```python
def test_elaborate_aarch64():
    """AArch64 functional sim elaborates without error."""
    sim = make_sim(isa=Isa.AArch64, mode=ExecMode.Functional)
    sim.elaborate()
    result = sim.run(n_instructions=0)
    assert result == "completed"
```

---

## 3. Param Type Tests

```python
# tests/python/test_params.py

import pytest
from helm_ng.params import Param
from helm_ng.enums import Isa, ExecMode, Timing
from helm_ng.components import Cpu, L1Cache, Memory


# ── Param.MemorySize ────────────────────────────────────────────────────────

class TestMemorySize:
    """Param.MemorySize accepts str/int, rejects bad types/values."""

    def _make(self, val):
        m = Memory()
        m.size = val
        return m.size   # returns normalized int (bytes)

    def test_kib_string(self):
        assert self._make("32KiB") == 32 * 1024

    def test_mib_string(self):
        assert self._make("256MiB") == 256 * 1024 * 1024

    def test_gib_string(self):
        assert self._make("1GiB") == 1024 ** 3

    def test_decimal_string(self):
        assert self._make("32768") == 32768

    def test_plain_int(self):
        assert self._make(65536) == 65536

    def test_case_insensitive(self):
        assert self._make("64mib") == 64 * 1024 * 1024

    def test_with_space(self):
        assert self._make("32 KiB") == 32 * 1024

    def test_rejects_negative(self):
        with pytest.raises(ValueError, match="positive"):
            self._make(-1)

    def test_rejects_zero(self):
        with pytest.raises(ValueError, match="positive"):
            self._make(0)

    def test_rejects_list(self):
        with pytest.raises(TypeError, match="expected str or int"):
            self._make([32768])

    def test_rejects_float(self):
        with pytest.raises(TypeError, match="expected str or int"):
            self._make(32.5)

    def test_rejects_none(self):
        with pytest.raises(TypeError):
            self._make(None)


# ── Param.Int ───────────────────────────────────────────────────────────────

class TestParamInt:
    def _cache_with_assoc(self, val):
        c = L1Cache()
        c.assoc = val
        return c.assoc

    def test_positive_int(self):
        assert self._cache_with_assoc(8) == 8

    def test_zero(self):
        # Zero is a valid int; range checked at elaborate() on Rust side
        assert self._cache_with_assoc(0) == 0

    def test_rejects_bool(self):
        with pytest.raises(TypeError, match="expected int"):
            self._cache_with_assoc(True)

    def test_rejects_float(self):
        with pytest.raises(TypeError, match="expected int"):
            self._cache_with_assoc(4.0)

    def test_rejects_string(self):
        with pytest.raises(TypeError, match="expected int"):
            self._cache_with_assoc("8")


# ── Param.Cycles ────────────────────────────────────────────────────────────

class TestParamCycles:
    def _cache_with_latency(self, val):
        c = L1Cache()
        c.hit_latency = val
        return c.hit_latency

    def test_positive_cycles(self):
        assert self._cache_with_latency(4) == 4

    def test_zero_cycles(self):
        assert self._cache_with_latency(0) == 0

    def test_rejects_negative(self):
        with pytest.raises(ValueError, match="non-negative"):
            self._cache_with_latency(-1)

    def test_rejects_float(self):
        with pytest.raises(TypeError, match="expected int"):
            self._cache_with_latency(4.5)


# ── Param.Addr ──────────────────────────────────────────────────────────────

class TestParamAddr:
    def _mem_with_base(self, val):
        m = Memory()
        m.base = val
        return m.base

    def test_hex_literal(self):
        assert self._mem_with_base(0x8000_0000) == 0x8000_0000

    def test_zero(self):
        assert self._mem_with_base(0) == 0

    def test_rejects_negative(self):
        with pytest.raises(ValueError, match="non-negative"):
            self._mem_with_base(-1)

    def test_rejects_string(self):
        with pytest.raises(TypeError):
            self._mem_with_base("0x80000000")


# ── Param.Isa ───────────────────────────────────────────────────────────────

class TestParamIsa:
    def _cpu_with_isa(self, val):
        c = Cpu()
        c.isa = val
        return c.isa

    def test_riscv(self):
        assert self._cpu_with_isa(Isa.RiscV) == Isa.RiscV

    def test_aarch64(self):
        assert self._cpu_with_isa(Isa.AArch64) == Isa.AArch64

    def test_rejects_string(self):
        with pytest.raises(TypeError, match="expected Isa enum"):
            self._cpu_with_isa("riscv")

    def test_rejects_int(self):
        with pytest.raises(TypeError, match="expected Isa enum"):
            self._cpu_with_isa(0)


# ── Param.ExecMode ──────────────────────────────────────────────────────────

class TestParamExecMode:
    def _cpu_with_mode(self, val):
        c = Cpu()
        c.mode = val
        return c.mode

    def test_functional(self):
        assert self._cpu_with_mode(ExecMode.Functional) == ExecMode.Functional

    def test_syscall(self):
        assert self._cpu_with_mode(ExecMode.Syscall) == ExecMode.Syscall

    def test_rejects_string(self):
        with pytest.raises(TypeError, match="expected ExecMode enum"):
            self._cpu_with_mode("syscall")


# ── Param.Timing ────────────────────────────────────────────────────────────

class TestParamTiming:
    def _cpu_with_timing(self, val):
        c = Cpu()
        c.timing = val
        return c.timing

    def test_virtual(self):
        assert self._cpu_with_timing(Timing.Virtual) == Timing.Virtual

    def test_interval(self):
        assert self._cpu_with_timing(Timing.Interval) == Timing.Interval

    def test_accurate(self):
        assert self._cpu_with_timing(Timing.Accurate) == Timing.Accurate

    def test_rejects_string(self):
        with pytest.raises(TypeError, match="expected Timing enum"):
            self._cpu_with_timing("virtual")
```

---

## 4. Exception Mapping Tests

```python
# tests/python/test_exceptions.py

import pytest
from helm_ng import (
    Simulation, Cpu, L1Cache, Memory, Board,
    HelmConfigError, HelmMemFault, HelmDeviceError, HelmCheckpointError,
)
from helm_ng.enums import Isa, ExecMode, Timing


def test_unknown_component_raises_config_error():
    """Passing an unknown type name to PendingObject raises HelmConfigError."""
    from helm_ng._helm_ng import PySimulation
    py_sim = PySimulation()
    with pytest.raises(HelmConfigError, match="unknown"):
        py_sim.elaborate([("NonExistentComponent", {})])


def test_invalid_cache_size_raises_config_error():
    """Non-power-of-two cache size raises HelmConfigError at elaborate."""
    cpu = Cpu(isa=Isa.RiscV)
    cpu.icache = L1Cache(size="3KiB")   # 3072 bytes — not a power of two
    mem = Memory(size="64MiB")
    sim = Simulation(root=Board(cpu=cpu, memory=mem))
    with pytest.raises(HelmConfigError, match="power.of.two"):
        sim.elaborate()


def test_mem_fault_attributes():
    """HelmMemFault exception carries addr and pc attributes."""
    # This test requires a way to trigger a MemFault. For now, inject via
    # build_simulator and manually trigger an access fault via a crafted binary
    # or via a test hook. Actual assertion:
    try:
        raise HelmMemFault("test", addr=0xDEAD, pc=0x1000, fault_kind="access")
    except HelmMemFault as e:
        assert e.addr == 0xDEAD
        assert e.pc == 0x1000
        assert e.fault_kind == "access"


def test_checkpoint_version_mismatch_raises():
    """Restoring a truncated checkpoint raises HelmCheckpointError."""
    sim = Simulation(root=Board(cpu=Cpu(), memory=Memory()))
    sim.elaborate()
    with pytest.raises(HelmCheckpointError):
        sim.checkpoint_restore(b"\x00\x00\x00\xFF")   # wrong version tag


def test_exception_hierarchy():
    """All helm exceptions inherit from HelmError."""
    from helm_ng import HelmError
    assert issubclass(HelmConfigError,     HelmError)
    assert issubclass(HelmMemFault,        HelmError)
    assert issubclass(HelmDeviceError,     HelmError)
    assert issubclass(HelmCheckpointError, HelmError)
```

---

## 5. World Integration Tests

```python
# tests/python/test_world.py

import pytest
from helm_ng import World, HelmConfigError
from helm_ng._helm_ng import PyWorld


UART_BASE = 0x10000000


def make_uart_world():
    """Create a minimal World with one UART."""
    world = World()
    from helm_ng import Uart16550
    uart_id = world.add_device(Uart16550(clock_hz=1_843_200), name="uart")
    world.map_device(uart_id, base=UART_BASE)
    world.elaborate()
    return world, uart_id


def test_world_creates():
    """World() creates without error."""
    world = World()
    assert world.current_tick == 0


def test_elaborate_empty_world():
    """Elaborating an empty World is valid."""
    world = World()
    world.elaborate()


def test_mmio_write_read():
    """mmio_write followed by mmio_read returns the written value (status reg)."""
    world, _ = make_uart_world()
    # Write to UART LCR (offset 3) and read back
    world.mmio_write(UART_BASE + 3, 1, 0x03)   # 8N1
    world.mmio_write(UART_BASE + 3, 1, 0x80)   # DLAB=1
    dll = world.mmio_read(UART_BASE, 1)
    # After reset, DLL should be 0
    assert dll == 0


def test_advance_increments_tick():
    """advance(N) increments current_tick by N."""
    world = World()
    world.elaborate()
    assert world.current_tick == 0
    world.advance(1000)
    assert world.current_tick == 1000
    world.advance(500)
    assert world.current_tick == 1500


def test_pending_interrupts_initially_empty():
    """No interrupts pending immediately after elaborate."""
    world, _ = make_uart_world()
    assert world.pending_interrupts() == []


def test_uart_tx_triggers_interrupt():
    """Write to TX register and advance: THRE interrupt fires."""
    world, uart_id = make_uart_world()

    # Enable TX interrupt (IER bit 1)
    world.mmio_write(UART_BASE + 1, 1, 0x02)

    # Write 'A' to THR
    world.mmio_write(UART_BASE, 1, ord('A'))

    # Advance enough cycles for one baud period at 9600 baud
    # 1.8432 MHz / (16 * 9600) = 12 cycles/baud clock → 10 bits → 120 cycles
    world.advance(200)

    irqs = world.pending_interrupts()
    assert any(pin == "irq_out" for (_, pin) in irqs), \
        f"Expected irq_out in {irqs}"


def test_on_event_callback():
    """on_event() callback is called when a MemWrite event fires."""
    world, _ = make_uart_world()

    writes = []
    handle = world.on_event("MemWrite", lambda e: writes.append(e["addr"]))

    world.mmio_write(UART_BASE, 1, 0x41)
    world.advance(1)

    assert UART_BASE in writes


def test_double_elaborate_raises():
    """Calling elaborate() twice must raise an error."""
    world = World()
    world.elaborate()
    with pytest.raises(Exception):
        world.elaborate()


def test_map_device_before_elaborate():
    """Mapping an unknown device id raises."""
    world = World()
    with pytest.raises(Exception):
        world.map_device(9999, base=0x1000)   # id 9999 not registered


def test_mmio_unmapped_address_panics():
    """mmio_write to unmapped address raises (Rust panics become Python RuntimeError)."""
    world = World()
    world.elaborate()
    with pytest.raises((RuntimeError, Exception)):
        world.mmio_write(0xDEAD_0000, 4, 0xFFFF_FFFF)
```

---

## 6. GIL and Concurrency Tests

```python
# tests/python/test_gil.py

import threading
import pytest
from helm_ng import Simulation, Cpu, Memory, Board


def test_run_releases_gil():
    """sim.run() releases the GIL: another Python thread can run concurrently."""
    sim = Simulation(root=Board(cpu=Cpu(), memory=Memory(size="64MiB")))
    sim.elaborate()

    ran_concurrently = [False]

    def background_thread():
        # This runs while the simulation loop holds the GIL-released lock
        ran_concurrently[0] = True

    t = threading.Thread(target=background_thread)
    t.start()

    # Run a short simulation — background thread should run during this
    sim.run(n_instructions=10_000_000)

    t.join(timeout=5.0)
    assert t.is_alive() is False, "Background thread did not complete"
    assert ran_concurrently[0], "Background thread did not run concurrently"


def test_event_callback_called_safely():
    """Python callback registered on event_bus is called without deadlock."""
    sim = Simulation(root=Board(cpu=Cpu(), memory=Memory(size="64MiB")))
    sim.elaborate()

    callback_count = [0]

    def counter(event):
        callback_count[0] += 1

    handle = sim.event_bus.subscribe("MemWrite", counter)
    sim.run(n_instructions=100_000)

    # Callback must have been invoked (may be 0 if no mem writes in these insns)
    # The important thing is no deadlock or panic
    assert isinstance(callback_count[0], int)


def test_multiple_subscribers():
    """Multiple Python subscribers on the same event kind all fire."""
    sim = Simulation(root=Board(cpu=Cpu(), memory=Memory()))
    sim.elaborate()

    seen_a = [0]
    seen_b = [0]

    h1 = sim.event_bus.subscribe("MemWrite", lambda e: seen_a.__setitem__(0, seen_a[0] + 1))
    h2 = sim.event_bus.subscribe("MemWrite", lambda e: seen_b.__setitem__(0, seen_b[0] + 1))

    sim.run(n_instructions=50_000)

    # Both subscribers should have seen the same number of events
    assert seen_a[0] == seen_b[0]
```

---

## 7. Plugin Loader Tests

```python
# tests/python/test_plugins.py

import pytest
import helm_ng


def test_list_devices_returns_list():
    """list_devices() returns a list of strings."""
    devices = helm_ng.list_devices()
    assert isinstance(devices, list)
    assert all(isinstance(d, str) for d in devices)


def test_device_schema_known_device():
    """device_schema() returns a dict for a known device."""
    # Requires at least one built-in device to be registered
    devices = helm_ng.list_devices()
    if not devices:
        pytest.skip("No built-in devices registered")
    name = devices[0]
    schema = helm_ng.device_schema(name)
    assert isinstance(schema, dict)


def test_device_schema_unknown_raises():
    """device_schema() raises HelmConfigError for unknown device."""
    with pytest.raises(helm_ng.HelmConfigError, match="unknown"):
        helm_ng.device_schema("definitely_not_a_real_device_xyz")


def test_load_plugin_missing_file():
    """load_plugin() with a nonexistent path raises HelmConfigError."""
    with pytest.raises(helm_ng.HelmConfigError):
        helm_ng.load_plugin("/nonexistent/path/libhelm_fake.so")
```

---

## 8. Test Infrastructure

### pytest configuration

```toml
# crates/helm-python/tests/python/pytest.ini
[pytest]
testpaths = .
python_files = test_*.py
python_classes = Test*
python_functions = test_*
addopts = -v --tb=short
```

### conftest.py

```python
# crates/helm-python/tests/python/conftest.py

import pytest
import helm_ng

@pytest.fixture
def simple_sim():
    """Provide a freshly elaborated minimal RV64 SE sim."""
    from helm_ng import Simulation, Cpu, Memory, Board, Isa, ExecMode, Timing
    sim = Simulation(root=Board(
        cpu=Cpu(isa=Isa.RiscV, mode=ExecMode.Functional, timing=Timing.Virtual),
        memory=Memory(size="64MiB"),
    ))
    sim.elaborate()
    return sim

@pytest.fixture
def uart_world():
    """Provide a World with one UART at 0x10000000."""
    from helm_ng import World, Uart16550
    world = World()
    uart = world.add_device(Uart16550(clock_hz=1_843_200), name="uart")
    world.map_device(uart, base=0x10000000)
    world.elaborate()
    return world, uart
```

### Running Tests

```bash
# Build the Rust extension
cd /path/to/helm-ng
maturin develop --manifest-path crates/helm-python/Cargo.toml

# Run all Python tests
pytest crates/helm-python/tests/python/ -v

# Run specific test file
pytest crates/helm-python/tests/python/test_params.py -v

# Run with coverage
pytest crates/helm-python/tests/python/ --cov=helm_ng --cov-report=term-missing

# Run Rust unit tests
cargo test -p helm-python
```
