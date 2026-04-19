# Plan: Helm Observability And Debug Roadmap

> **Status:** Design pass started — 2026-04-19
> **Goal:** Turn Helm's current probe, spy, report, and debug pieces into a coherent first-class observability and control stack, with Python as the primary user-facing control plane and Rust as the implementation layer
> **Primary references:** QEMU TCG plugins / gdbstub / replay, gem5 probes / stats / checkpoints, Simics instrumentation / breakpoints / checkpoints
> **Completion gate:** Helm has a default observability path, a coherent debug control plane, and a clear separation between hot-path collection and cold-path delivery/control

---

## Quick Start: The First 3 Tasks

These are the three highest-leverage tasks and should be treated as the initial execution order for this roadmap.

| Priority | Task | Outcome |
|---|---|---|
| P0 | Finish `probe -> spy -> report` as the primary instrumentation path | New analysis work stops landing in legacy plugin shims |
| P1 | Add scoreboard-backed filtered observability | Hot-path counters and selective tracing become cheap enough to leave on by default in focused workflows |
| P2 | Integrate debug control with checkpoints and replay | Breakpoints, watchpoints, save/restore, and replay become one coherent debugging workflow |

---

## Current State

Helm already has the right major layers, but they are not yet fully connected.

| Layer | Current state |
|---|---|
| `framework/helm-probe` | Primary low-overhead typed observation layer; `Probe<T>` and probe bundles already exist |
| `debug/helm-spy` | Session and analysis primitive layer exists, but its own HLD says the `ProbePluginBridge` is not yet built |
| `debug/helm-report` | Formatter / sink layer exists, but is not yet the default public workflow |
| `runtime/helm-python` | Public API already nudges users from legacy plugin helpers toward `observe()` / `HelmSpy`; this is the intended monitor/control surface |
| `runtime/helm-debug` | Breakpoint, watchpoint, checkpoint, and GDB pieces exist or are designed, but are not yet presented as one integrated control plane |
| `helm-plugin` | Explicitly deprecated compatibility layer; should not be the long-term place for new observability work |

The repo therefore does not have a "missing architecture" problem. It has two
specific gaps:

1. finish wiring the intended path
2. keep that path explicitly Python-first at the interface layer and Rust-first at the implementation layer

---

## Design Principles

This roadmap should preserve the direction already visible in the codebase:

1. Hot-path event production stays in `helm-probe` and remains cheap when inactive.
2. Session aggregation and analysis live in `helm-spy`, not in the engine.
3. Formatting and delivery live in `helm-report`, not in the collector.
4. Debugger control is a separate concern from observability collection.
5. Legacy plugin APIs remain compatibility shims only.
6. All new work should prefer typed events, reusable filters, and explicit lifecycle boundaries.
7. Python is the default control plane; Rust implements the behavior behind it.
8. Do not create a separate monitor shell by default when `HelmSystem` / `HelmSpy` can expose the same capability programmatically.

---

## Task 1: Finish `probe -> spy -> report`

### Goal

Make Helm's intended instrumentation path complete and default:

```text
engine / arch / device events
    -> helm-probe
    -> helm-spy
    -> helm-report
    -> Python control surface / files / tcp / stderr / snapshots
```

### Why this is first

Without this, every new observability feature risks landing in the wrong place:

- engine-local ad hoc counters
- deprecated plugin shims
- Python-only glue
- report formatting mixed into collection logic

This task turns the current architecture from "promising pieces" into a
stable product surface.

### Immediate work

1. Build the missing `ProbePluginBridge` or equivalent direct subscription layer in `debug/helm-spy`.
2. Give `HelmSpy` a first-class attach/subscribe lifecycle to `CpuProbes`, JIT probes, and IRQ/GIC probes.
3. Standardize the snapshot model passed to `helm-report`.
4. Make `runtime/helm-python`'s `observe()` path the canonical public API, not just the preferred one in docstrings.
5. Keep `add_plugin()` and `trace_after()` only as compatibility wrappers around the new path where practical.

### Primary landing spots

- `framework/helm-probe/src/lib.rs`
- `debug/helm-spy/src/bridge.rs`
- `debug/helm-spy/src/session.rs`
- `debug/helm-report/src/*`
- `runtime/helm-python/src/system.rs`
- `runtime/helm-engine/src/lib.rs`
- `runtime/helm-engine/src/fs.rs`

### Acceptance gate

- A new analysis feature can be implemented without touching `helm-plugin`.
- Python users can attach an observation session through one obvious API.
- The same collected session can be rendered to at least text, JSON, and file/tcp sinks through `helm-report`.
- Existing legacy helper flows still work or fail with a migration path that points to `observe()`.

---

## Task 2: Add Scoreboard-Backed Filtered Observability

### Goal

Add a first-class hot-path collection mode for the common case:

- counters
- histograms
- coverage
- branch direction stats
- cache / MMU event counts
- watchpoint prefilters

without requiring a callback for every event.

### Why this is second

Helm already has zero-cost "inactive" probes, but it still needs a stronger
"active and still cheap" model for targeted measurement. This is the point
where the external systems are most instructive:

- QEMU shows the value of inline scoreboards and conditional callbacks
- gem5 shows the value of hierarchical structured stats
- Simics shows the value of reusable filters and tool scope control

### Immediate work

1. Add reusable filter objects:
   - vCPU mask
   - address range
   - PC range
   - symbol
   - EL / mode
   - access type
   - instruction class
