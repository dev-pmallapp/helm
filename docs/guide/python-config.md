# Python Configuration Layer

Compose platforms, cores, memory hierarchies, and devices in Python.

## Overview

The Python package `python/helm/` provides a gem5-style configuration
API. Users describe a platform, select a timing model, attach plugins,
and run simulations — all from Python.

## Core Classes

### Platform

```python
from helm import Platform, Core, MemorySystem, Cache
from helm.isa import Arm
from helm.timing import TimingModel

platform = Platform(
    name="my-system",
    isa=Arm(),
    cores=[Core(name="big", width=4, rob_size=192)],
    memory=MemorySystem(
        l1i=Cache(size="32KB", assoc=8, latency=1),
        l1d=Cache(size="32KB", assoc=8, latency=4),
        l2=Cache(size="256KB", assoc=4, latency=12),
    ),
    timing=TimingModel.ite(),
)
```

### Simulation

```python
from helm import Simulation
from helm.plugins import InsnCount, CacheSim

sim = Simulation(platform, binary="./test", mode="se")
sim.add_plugin(InsnCount())
sim.add_plugin(CacheSim(l1d_size="32KB"))
results = sim.run()

print(f"IPC: {results.ipc:.3f}")
print(f"Instructions: {results.instructions_committed}")
```

### SeSession (Direct)

```python
from helm.session import SeSession

s = SeSession("./binary", ["binary"])
s.run(1_000_000)          # warm-up
s.add_plugin(FaultDetect()) # hot-load
s.run(10_000_000)         # with plugin
s.finish()
```

### FsSession (Direct)

```python
from helm.session import FsSession

s = FsSession("Image", machine="virt",
              append="console=ttyAMA0", memory_size="256M")
s.run(100_000_000)
print(f"PC={s.pc:#x}")
```

## Timing Models

```python
TimingModel.fe()    # L0: IPC=1, fastest
TimingModel.ite()   # L1-L2: cache latencies, branch penalty
TimingModel.cae()   # L3: cycle-accurate pipeline
```

ITE accepts keyword arguments for per-class latencies:

```python
TimingModel.ite(
    int_mul_latency=3,
    fp_div_latency=15,
    load_latency=4,
    branch_penalty=10,
)
```

## Available Plugins

```python
from helm.plugins import InsnCount, ExecLog, HotBlocks, HowVec
from helm.plugins import SyscallTrace, FaultDetect, CacheSim
```

## ISA Selection

```python
from helm.isa import Arm, RiscV, X86

platform = Platform(name="arm-system", isa=Arm(), ...)
platform = Platform(name="rv-system", isa=RiscV(), ...)  # stub
platform = Platform(name="x86-system", isa=X86(), ...)   # stub
```

## Device Configuration

```python
from helm.device import Device, Bus

platform.add_device(Device(name="uart", type="pl011",
                           base=0x0900_0000))
```
