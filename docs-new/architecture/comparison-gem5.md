# Comparison with gem5

Architectural parallels and differences between HELM and gem5.

## Object Model

| Aspect | gem5 | HELM |
|--------|------|------|
| Base class | `SimObject` (C++ + Python) | `HelmObject` trait (Rust) |
| Properties | `Param` descriptors in Python | `Property` with `PropertyType` enum |
| Composition | Python object graph | `ObjectTree` + `Platform` builder |
| Serialisation | `serialize()` / `unserialize()` | `checkpoint()` / `restore()` (JSON) |

## Port System vs Bus

gem5 uses typed ports (`RequestPort`, `ResponsePort`) with
`recvTimingReq` / `sendTimingResp`. HELM uses `DeviceBus` with
`Transaction` objects that flow through a bus tree, accumulating
stall cycles at each bridge.

| Aspect | gem5 | HELM |
|--------|------|------|
| Topology | Port connections (point-to-point) | Bus tree (hierarchical) |
| Latency | Crossbar / bridge latency | `bridge_latency` per bus level |
| Address routing | `AddrRangeMap` | `DeviceBus::dispatch()` |

## Timing Models

| gem5 | HELM | Description |
|------|------|-------------|
| `AtomicSimpleCPU` | `FeModel` | IPC = 1, no pipeline |
| `TimingSimpleCPU` | `ApeModel` | Cache-level latencies |
| `MinorCPU` | `ApeModelDetailed` | In-order pipeline approximation |
| `O3CPU` | `Pipeline` (CAE) | Full OoO with ROB, rename, IQ |

## Python Configuration

| Aspect | gem5 | HELM |
|--------|------|------|
| Config files | `configs/example/se.py`, `fs.py` | `python/helm/configs/se.py`, `fs.py` |
| Platform class | `System` + `Board` | `Platform` |
| Core class | `BaseCPU` subclasses | `Core` |
| Cache | `Cache` SimObject with `L1Cache` params | `Cache` + `MemorySystem` |
| Branch predictor | `BranchPredictor` SimObject | `BranchPredictor` (Static/Bimodal/GShare/TAGE/Tournament) |

## Memory System

| Aspect | gem5 | HELM |
|--------|------|------|
| Cache class | `BaseCache` with MSHR, write buffer | `Cache` (set-associative, LRU stub) |
| Coherence | MOESI_CMP_directory, MESI_Two_Level (Ruby) | `CoherenceController` (MOESI stub) |
| TLB | `ArmTLB` with full walk | `Tlb` (ASID-tagged) + `mmu::walk()` |
| Address space | `AddrRange` + port system | `AddressSpace` + `MemRegion` |

## Execution

gem5 uses interpretation for all ISA execution (no JIT). HELM provides
both a direct interpreter (`Aarch64Cpu::step()`) and a JIT path
(`helm-tcg` + Cranelift). gem5's `StaticInst` maps to HELM's `MicroOp`.