2. Add inline scoreboard-like counters for:
   - per-vCPU instruction counts
   - per-opcode / per-class execution counts
   - JIT block / trace / fallback / cache events
   - MMU walk and TLB counters
3. Support "if true then callback" style triggers so expensive handlers fire only when a filter matches.
4. Define a stable hierarchical stat namespace that `helm-report` can export.

### Primary landing spots

- `framework/helm-probe/src/probe.rs`
- `framework/helm-probe/src/lib.rs`
- `debug/helm-spy/src/primitives/*`
- `debug/helm-spy/src/analysis/*`
- `runtime/helm-engine/src/jit.rs`
- `runtime/helm-engine/src/lib.rs`
- `runtime/helm-engine/src/fs.rs`
- `runtime/helm-python/src/spy.rs`

### Suggested first slice

Implement a filtered counter path for:

- instruction class histogram
- branch taken/not-taken counts
- JIT fallback counts
- MMU walk counts

before attempting richer trace collection.

### Acceptance gate

- Focused observability sessions can keep always-on counters with minimal overhead.
- Expensive trace callbacks only fire when filters match.
- The same filters are reusable by `HelmSpy`, watchpoints, breakpoints, and future trace tools.
- Stats export has a stable shape instead of one-off ad hoc dictionaries.

---

## Task 3: Integrate Debug Control With Checkpoints And Replay

### Goal

Make `helm-debug` a coherent implementation layer behind the Python control plane instead of a bag of separate features.

The target workflow is:

1. set breakpoint / watchpoint / trace trigger
2. run
3. stop on event
4. inspect state
5. checkpoint or snapshot
6. replay / rewind / re-run with different observation settings

### Why this is third

Once Task 1 and Task 2 make collection structured and cheap, the next step is
to make debugging reproducible. This is especially important for:

- HAJ divergence debugging
- FS boot regressions
- device / IRQ races
- intermittent state corruption

### Immediate work

1. Finish and harden the GDB RSP surface in `runtime/helm-debug`.
2. Unify software breakpoints and watchpoints with probe-backed filtering.
3. Define a durable checkpoint flow that explicitly re-establishes probe/debug subscriptions after restore.
4. Add deterministic replay / rewind planning on top of checkpoints and execution event capture.
5. Expose a minimal inspection API for:
   - registers
   - memory ranges
   - symbol lookup
   - device register dumps
   - current debug trigger state

### Primary landing spots

- `runtime/helm-debug/src/gdb/*`
- `runtime/helm-debug/src/checkpoint.rs`
- `runtime/helm-debug/src/breakpoint.rs`
- `runtime/helm-debug/src/watchpoint.rs`
- `runtime/helm-debug/src/lib.rs`
- `runtime/helm-engine/src/lib.rs`
- `runtime/helm-engine/src/fs.rs`
- `runtime/helm-python/src/system.rs`
- `framework/helm-diag/src/*`

### Suggested first slice

Unify breakpoint/watchpoint subscription with a checkpoint restore hook, so:

- breakpoints and watchpoints are defined in one place
- restore does not silently lose intended debug state
- Python can checkpoint, restore, and resume with the same active debug intent

### Acceptance gate

- A user can stop on a probe-backed breakpoint or watchpoint, checkpoint, restore, and continue debugging without rebuilding state manually.
- `HelmSystem` remains the first-class user-facing control surface, while `helm-debug` owns implementation concerns.
- `helm-debug` owns control concerns, while `helm-spy` continues to own collection concerns.
- Replay/rewind has a clear architectural slot instead of being bolted onto ad hoc scripts.

---

## Cross-Cutting Constraints

These constraints should apply across all three tasks.

### 1. Do not grow new logic in `helm-plugin`

`helm-plugin` is legacy compatibility only.

### 2. Do not mix delivery with collection

`helm-spy` collects. `helm-report` formats and sinks.

### 3. Keep the hot path explicit

Every new observability feature should state which path it uses:

- zero-cost inactive probe
- inline counter / scoreboard
- filtered callback
- cold-path flush / snapshot only

### 4. Keep Python as orchestration, not the sole implementation

The public workflow should be Python-first, but core logic should live in Rust so:

- checkpoints are durable
- replay is not script-fragile
- GDB/debug control is not coupled to one launcher script

### 5. No separate monitor by default

Helm should not introduce a QEMU-monitor-style shell as the primary control
surface unless a real external-tooling need appears.

Default expectation:

- Python methods are the user-facing interface
- Rust crates implement the underlying behavior
- optional protocols or shells, if added later, are adapters over the same Rust control APIs

---

## Suggested Execution Order

1. Finish `probe -> spy -> report`.
2. Add reusable filters and inline scoreboard-style counters.
3. Rework existing legacy debug helpers to ride the new path where possible.
4. Integrate breakpoint/watchpoint/checkpoint flows.
5. Add replay / rewind once state capture and event ownership are clear.

---

## Out Of Scope For The First Pass

These are useful, but they should not block the first three tasks:

- external shared-library plugin ABI
- full IDE protocol integration beyond GDB RSP
- advanced power models
- full process-aware OS introspection
- complete Simics-style reverse execution semantics
- broad dynamic module loading

---

## Practical Next Action

Start with Task 1 and make one thin vertical slice:

`wire CpuProbes -> HelmSpy session -> HelmReport text/json output -> Python observe() demo`

That slice is small enough to verify end-to-end and strong enough to force the
intended architecture into the default user path.
