# Multi-Threaded Execution Model

How HELM executes multi-core simulations across host threads.

## Overview

```
┌──────────────────────────────────────────────────────────────┐
│  Host Process                                                │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │  Core-0 Thread│  │  Core-1 Thread│  │  Core-N Thread│      │
│  │              │  │              │  │              │       │
│  │  Regs        │  │  Regs        │  │  Regs        │       │
│  │  TcgContext  │  │  TcgContext  │  │  TcgContext  │       │
│  │  MicroOp buf │  │  MicroOp buf │  │  MicroOp buf │       │
│  │  TLB (local) │  │  TLB (local) │  │  TLB (local) │       │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘       │
│         │                 │                 │               │
│         └────────┬────────┴────────┬────────┘               │
│                  │                 │                         │
│         ┌────────▼─────────────────▼────────┐               │
│         │        Shared (read-mostly)        │               │
│         │                                    │               │
│         │  DecodeTree (immutable)             │               │
│         │  TranslationCache (lock-free read)  │               │
│         │  AddressSpace (CoW pages)           │               │
│         │  DeviceBus (mutex per device)       │               │
│         │  IrqController (atomic lines)       │               │
│         └────────────────────────────────────┘               │
│                  │                                           │
│         ┌────────▼────────────────┐                          │
│         │  TemporalDecoupler     │                          │
│         │  (quantum barrier)     │                          │
│         └────────────────────────┘                           │
└──────────────────────────────────────────────────────────────┘
```

## Thread Ownership

### Per-Core (thread-local, no sharing)

| Resource | Notes |
|----------|-------|
| `Aarch64Regs` | Architectural state |
| `TcgContext` | TCG op buffer for current block |
| `MicroOp` buffer | Static-decode output for current insn |
| Per-core TLB | Software TLB cache |
| Pipeline state | ROB, rename, scheduler (APE/CAE only) |
| StatsCollector | Per-core counters |
| Local `virtual_time` | Core's own cycle counter |

### Shared (read-mostly, low contention)

| Resource | Sync mechanism | Notes |
|----------|---------------|-------|
| `DecodeTree` | None (immutable) | Built once at startup, `Arc<DecodeTree>` |
| `TranslationCache` | `DashMap` or lock-free | Block lookup by PC; insert is rare |
| `AddressSpace` pages | `RwLock` per page or CoW | Reads dominate; writes rare |
| Coherence directory | Per-line spinlock | Only for CAE mode |

### Shared (contended)

| Resource | Sync mechanism | Notes |
|----------|---------------|-------|
| `DeviceBus` | `Mutex` per device slot | MMIO is infrequent |
| `IrqController` | `AtomicU64` line bitmap | Lock-free assert/deassert |
| SystemC bridge | Channel (mpsc) | Transactions queued |

## Quantum-Based Synchronisation

Cores run independently within a quantum window, then synchronise:

```
Core 0:  ════════quantum════════╦═══════quantum═══════╦═══
Core 1:  ════════quantum════════╬═══════quantum═══════╬═══
Core 2:  ════════quantum════════╬═══════quantum═══════╬═══
                                ▲ barrier             ▲ barrier
```

Within a quantum:
- Each core fetches, decodes, and executes without coordinating.
- Memory accesses hit the local TLB; misses go to the shared
  `AddressSpace` under a read lock.
- Device MMIO forces an early sync (the core yields its quantum).

At the barrier:
- All cores reach the same virtual time.
- Cross-core events are delivered (IPIs, coherence invalidations).
- Global `virtual_time` advances.

Quantum size is configurable:

| Quantum | Syncs/sec (@ 1 GHz) | Overhead | Timing error |
|---------|---------------------|----------|--------------|
| 1,000 cycles | 1 M | ~50% | ±0.5 us |
| 10,000 cycles | 100 K | ~5% | ±5 us |
| 100,000 cycles | 10 K | <1% | ±50 us |

Default: 10,000 cycles.

## Translation Cache Sharing

The `TranslationCache` maps guest PC → translated block.  Because
AArch64 instructions are position-independent within a block (no
PC-relative data embedded in the block itself), translated blocks
can be shared across cores.

```rust
struct SharedTranslationCache {
    // DashMap: concurrent hashmap, lock-free reads, sharded writes.
    blocks: DashMap<Addr, Arc<TcgBlock>>,
}
```

**Insert path** (rare — only on first encounter of a block):
1. Core translates a block into `TcgBlock`.
2. Core calls `blocks.entry(pc).or_insert(Arc::new(block))`.
3. If another core already inserted, the duplicate is dropped.

**Lookup path** (hot — every block dispatch):
1. `blocks.get(&pc)` — lock-free read.
2. If hit, clone the `Arc` (cheap).
3. If miss, translate and insert.

## Memory Consistency

### FE / SE Mode

No memory ordering is modelled.  Each core sees its own sequentially
consistent view.  Cross-core races are not detected.

### APE Mode

`DeviceBus` accesses and explicit barriers (`DMB`, `DSB`) force a
quantum sync.  Ordinary loads/stores are not synchronised.

### CAE Mode

Full coherence protocol (MOESI directory).  Every shared-line write
sends an invalidation to the directory, which broadcasts to sharers.
This is expensive but necessary for accurate multi-core studies.

## Thread Pool vs Thread-per-Core

HELM uses **thread-per-core** (not a work-stealing pool):

- Each simulated core gets one OS thread.
- Threads are pinned to host cores for cache locality.
- The quantum barrier is a standard `std::sync::Barrier`.

This is simpler than a pool and avoids context-switch overhead.
For simulations with more guest cores than host cores, cores are
multiplexed round-robin within the quantum.

## Scaling Expectations

| Guest cores | Host cores | Expected scaling |
|-------------|-----------|-----------------|
| 1 | 1 | 1.0x (baseline) |
| 4 | 4 | ~3.5x |
| 8 | 8 | ~6x |
| 16 | 8 | ~6x (multiplexed) |

Scaling is limited by:
- Barrier synchronisation overhead.
- Shared `AddressSpace` contention (reads are cheap, writes are not).
- Device MMIO serialisation.
- Translation cache insert contention (first-run only).
