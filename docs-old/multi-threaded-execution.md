# Multi-Threaded Execution Model

Based on QEMU's Multi-Threaded TCG (MTTCG) design.
Reference: <https://www.qemu.org/docs/master/devel/multi-thread-tcg.html>

## Design Principles (from QEMU MTTCG)

1. **One host thread per vCPU** — no work-stealing, no green threads.
2. **Translation cache is shared** — translated blocks are visible to
   all vCPU threads after insertion.
3. **Memory ordering uses host atomics** — guest atomic ops map to
   host atomic ops, not software locks.
4. **MMIO and device access serialises through a global mutex**
   (QEMU's BQL / Big QEMU Lock) — HELM uses per-device mutexes
   instead for better parallelism.
5. **Interrupt delivery is asynchronous** — one thread signals another
   via an atomic flag; the target checks it at the next safe point.

## Thread Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Host Process                                                │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │  vCPU-0      │  │  vCPU-1      │  │  vCPU-N      │       │
│  │  (OS thread) │  │  (OS thread) │  │  (OS thread) │       │
│  │              │  │              │  │              │       │
│  │  Regs        │  │  Regs        │  │  Regs        │       │
│  │  TcgContext  │  │  TcgContext  │  │  TcgContext  │       │
│  │  SoftTLB     │  │  SoftTLB     │  │  SoftTLB     │       │
│  │  LocalStats  │  │  LocalStats  │  │  LocalStats  │       │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘       │
│         │                 │                 │               │
│         └────────┬────────┴────────┬────────┘               │
│                  ▼                 ▼                         │
│  ┌───────────────────────────────────────────────────┐      │
│  │  Shared State                                     │      │
│  │                                                   │      │
│  │  DecodeTree          (Arc, immutable)              │      │
│  │  TranslationCache    (DashMap, lock-free read)     │      │
│  │  GuestMemory         (mmap'd, host-atomic access)  │      │
│  │  DeviceBus           (Mutex per slot)              │      │
│  │  IrqController       (AtomicU64 per line-group)   │      │
│  └───────────────────────────────────────────────────┘      │
│                  │                                           │
│  ┌───────────────▼───────────────────────────────────┐      │
│  │  TemporalDecoupler                                │      │
│  │  (quantum-based barrier, configurable)            │      │
│  └───────────────────────────────────────────────────┘      │
└──────────────────────────────────────────────────────────────┘
```

## Per-vCPU State (thread-local, no sharing)

| Resource | Purpose |
|----------|---------|
| `Aarch64Regs` / `Aarch32Regs` | Architectural register file |
| `TcgContext` | TCG op buffer for current translation block |
| `Vec<MicroOp>` | Static-decode buffer (APE/CAE mode) |
| `SoftTLB` | Per-CPU TLB cache (invalidated on TLB flush) |
| Pipeline state | ROB, rename, scheduler (APE/CAE only) |
| `StatsCollector` | Per-CPU counters, merged at end |
| `exit_request: AtomicBool` | Checked at block boundaries |

## Shared State

### Translation Cache (lock-free reads)

QEMU's `tb_htable` is a hash table of translated blocks.  HELM uses
a concurrent hash map (`DashMap` or equivalent):

```rust
struct SharedTranslationCache {
    // Key: guest PC | flags (CS base, etc.)
    blocks: DashMap<u64, Arc<TcgBlock>>,
}
```

**Read path** (hot — every block dispatch):
- Lock-free `get()`, clone the `Arc`.
- No contention, no CAS, just atomic load of the bucket pointer.

**Write path** (rare — first encounter of a new block):
- Sharded lock in `DashMap` — only one shard is locked.
- If two vCPUs translate the same block, one wins, the other's
  duplicate is dropped.

**Invalidation** (code modification, rare):
- Remove entry, bump a generation counter.
- vCPUs check the counter at block boundaries and flush their
  local TB jump cache.

### Guest Memory (host-mapped)

Like QEMU, guest RAM is `mmap`'d into the host process.  Guest
loads/stores map directly to host loads/stores through the SoftTLB:

```
Guest LDR X0, [X1]
  → SoftTLB lookup(guest_vaddr)
  → if hit: host_ptr = tlb_entry.host_base + offset
            X0 = *host_ptr  (direct host load — uses host memory ordering)
  → if miss: full page-table walk, fill TLB, retry
```

For guest atomic operations (`LDXR`/`STXR`, LSE `SWP`/`LDADD`/...):
- Map to host atomic instructions (`compare_exchange`, `fetch_add`).
- This gives sequential consistency per-location on x86 hosts and
  requires explicit barriers on ARM hosts (matching QEMU's approach).

### Device Access (per-device mutex)

QEMU serialises all device access through the Big QEMU Lock (BQL).
HELM improves on this with per-device mutexes:

```rust
struct DeviceSlot {
    device: Mutex<Box<dyn MemoryMappedDevice>>,
    base: Addr,
    size: u64,
}
```

A vCPU thread hitting an MMIO address:
1. Looks up the `DeviceSlot` in the `DeviceBus` (read-only scan).
2. Locks that specific device's `Mutex`.
3. Calls `device.read/write()`.
4. Unlocks.

Other vCPUs accessing different devices proceed in parallel.

### Interrupt Delivery (async, atomic)

Like QEMU's `cpu->interrupt_request`:

```rust
struct VcpuState {
    interrupt_pending: AtomicBool,
    // ...
}
```

Thread A wants to interrupt vCPU-2:
1. `vcpu[2].interrupt_pending.store(true, Ordering::Release)`
2. (Optional) `pthread_kill` to wake vCPU-2 if it's in a host sleep.

vCPU-2 checks `interrupt_pending` at every translated-block boundary
(the exit path from the TCG execution loop).

## Synchronisation Points

### Quantum Barrier (configurable)

For timing accuracy (APE/CAE modes), vCPUs synchronise periodically:

| Quantum | Use case | Cost |
|---------|----------|------|
| None (free-running) | FE/SE functional-only | zero |
| 100K cycles | APE approximate | ~1% |
| 10K cycles | APE detailed | ~5% |
| 1K cycles | CAE cycle-accurate | ~20% |
| 1 cycle | CAE lockstep | ~80% |

In FE/SE mode with no timing, vCPUs run free (no quantum barrier).
This matches QEMU's default MTTCG behaviour.

### Mandatory Sync Points

Regardless of quantum setting:
1. **TLB flush** (TLBI instructions) — all vCPUs flush SoftTLB.
2. **Code modification** (self-modifying code) — invalidate TB cache.
3. **IPI delivery** — wake target vCPU.
4. **Halt/WFI** — vCPU parks on a condvar until interrupted.

## Memory Consistency Model

### FE / SE (no timing)

Guest memory is accessed via host atomics.  The host memory model
provides the ordering:
- x86 host: TSO (strong) — most ARM code "just works".
- ARM host: relaxed — guest `DMB`/`DSB` map to host `dmb`/`dsb`.

This matches QEMU MTTCG.

### APE (approximate timing)

Same as FE plus:
- Guest barriers (`DMB`, `DSB`) force a quantum sync.
- Atomic operations (`LDXR`/`STXR`, LSE) map to host atomics and
  inject stall cycles into the timing model.

### CAE (cycle-accurate)

Full directory-based MOESI coherence:
- Every shared-line write sends an invalidation message.
- Read misses to shared lines trigger data forwarding.
- Quantum is set to 1 for lockstep (or N for approximate multi-core).

This is the most expensive mode and is typically used only for
cache-coherence research.

## Scaling

| Guest vCPUs | Host threads | Expected throughput vs 1-vCPU |
|-------------|-------------|-------------------------------|
| 1 | 1 | 1.0x |
| 2 | 2 | ~1.9x |
| 4 | 4 | ~3.5x |
| 8 | 8 | ~5.5x |
| 16 | 8 | ~5.5x (time-sliced) |

Bottlenecks:
- Translation cache insert contention (first run only).
- Device MMIO serialisation (per-device, not global).
- Quantum barrier overhead (proportional to 1/quantum_size).
- Coherence traffic (CAE mode only).
