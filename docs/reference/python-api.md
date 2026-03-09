# Python API

Reference for the `python/helm/` configuration package.

## helm.session.SeSession

```python
SeSession(binary, argv=None, envp=None)
```

| Method | Returns | Description |
|--------|---------|-------------|
| `run(max_insns)` | `StopResult` | Execute up to N instructions |
| `run_until_pc(target, max_insns)` | `StopResult` | Run until PC matches |
| `run_until_insns(total)` | `StopResult` | Run until total insns reached |
| `add_plugin(plugin)` | — | Hot-load a plugin |
| `finish()` | — | Call atexit on all plugins |

| Property | Type | Description |
|----------|------|-------------|
| `pc` | `int` | Current program counter |
| `insn_count` | `int` | Total instructions executed |
| `virtual_cycles` | `int` | Virtual cycle count |
| `has_exited` | `bool` | Whether guest has exited |
| `exit_code` | `int` | Guest exit code |

## helm.session.FsSession

```python
FsSession(kernel, machine="virt", append="", memory_size="256M",
          serial="stdio", timing="fe", backend="jit",
          dtb=None, sysmap=None)
```

| Method | Returns | Description |
|--------|---------|-------------|
| `run(max_insns)` | `StopResult` | Execute up to N instructions |
| `run_until_symbol(sym)` | `StopResult` | Run until named symbol |
| `run_until_pc(target)` | `StopResult` | Run until PC matches |
| `xn(n)` | `int` | Read register Xn |
| `regs()` | `dict` | All registers |
| `sysreg(name)` | `int` | Read named system register |
| `read_memory(addr, size)` | `bytes` | Physical memory read |
| `read_virtual(va, size)` | `bytes` | Virtual memory read |
| `stats()` | `dict` | Session statistics |

| Property | Type | Description |
|----------|------|-------------|
| `pc` | `int` | Current PC |
| `sp` | `int` | Stack pointer |
| `insn_count` | `int` | Instructions executed |
| `virtual_cycles` | `int` | Virtual cycles |
| `current_el` | `int` | Current exception level |
| `daif` | `int` | Interrupt mask |
| `irq_count` | `int` | IRQs delivered |

## helm.session.StopReason

Enum: `INSN_LIMIT`, `BREAKPOINT`, `EXITED`, `ERROR`.

## helm.platform.Platform

```python
Platform(name, isa, cores, memory, devices=None, timing=None)
```

## helm.core.Core

```python
Core(name="core", width=4, rob_size=128, iq_size=64,
     lq_size=32, sq_size=32, branch_predictor=None)
```

## helm.memory.Cache / MemorySystem

```python
Cache(size="32KB", assoc=8, latency=4, line_size=64)
MemorySystem(l1i=Cache(...), l1d=Cache(...), l2=Cache(...),
             l3=None, dram_latency=200)
```

## helm.timing.TimingModel

```python
TimingModel.fe()
TimingModel.ite(**kwargs)
TimingModel.cae()
```

## helm.simulation.Simulation

```python
Simulation(platform, binary, mode="se", max_cycles=100_000_000)
sim.add_plugin(plugin)
results = sim.run()
```
