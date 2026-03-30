# SimObject Lifecycle

How simulation components are constructed, wired, and managed through
the simulation lifecycle.

## Lifecycle Phases

Every simulation component follows a strict lifecycle:

```text
CONSTRUCT → init() → elaborate(system) → startup() → RUN
                                                       │
                                           ┌───────────┤
                                           ▼           ▼
                                        reset()    checkpoint_save()
                                           │        checkpoint_restore()
                                           ▼
                                         RUN (resumed)
```

### Phase Details

| Phase | Purpose | Cross-Component Access | When |
|-------|---------|----------------------|------|
| `CONSTRUCT` | Allocate, set parameters | No | Object creation |
| `init()` | Internal state setup | No | After all objects created |
| `elaborate(system)` | Wire cross-component refs | Yes | After all `init()` calls |
| `startup()` | Schedule initial events, assert signals | Yes | After all `elaborate()` calls |
| `RUN` | Normal simulation | Yes | Simulation loop |
| `reset()` | Return to post-startup state | Yes | On reset request |
| `checkpoint_save()` | Serialize architectural state | Read-only | On checkpoint |
| `checkpoint_restore()` | Deserialize architectural state | Write | On restore |

### Critical Rules

1. **`init()` is self-contained** — no cross-component access. Only
   set up internal data structures. This ensures objects can be
   initialized in any order.

2. **`elaborate(system)` is for wiring** — register MMIO regions in
   `AddressMap`, store `Arc` refs to other components (e.g., interrupt
   controller), wire interrupt pins. All `Arc` refs are stored here,
   not during `RUN`.

3. **No dynamic lookup in the hot loop** — every cross-component
   reference is resolved during `elaborate()` and stored as a direct
   pointer. Zero hash lookups, zero string comparisons per instruction.

4. **`startup()` runs after wiring** — schedule initial timer events,
   assert initial signal levels. Unlike `init()`, this can access
   other components.

5. **`reset()` is idempotent** — calling it multiple times produces
   the same state as calling it once. Returns to post-`startup()`
   state.

## Checkpoint Protocol

### What is Saved

- Architectural state (registers, PC, PSTATE, system registers)
- Memory contents (RAM regions)
- Device register values (MMIO state)
- EventQueue entries (scheduled future events)

### What is NOT Saved

- Performance counters (`PerfCounter`, `PerfHistogram`)
- `HelmEventBus` subscribers (re-register on restore)
- JIT cache (recompiled on demand)
- Host file descriptors (reopened on restore)

### Save/Restore Flow

```text
checkpoint_save():
  1. Pause simulation
  2. For each component: serialize state via AttrRegistry
  3. Serialize EventQueue
  4. Write checkpoint blob

checkpoint_restore():
  1. For each component: deserialize state
  2. Restore EventQueue
  3. HelmEventBus subscribers re-register
  4. Resume simulation
```

## No Dark State

**Inviolable rule:** every persistent field in a simulation component
must be a registered `AttrDescriptor`. This ensures:

- Checkpointing captures all state
- Debuggers can inspect all state
- No hidden state creates non-determinism

Fields that are derived or cached (like TLB entries, JIT blocks) are
explicitly excluded from checkpointing and reconstructed on restore.

## Python SimObject Hierarchy

In the PyO3 layer (`helm-python`), the lifecycle maps to Python
operations:

```python
# CONSTRUCT — create objects
system = System(isa="aarch64", timing="virtual")
system.cpu = Cpu(model="cortex-a55")
system.ram = Ram(size="256M")

# init() + elaborate() — called internally by:
system.instantiate()

# startup() + RUN — called by:
system.run(quantum=1000000)
```

`SimObject` is a `#[pyclass(subclass)]` base class. Children are
attached via `__setattr__` (tracked internally). The `instantiate()`
method drives `init()` → `elaborate()` → `startup()` for the entire
object tree.

### SimObject States

| State | Description |
|-------|-------------|
| `Pending` | Created but not instantiated; parameters can be changed |
| `Instantiated` | `instantiate()` called; parameters frozen |

After `Instantiated`, configuration is frozen. This enforces the
**"Python describes; Rust simulates"** principle — no mutation during
simulation.

## Comparison

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Lifecycle | `realize()` + `reset()` | `init()` + `startup()` | `init()` + phase callbacks | `init()` → `elaborate()` → `startup()` |
| Config freeze | At `realize()` | At `simulate()` | At `continue` | At `instantiate()` |
| Cross-component refs | QOM links (runtime lookup) | Port binding | Interface queries | `elaborate()` stores `Arc` refs |
| Checkpoint | Migration framework | Serialize/Drain | First-class | `checkpoint_save/restore` via AttrRegistry |
| Dark state | Common (missed QOM properties) | Common | Rare (attribute system) | Forbidden (all fields registered) |
