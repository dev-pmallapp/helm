# Performance-Safe API, Abstraction, and Interface Enhancement Plan

## Scope

This document is a repo-wide design review focused on:

- API design
- abstractions and interface boundaries
- software engineering discipline
- performance preservation

The review intentionally ignores worktrees and temporary runtime artifacts. It is based on the current code and docs in `framework/`, `runtime/`, `hw/`, and `docs/`.

## Executive Summary

The repository already has the right high-level instinct: keep the instruction path simple, monomorphize timing, and push flexibility toward configuration-time or cold paths. The main gap is not raw architecture quality. The main gap is that the codebase currently exposes multiple overlapping control-plane abstractions while the runtime hot path is converging on a different practical shape.

The highest-value improvement is to make the codebase explicitly two-tier:

1. A small, performance-critical runtime kernel with frozen data structures and minimal interfaces.
2. A richer build/configuration layer that can stay ergonomic, dynamic, and descriptive without leaking into execution.

If that separation is made explicit, the project can improve API clarity, documentation accuracy, extension surfaces, and maintainability without paying a performance tax.

## What Is Already Strong

### 1. The hot-path instinct is mostly correct

The code consistently treats `ExecContext`, `MemInterface`, instruction decode/execute, and `TimingModel` as special performance-sensitive seams. That is the correct foundation for a simulator.

### 2. Device design is aiming at good platform decoupling

`Device` intentionally does not know base addresses or IRQ numbers. `InterruptPin` and `AddressMap` reinforce that separation well.

### 3. There is a healthy preference for frozen runtime state

Several modules already assume a configuration phase followed by execution. That is the right shape for determinism, performance, and simpler ownership.

### 4. Many modules are intentionally small and composable

`helm-core`, `helm-event`, `helm-diag`, `helm-plugin`, `helm-devices`, and the hardware crates are laid out in a way that can scale if the boundaries are cleaned up.

## Current Structural Gaps

### 1. The runtime shape and the documented architecture have drifted apart

The docs describe a cleaner final architecture than the one the code currently executes. That is normal during development, but the drift is large enough that it is now affecting API clarity.

Examples:

- `runtime/helm-engine/src/lib.rs` still carries mixed ISA, mode, syscall, plugin, and FS state in one engine type.
- `framework/helm-memory/src/lib.rs` describes the future unified memory model, but execution currently depends on `FlatMem` and `HelmAddressSpace`.
- `runtime/helm-platform/src/lib.rs` exposes a platform descriptor interface, but actual arm-virt construction still lives in the engine.
- `runtime/helm-python/src/system.rs` still builds a hardcoded AArch64 simulator rather than instantiating a fully described object graph.
- `docs/api.md` and parts of `docs/traits.md` describe APIs that do not exactly match current code.

This is the single biggest source of confusion for contributors.

### 2. There are three partially overlapping memory abstractions

The repo currently has:

- `FlatMem` in `framework/helm-memory/src/flat_mem.rs`
- `HelmAddressSpace` in `framework/helm-memory/src/address_space.rs`
- `MemoryMap` in `framework/helm-memory/src/lib.rs`
- `ByteMem` in `framework/helm-core/src/mem.rs`

This is more than a transitional inconvenience. It splits design effort, tests, and mental models across multiple address-space implementations.

The correct long-term direction is one memory subsystem with multiple internal fast paths, not multiple public memory concepts.

### 3. Platform is described as a first-class abstraction but is not yet a first-class build boundary

`runtime/helm-platform` currently behaves more like a topology descriptor library than the place where platforms are truly built. The engine still owns platform construction details for arm-virt.

That weakens interface clarity in three ways:

- platform authors cannot reason about a single authoritative API
- engine code absorbs board-specific construction concerns
- Python/config APIs cannot cleanly target a stable platform boundary

### 4. Control-plane and data-plane interfaces are not clearly separated

The repo already distinguishes hot and cold paths conceptually, but the public design language is not yet strict enough.

A few examples:

- `HelmEventBus`, `EventQueue`, plugin callbacks, and probe macros are all valid surfaces, but they overlap in purpose
- `Device` is lean, but lifecycle/context/snapshot concerns are spread across docs, placeholders, and adjacent types
- Python-side `SimObject` hierarchy exists, but the execution engine does not yet consume that hierarchy as the authoritative build graph

The missing piece is an explicit statement that different interfaces are allowed to optimize for different goals, and that only a very small subset may ever reach the instruction path.

### 5. Documentation and API contracts are ahead of implementation

This repo is unusually well documented, which is a strength. But once docs become more aspirational than executable, they stop being reliable contracts.

The fix is not less documentation. The fix is better contract discipline:

- documents must clearly distinguish `current`, `planned`, and `invariant`
- public surface docs should be tied to code and tests
- examples should compile or be mechanically validated where possible

## Design Principles To Preserve

These should become non-negotiable architectural guardrails.

### 1. No new dynamic dispatch in the instruction path

Do not turn ISA execution, timing, or register/memory access into object-safe runtime-polymorphic interfaces.

Acceptable:

- enum dispatch at chunk boundaries
- generic timing model
- trait objects in MMIO, plugins, debug, config, device lifecycle, and host integrations

Not acceptable:

- `Box<dyn ExecContext>`
- `Box<dyn TimingModel>` in the step loop
- string-based lookup during execution

### 2. No allocations, logging, or locking in the common step path

If a feature needs per-instruction allocation, locking, or string handling, it belongs behind a disabled-by-default observer path or in a cold boundary.

### 3. Configuration flexibility must compile down to frozen runtime tables

The runtime should operate on:

- arrays
- compact structs
- pre-resolved handles
- numeric IDs
- immutable routing tables

It should not operate on late-bound names, maps, or Python objects after instantiation.

### 4. Use richer abstractions on cold paths only

The project does not need to be “minimal everywhere.” It needs to be minimal where performance matters.

That means:

- richer config models are acceptable
- better diagnostics are acceptable
- typed builders are acceptable
- schema-driven device creation is acceptable

As long as those collapse into a frozen runtime representation before `run()`.

## Recommended Enhancements

### 1. Formalize a two-phase architecture

Make the architecture explicit in both code and docs:

- **Build phase**: object graph creation, parameter validation, platform wiring, naming, schema use, topology inspection
- **Run phase**: pure simulation on frozen state, numeric handles only, no structural mutation

The current code is already trending this way. The enhancement is to make it the official rule and refactor APIs around it.

### Concrete outcome

Introduce one authoritative “built system” boundary, conceptually similar to:

- `SystemBuilder` or `MachineBuilder` for configuration
- `BuiltSystem` or `RuntimeSystem` for execution

This does not require a performance cost. It should reduce accidental coupling.

### 2. Collapse memory toward one authoritative subsystem

The repo should converge toward one public memory abstraction with internal specialization.

### Recommendation

Treat `framework/helm-memory` as the only long-term memory API and move current performance tricks into it instead of keeping parallel implementations alive.

That means:

- preserve the `FlatMem` fast path idea
- preserve `HelmAddressSpace` device dispatch behavior
- preserve `AddressMap` flat-view lookup discipline
- unify them behind one coherent `MemoryMap` / address-space API

### Why this is performance-safe

This is not a request to replace fast RAM accesses with a generic boxed tree walk. The right end state is:

- a unified memory API
- multiple specialized internal backends
- cached flat views and direct RAM fast paths
- pre-resolved MMIO dispatch tables

The abstraction should unify the model, not homogenize the implementation.

### 3. Make platform a real construction boundary

`runtime/helm-platform` should own more than metadata and topology printing. It should become the authoritative home for platform assembly contracts.

### Recommendation

Evolve platform APIs so a platform can produce a frozen runtime description:

- memory regions
- device instances or device factories
- interrupt routes
- boot configuration
- attachment slots
- optional topology metadata

The engine should consume that result, not contain board construction logic itself.

### Expected benefit

- engine becomes more ISA/mode/runtime focused
- platform code becomes testable in isolation
- Python config gets a stable target
- board-specific changes stop leaking into the kernel

### 4. Split engine responsibilities more explicitly

`HelmEngine<T>` currently carries too many responsibilities in one concrete type:

- ISA-specific state
- mode-specific behavior
- syscall handling
- FS machine state
- plugins
- probes
- symbols
- timers and run loop bookkeeping

This is workable now, but it will get more expensive to reason about as more ISAs and modes arrive.

### Recommendation

Refactor internally toward a thinner kernel plus frozen sub-objects, while preserving the current performance model.

Possible direction:

- keep timing generic
- keep run-loop dispatch explicit
- move ISA-specific state into dedicated runtime structs
- move FS machine state into a dedicated machine/runtime object
- keep plugin/probe state behind clearly optional observer structs

The important point is not a particular type name. The important point is to reduce the number of unrelated invariants that one struct must maintain.

### Performance constraint

Do not solve this with hot-path trait objects. Prefer:

- enums
- separate structs
- pre-borrowed or pre-split state passed into step functions

### Multi-ISA scaling rule

This matters even more once the project supports many architectures. The engine must not evolve into:

- `a64_state`
- `a64_handler`
- `a64_fs`
- `riscv_state`
- `riscv_handler`
- `x86_state`
- and so on

That does not scale.

The long-term shape should be:

- `helm-arch` owns ISA semantics, architectural state, decode/execute, privilege behavior, MMU/TLB rules, and other ISA-level helpers
- `helm-engine` owns run-loop orchestration, scheduling, syscall integration, device/platform interaction, and full-system session management
- `helm-platform` owns machine layout and fixed wiring contracts

The engine should therefore stop accumulating ad hoc per-ISA fields. The immediate cleanup step can be a single selected runtime, but the long-term design must also allow heterogeneous machines. That means the architecture should support either:

- a single selected runtime for simple single-core / homogeneous sessions, or
- a runtime collection for heterogeneous systems, where each executable compute context owns its own runtime

The next simplification step can look like this:

```rust
struct HelmEngine<T: TimingModel> {
    timing: T,
    memory: FlatMem,
    events: EventQueue,
    runtime: Runtime,
}

enum Runtime {
    Riscv(RiscvCore),
    Aarch64(Aarch64Core),
    Aarch32(Aarch32Runtime),
    // future ISAs...
}
```

But the heterogeneous end-state should be closer to a session or machine object that owns a vector of runtimes:

```rust
struct RuntimeSystem<T: TimingModel> {
    timing: T,
    memory: FlatMem,
    events: EventQueue,
    runtimes: Vec<Runtime>,
}

enum Runtime {
    Riscv(RiscvCore),
    Aarch64(Aarch64Core),
    Aarch32(Aarch32Runtime),
    // GPU / DSP / accelerator runtimes later
}
```

In other words:

- do not let `HelmEngine` become a pile of optional per-architecture fields
- do not assume the entire simulated machine is one ISA forever
- model one runtime per executable compute context, then let higher-level system/session objects own a vector when heterogeneous computing is needed

Inside each ISA runtime, mode-specific state can remain explicit:

```rust
enum Aarch64Core {
    Functional(Aarch64ArchState),
    Syscall { state: Aarch64ArchState, handler: LinuxAarch64SyscallHandler },
    System(HelmBoard),
}
```

This keeps enum dispatch coarse-grained and preserves hot-path performance, while leaving a clean path toward heterogeneous machines.

### Ownership boundary

Not all AArch64 code belongs in `helm-arch`.

`helm-arch` should own:

- architectural state
- decode/execute
- exceptions and privilege behavior
- MMU/TLB logic
- ISA-level helper/state transitions

`helm-engine` should continue to own:

- the run loop
- mode orchestration
- syscall dispatch integration
- platform boot/session construction
- full-system vCPU scheduling
- device/platform interaction

The plan is therefore not "move all AArch64 code out of engine." The plan is "move ISA concerns back to `helm-arch`, and keep orchestration concerns in `helm-engine`."

### 5. Keep `Device` small, and add adjacent traits instead of growing it

The actual `Device` trait is lean and mostly good. It should stay that way.

### Recommendation

Do not turn `Device` into a god trait covering:

- lifecycle
- reset
- snapshot
- topology
- scheduling
- host services
- statistics

Instead, add adjacent cold-path traits or capability structs as needed, for example:

- `ResettableDevice`
- `SnapshotDevice`
- `SignalDevice`
- `SchedulableDevice`

or equivalent internal capability enums/registrations.

### Why this matters

Small hot-adjacent traits are easier to implement, test, and optimize. Large universal traits create accidental coupling and slow every extension point down conceptually even when runtime overhead is unchanged.

### 6. Unify observability as a layered system

Right now the repo has four distinct observability surfaces:

- `helm-event::EventQueue` for deferred runtime events
- `helm-devices::HelmEventBus` for synchronous observers
- `helm-plugin::HelmPluginRegistry` for extensible execution hooks
- `helm-probe` macros for low-overhead typed instrumentation

Each has a valid role, but the intended usage boundaries are not explicit enough.

### Recommendation

Document and enforce a layered model:

- `EventQueue`: simulation semantics
- `HelmEventBus`: structural/device-level synchronous notifications
- `helm-probe`: near-zero-overhead internal instrumentation
- `helm-plugin`: user-facing extension hooks and analysis tools

Then make sure new features choose one surface deliberately instead of adding a fifth path.

### Performance rule

Only `helm-probe` and carefully guarded plugin hooks should ever touch the common instruction path. Everything else should stay cold or explicitly opt-in.

### 7. Make Python configuration authoritative or intentionally thin

The Python layer is currently between two states:

- it has a `SimObject` hierarchy and redesign direction
- but `System::instantiate()` still hardcodes a direct engine build path

This creates confusion about whether Python is descriptive or merely convenience sugar.

### Recommendation

Choose one model and finish it.

The stronger option is:

- Python describes an object graph
- Rust validates and freezes it
- runtime executes the frozen graph

That matches the project’s documented direction and will make platform/machine APIs much cleaner.

If that is deferred, then the Python layer should be documented as intentionally thin and non-authoritative for now.

### Performance rule

No Python object graph, dictionary, or string lookup should be consulted once the runtime is instantiated.

### 8. Introduce explicit API tiers

The project would benefit from naming its API tiers directly.

### Suggested tiers

- **Tier 1: Hot runtime contracts**
  - `ExecContext`
  - `MemInterface`
  - `TimingModel`
  - ISA decode/execute entry points
- **Tier 2: Frozen runtime composition**
  - platform build outputs
  - memory layouts
  - interrupt routes
  - device registries resolved to handles
- **Tier 3: Build/config APIs**
  - Python `SimObject`
  - parameter schemas
  - platform descriptors
  - topology builders
- **Tier 4: Observability and tooling**
  - plugins
  - probes
  - debug
  - reporting

Every public API should clearly belong to one tier. That makes performance review much easier because the hot/cold expectations become obvious.

### 9. Add contract discipline for docs and interfaces

The repo needs a small amount of process to keep its design quality from turning into drift.

### Recommendation

For public-facing docs and core traits:

- mark sections as `Current`, `Planned`, or `Invariant`
- add compile-checked examples where possible
- add contract tests for critical interface invariants
- keep a short “surface map” doc that points to the code that is authoritative today

### Particularly important

`docs/api.md` and `docs/traits.md` should not describe types or signatures that no longer exist. Once API docs drift, extension work becomes slower and riskier.

## Priority Roadmap

### Phase 1: Clarify and freeze boundaries

Low-risk, high-value work:

- define API tiers in docs
- mark current vs planned architecture explicitly
- document hot-path guardrails
- document one authoritative memory convergence plan
- document one authoritative platform build boundary

This phase improves contributor behavior immediately without touching performance-sensitive code.

### Phase 2: Remove overlapping control-plane abstractions

Medium-risk, high-value work:

- move platform build logic out of the engine
- make Python configuration target the same build boundary
- unify observability roles
- reduce duplicated memory/address-space concepts

This phase should mostly improve maintainability and testability while staying performance-neutral.

### Phase 3: Consolidate runtime internals around frozen state

Higher-risk work:

- reduce mixed-mode/mixed-ISA state inside the engine
- make per-ISA runtime state more explicit
- collapse memory implementation duplication
- push all late-bound configuration out of execution objects

This phase can improve both clarity and performance if done carefully.

## Suggested Acceptance Criteria

An enhancement should be considered successful only if it satisfies all of the following:

- the instruction path does not gain new dynamic dispatch
- the instruction path does not gain new allocation or locking
- the same feature is not modeled by multiple public abstractions
- platform construction has one authoritative boundary
- memory has one authoritative API
- Python/config surfaces map cleanly to runtime construction
- docs describing public interfaces match the code that exists now

## Short List of Immediate Repo-Specific Targets

These are the highest-leverage places to clean up first:

- `runtime/helm-engine/src/lib.rs`
  - reduce mixed responsibilities and make runtime state partitions clearer
- `framework/helm-memory/src/lib.rs`
  - turn it into the destination of memory convergence, not a side path
- `runtime/helm-engine/src/system_mem.rs`
  - treat as a transitional integration layer, not a permanent parallel memory API
- `runtime/helm-platform/src/lib.rs`
  - upgrade from descriptor-only role toward true platform build authority
- `runtime/helm-python/src/system.rs`
  - align instantiation with the actual long-term object-graph model
- `docs/api.md` and `docs/traits.md`
  - bring them back in sync with current code or clearly label planned content

## Implementation Progress

This section is the canonical recovery checkpoint for ongoing implementation work.

### Active worktree

- Worktree path: `/home/pmallapp/proj/personal/helm-ng/.claude/worktrees/perf-safe-api`
- Branch: `worktree-perf-safe-api`

### Completed slices

#### Slice 1: Build/runtime boundary made explicit

Completed:

- Added a frozen platform build-plan API in `runtime/helm-platform`
- Added Python-side frozen system config / instantiate path
- Routed `System::instantiate()` and legacy `build_simulation()` through the same build boundary
- Began making `helm-platform` the source of truth for arm-virt layout metadata

Key outcomes:

- Python configuration no longer jumps straight from ad hoc fields to `build_simulator()`
- platform metadata is now a real build artifact instead of only topology documentation

#### Slice 2: Memory ownership moved toward `helm-memory`

Completed:

- Moved `FlatMem` from `runtime/helm-engine` to `framework/helm-memory`
- Re-exported `FlatMem` through `helm-engine` for compatibility
- Moved `HelmAddressSpace` from `runtime/helm-engine` to `framework/helm-memory`
- Left `helm_engine::address_space::HelmAddressSpace` as a compatibility re-export

Key outcomes:

- framework-level memory ownership is now more canonical
- `helm_core::ByteMem` now provides one shared byte-access contract across
  runtime helpers, VirtIO, and IOMMU table walks
- engine no longer owns the two main RAM/MMIO composition types

#### Slice 3: Platform plan is now used for validation and arm-virt assembly

Completed:

- Added helper lookups on `PlatformBuildPlan`
- Added FS-mode Python instantiate validation against arm-virt fixed regions
- Added attachment-window validation for unknown devices in FS mode
- Switched arm-virt engine assembly to consume platform build-plan metadata for fixed placement and UART interrupt routing

Key outcomes:

- the platform crate is no longer documentation-only for arm-virt
- Python-side FS config is checked against the same platform contract used by engine assembly

#### Slice 4: AArch64 runtime state consolidated inside `HelmEngine`

Completed:

- Replaced the parallel `a64_state` / `a64_handler` / `a64_fs` engine fields with one explicit internal AArch64 runtime state representation
- Introduced an internal mode-shaped AArch64 runtime enum
- Switched AArch64 state access, syscall handling, and FS machine accessors to go through that enum
- Initialized functional AArch64 engines with an explicit architectural runtime state instead of leaving the variant dormant

Representative shape:

```rust
enum Aarch64Core {
    Disabled,
    Functional(Aarch64ArchState),
    Syscall { state: Aarch64ArchState, handler: LinuxAarch64SyscallHandler },
    System(HelmBoard),
}
```

Key outcomes:

- the engine no longer models AArch64 as three loosely-related optional fields
- the direction toward a future top-level multi-ISA runtime enum is now concrete
- the boundary between ISA-owned state and engine-owned orchestration is clearer

#### Slice 5: RISC-V runtime fields consolidated

Completed:

- Wrapped the RISC-V architectural runtime fields inside a dedicated `RiscvCore` struct
- Switched RISC-V loader/setup, syscall dispatch, `ExecContext`, and `pc()` fallback paths to use that runtime struct
- Removed another batch of per-architecture fields from the main `HelmEngine` body

Representative shape:

```rust
struct RiscvCore {
    iregs: [u64; 32],
    fregs: [u64; 32],
    csrs: Box<[u64; 4096]>,
    pc: u64,
    lr_addr: Option<u64>,
}
```

Key outcomes:

- the engine no longer stores raw RISC-V register arrays directly at top level
- AArch64 and RISC-V now both have explicit per-ISA runtime containers
- the remaining jump to a higher-level multi-ISA runtime container is smaller and more mechanical

#### Slice 6: Shared runtime container introduced

Completed:

- Added a shared runtime container layer in `runtime/helm-engine`
- Changed `HelmEngine` to hold `HelmCoreSet` rather than separate top-level `riscv` and `aarch64` fields
- Kept the container homogeneous for now, but shaped it as a `Vec<Runtime>`-backed owner so the path toward heterogeneous systems remains open
- Routed AArch64 and RISC-V access through container helpers instead of direct engine fields

Representative shape:

```rust
struct HelmCoreSet {
    primary: usize,
    runtimes: Vec<Runtime>,
}

enum Runtime {
    Riscv(RiscvCore),
    Aarch64(Aarch64Core),
}
```

Key outcomes:

- `HelmEngine` no longer carries separate top-level per-ISA runtime fields
- the next step toward heterogeneous systems is now about scheduler/session ownership, not about undoing engine field layout
- one-runtime-per-compute-context is now reflected in code structure instead of only in the plan

#### Slice 7: Runtime selection semantics introduced

Completed:

- Added a typed `HelmCoreId`
- Changed `HelmCoreSet` to track an active runtime explicitly instead of using an untyped index field
- Added container operations that can evolve naturally toward heterogeneous runtime scheduling
- Added unit tests covering active-runtime switching and invalid-selection rejection

Representative shape:

```rust
struct HelmCoreSet {
    active: HelmCoreId,
    runtimes: Vec<Runtime>,
}

struct HelmCoreId(usize);
```

Key outcomes:

- runtime selection is now an explicit concept rather than an implicit `0` index
- the next step toward heterogeneous systems can focus on policy and ownership rather than on inventing identifiers
- the engine now has a clean place to add scheduler-driven runtime selection later

#### Slice 8: Runtime ownership lifted into a session wrapper

Completed:

- Added a `HelmMachine` wrapper around the runtime collection
- Changed `HelmEngine` to own session state rather than owning the runtime collection directly
- Routed runtime access through session helpers in both engine code and tests

Representative shape:

```rust
struct HelmMachine {
    runtimes: HelmCoreSet,
}
```

Key outcomes:

- runtime ownership is now one layer farther away from the execution loop
- the path to a future machine/session object that owns multiple runtimes is clearer
- the engine shape is moving toward “loop + timing + memory + session” rather than “loop + lots of ISA state”

#### Slice 9: Runtime selection policy introduced

Completed:

- Added explicit runtime-selection policy to the session layer
- Added `Fixed(HelmCoreId)` and `RoundRobin` policy variants
- Added tests covering fixed selection and round-robin advancement

Representative shape:

```rust
enum HelmSchedulePolicy {
    Fixed(HelmCoreId),
    RoundRobin,
}
```

Key outcomes:

- runtime choice is now policy-shaped rather than just index-shaped
- the session layer now has a concrete place to host future heterogeneous scheduling logic
- later scheduler work can extend policy instead of inventing a new selection mechanism from scratch

#### Slice 10: Session/runtime ownership extracted to a dedicated module

Completed:

- Moved runtime and session ownership types out of `runtime/helm-engine/src/lib.rs`
- Added a dedicated `runtime/helm-engine/src/session.rs` module to hold:
  - per-ISA runtime containers
  - runtime identifiers
  - runtime collection container
  - session ownership wrapper
  - runtime selection policy
- Reduced the amount of ownership scaffolding embedded directly inside the engine loop file

Key outcomes:

- `lib.rs` is now more clearly the engine loop/orchestration layer
- runtime/session ownership has a dedicated place to evolve
- future heterogeneous coordination work no longer needs to begin inside the giant engine file

#### Slice 11: Session-owned active ISA dispatch introduced

Completed:

- Added ISA lookup on the runtime/session layer
- Switched the engine run loop to dispatch from the active runtime ISA rather than relying only on the separate engine-level `isa` field
- Switched syscall-mode AArch64-vs-RISC-V dispatch selection to use the session-owned active ISA

Key outcomes:

- runtime identity is moving toward the session as the source of truth
- the remaining `HelmEngine.isa` field is now closer to legacy/config metadata than execution truth
- this is the first concrete step toward scheduler-driven multi-runtime dispatch

#### Slice 12: RISC-V syscall ownership moved into runtime state

Completed:

- Moved the RISC-V syscall handler off `HelmEngine` and into `RiscvCore`
- Kept syscall dispatch behavior unchanged while reducing another engine-level per-ISA field

Key outcomes:

- mode-specific RISC-V runtime state now lives with the RISC-V runtime
- the engine owns less per-ISA execution state directly
- the path toward per-runtime mode ownership is clearer

#### Slice 13: Session-owned mode/ISA state used by execution dispatch

Completed:

- Added active ISA lookup to the session/runtime layer
- Added active mode lookup to the session/runtime layer
- Switched the engine run loop and syscall-path ISA/mode decisions to prefer session-owned runtime state
- Synced RISC-V constructor/setup paths so runtime mode matches session-owned execution mode

Key outcomes:

- execution dispatch now trusts runtime/session state more than duplicated engine bookkeeping
- `HelmEngine.isa` and `HelmEngine.mode` are increasingly legacy/config metadata instead of the sole execution truth
- this reduces another source-of-truth split before heterogeneous scheduling logic is added

#### Slice 14: Runtime labels and roles introduced

Completed:

- Added runtime labels and roles to the session/runtime layer
- Added APIs to query and update runtime labels and roles by `HelmCoreId`
- Added test coverage for runtime metadata

Representative shape:

```rust
struct HelmCoreMeta {
    label: String,
    role: HelmCoreRole,
}

enum HelmCoreRole {
    PrimaryCpu,
    Cpu,
    Accelerator,
    Service,
}
```

Key outcomes:

- heterogeneous runtime coordination now has a place to attach identity and intent beyond raw runtime indices
- future scheduler and topology work can build on stable runtime metadata rather than inventing ad hoc side tables

#### Slice 15: Session progress now drives runtime selection

Completed:

- Added a session progress hook
- Wired `HelmEngine::run()` to notify the session after retired work
- Made runtime selection policy part of live execution behavior rather than just stored state

Representative shape:

```rust
enum RunStep {
    RetiredInstruction,
    YieldedQuantum,
}
```

Key outcomes:

- round-robin selection can now affect active runtime choice during execution
- the session layer has a real execution-time coordination hook
- future heterogeneous scheduling logic now has a concrete place to plug in

#### Slice 16: Session scheduler extracted

Completed:

- Extracted selection/progress behavior into a dedicated session scheduler object
- Kept the session wrapper as the owner of runtimes and metadata
- Reduced the amount of coordination logic directly embedded in `HelmMachine`

Key outcomes:

- coordination policy is now a distinct concept from runtime ownership
- future heterogeneous scheduling work has a clearer extension point
- session data and scheduler behavior are less tightly coupled

#### Slice 17: Scheduler coordination made role-aware

Completed:

- Made round-robin selection inspect `HelmCoreMeta.role` instead of treating every runtime slot as equivalent
- Constrained the current round-robin policy to CPU-class roles (`PrimaryCpu` and `Cpu`) when such runtimes exist
- Kept an explicit fallback to slot-based rotation only when no CPU-class runtimes are present, so unusual future runtime vectors still remain operable
- Resynced the active runtime immediately when runtime-role updates invalidate the currently active round-robin choice
- Added tests covering accelerator/service skipping and role-change resynchronization

Representative shape:

```rust
impl HelmCoreRole {
    fn participates_in_round_robin(self) -> bool {
        matches!(self, Self::PrimaryCpu | Self::Cpu)
    }
}
```

Key outcomes:

- the scheduler now uses metadata and role intent rather than raw vector position for coordination decisions
- heterogeneous runtime vectors can distinguish executable CPU contexts from service/accelerator sidecars without touching the instruction hot path
- role changes are now observable by scheduler policy immediately, which removes another source-of-truth gap between metadata and coordination state

#### Slice 18: Scheduler policy gained explicit scope and progress controls

Completed:

- Expanded `HelmSchedulePolicy` from a simple fixed-vs-round-robin toggle into a richer cold-path policy shape
- Added explicit runtime-selection scope so round-robin can target:
  - all runtimes
  - compute runtimes
  - a specific runtime role
- Added explicit progress-trigger control so scheduler advancement can occur on:
  - every progress event
  - retired instructions only
  - yielded quanta only
- Preserved the current compatibility path by making the default round-robin helper mean “compute-scoped, advance on any progress”
- Added tests covering role-targeted service scheduling plus progress-filtered advancement for both yielded-quantum and retired-instruction semantics

Representative shape:

```rust
enum HelmSchedulePolicy {
    Fixed(HelmCoreId),
    RoundRobin {
        scope: HelmCoreScope,
        advance_on: HelmAdvancePolicy,
    },
}
```

Key outcomes:

- scheduler coordination is now explicit about both *which* runtimes participate and *when* the active runtime may advance
- heterogeneous systems can express different coordination semantics for CPU, service, and other runtime classes without adding dynamic dispatch or allocation to the execution loop
- the session layer now has a stronger foundation for future machine-level policies that manage multiple runtime classes with different progress semantics

#### Slice 19: Scheduler topology state moved out of repeated metadata scans

Completed:

- Added scheduler-owned runtime topology state that pre-indexes runtimes by:
  - all runtimes
  - compute runtimes
  - per-role runtime groups
- Reworked scoped round-robin selection to use cached topology membership instead of rescanning runtime metadata on each scheduler progress event
- Refreshed scheduler topology whenever runtime membership or runtime roles change
- Made `HelmMachine::push()` notify the scheduler so scoped policies can immediately observe newly added eligible runtimes
- Added regression coverage proving topology refresh works both for role changes and for newly pushed runtimes

Representative shape:

```rust
struct SessionScheduler {
    selection: HelmSchedulePolicy,
    topology: CoreTopology,
}
```

Key outcomes:

- scheduler coordination now owns durable heterogeneous topology state instead of recomputing scope membership from scratch on each advancement decision
- per-progress scheduler work is reduced to cached-slice selection plus simple index stepping, which is safer for performance as runtime vectors grow
- the session layer now has a concrete place to accumulate richer scheduler-owned coordination state beyond raw policy enums

#### Slice 20: Explicit coordination domains introduced for heterogeneous clusters

Completed:

- Added explicit runtime coordination-domain metadata to the session/runtime layer
- Made coordination domains part of the cached scheduler topology so domain-scoped policies can use precomputed membership
- Extended runtime-selection scope so round-robin can target a specific coordination domain
- Added APIs to query and update runtime domains by `HelmCoreId`
- Refreshed scheduler topology when runtime domains change, keeping domain-scoped policies live and self-consistent
- Added regression coverage for:
  - domain-scoped round-robin selection
  - domain-change resynchronization
  - default-vs-updated domain metadata

Representative shape:

```rust
struct HelmCoreMeta {
    label: String,
    role: HelmCoreRole,
    domain: HelmCluster,
}
```

Key outcomes:

- heterogeneous runtime vectors now have an explicit cluster/domain abstraction instead of relying only on flat role labels
- scheduler coordination can target grouped runtime clusters without rescanning runtime metadata or inventing side tables
- the session layer now has the first concrete abstraction for future machine-level heterogeneous coordination beyond per-role routing

#### Slice 21: Scheduler gained per-domain progress bookkeeping

Completed:

- Added scheduler-owned per-domain progress counters
- Attributed progress to the active runtime’s coordination domain before scheduler advancement occurs
- Reused cached runtime-to-domain topology so progress bookkeeping remains O(1) on the existing progress hook
- Preserved counts across runtime-role and runtime-domain topology refreshes
- Added query APIs for domain progress on the session layer
- Added regression coverage for:
  - progress attribution across active-runtime changes
  - progress attribution after runtime-domain reassignment

Representative shape:

```rust
struct SessionScheduler {
    selection: HelmSchedulePolicy,
    topology: CoreTopology,
    progress_by_domain: Vec<HelmClusterProgress>,
}
```

Key outcomes:

- the scheduler now owns durable heterogeneous progress bookkeeping in addition to policy and topology
- future machine/session coordination can reason about per-domain forward progress without reconstructing that state from the engine loop
- the added runtime cost is constrained to cached domain lookup plus counter increments on the already-existing progress hook

#### Slice 22: Domain-local compute scheduling scope introduced

Completed:

- Extended runtime-selection scope with explicit compute-within-domain targeting
- Added cached per-domain compute membership to scheduler topology
- Made compute-domain selection skip non-compute runtimes within the same coordination domain
- Preserved the existing performance profile by resolving the new scope through cached topology slices rather than metadata scans
- Added regression coverage for:
  - domain-local compute round-robin
  - resynchronization when domain-local compute membership changes

Representative shape:

```rust
enum HelmCoreScope {
    Compute,
    ComputeInDomain(HelmCluster),
    // ...
}
```

Key outcomes:

- the scheduler can now target executable compute contexts inside a heterogeneous cluster without being diluted by sidecar/service runtimes in that same domain
- role and domain abstractions now compose directly in the scheduling model
- this gives future machine-level coordination a more precise building block than either domain-only or role-only targeting

#### Slice 23: Session-owned coordination state extracted from scheduler behavior

Completed:

- Moved cached topology and per-domain progress bookkeeping out of `SessionScheduler`
- Added a dedicated session-owned coordination state that now holds:
  - cached topology
  - per-domain progress counters
- Kept the scheduler focused on policy/selection behavior and made it consume shared coordination state owned by the session
- Preserved existing scheduling semantics and progress accounting while clarifying ownership boundaries
- Verified the refactor against the existing scheduler/session regression suite

Representative shape:

```rust
struct SessionCoordinationState {
    topology: CoreTopology,
    progress_by_domain: Vec<HelmClusterProgress>,
}
```

Key outcomes:

- the session now owns reusable heterogeneous coordination state instead of burying it inside scheduler behavior
- this creates a cleaner handoff point for a future machine-owned coordination object that can consume the same state without inheriting scheduler internals
- performance characteristics stay stable because the scheduler still reads cached slices and the progress hook still performs only cached lookup plus counter updates

#### Slice 24: Machine-facing coordination view introduced

Completed:

- Added a read-only machine-facing coordination view built from session-owned coordination state
- Exposed runtime-level coordination entries including:
  - active flag
  - ISA/mode
  - role/domain
  - label
- Exposed domain-level coordination entries including:
  - runtime membership
  - compute-runtime membership
  - accumulated domain progress
- Preserved visibility of progress for domains that currently have no runtime members, so higher-level orchestration can still reason about prior progress
- Added regression coverage for:
  - runtime/domain snapshot correctness
  - progress visibility for empty-but-historic domains

Representative shape:

```rust
struct HelmMachineView {
    active_runtime: HelmCoreId,
    runtimes: Vec<HelmCoreView>,
    domains: Vec<HelmClusterView>,
}
```

Key outcomes:

- a future machine-owned coordination object now has a clean read-only surface to consume without reaching into scheduler internals
- the session layer can publish heterogeneous coordination state in a form that is stable, inspectable, and decoupled from selection behavior
- this creates a concrete next step for lifting cross-runtime orchestration above `HelmMachine` while preserving the current performance boundary

#### Slice 25: Active-runtime execution view cached to recover hot-path throughput

Completed:

- Added a cached active-runtime execution view inside session-owned coordination state
- Switched `active_id`, `active_isa`, `active_mode`, and progress attribution to use the cached active-runtime view on the hot path
- Added explicit cache refreshes on:
  - active-runtime switches
  - metadata-driven scheduler resynchronization
  - in-place active-runtime mode changes
  - `replace_primary()` lifecycle resets
- Added regression coverage proving the active-runtime cache tracks explicit and scheduler-driven runtime switches

Representative shape:

```rust
struct ActiveRuntimeCoordination {
    id: HelmCoreId,
    isa: Isa,
    mode: Option<ExecMode>,
    domain: HelmCluster,
}
```

Key outcomes:

- the engine no longer re-derives active ISA/mode/domain from the runtime vector on every instruction
- the hottest post-step coordination operations are reduced to cached field reads and counter updates
- the observed regression during this refactor series recovered substantially, from roughly `12-14 MIPS` back to about `18 MIPS` after the cache refresh points were wired correctly
- this keeps the richer heterogeneous coordination model while restoring most of the throughput lost to repeated session/runtime lookups

#### Slice 26: Machine-owned coordination state introduced above the session view

Completed:

- Added a dedicated machine-owned coordination-state module
- Built machine-level summaries from the session-owned coordination view rather than from scheduler internals
- Added domain summaries with:
  - runtime counts
  - compute-runtime counts
  - per-role counts
  - accumulated domain progress
- Added machine-level queries for:
  - total runtime count
  - total compute-runtime count
  - busiest domain by retired instructions
- Kept the new layer cold-path only so it does not participate in instruction-step execution
- Added regression coverage for machine-level summary correctness

Representative shape:

```rust
struct MachineCoordinationState {
    active_runtime: HelmCoreId,
    runtimes: Vec<HelmCoreView>,
    domains: Vec<MachineDomainSummary>,
}
```

Key outcomes:

- the project now has the first explicit machine-owned coordination object above `HelmMachine`
- machine-level orchestration can consume summarized heterogeneous state without coupling itself to session/scheduler implementation details
- this creates a clean next step for policy-feedback work that can reason over domains, roles, and progress while staying off the hot path

#### Slice 27: Machine-level policy feedback introduced as a cold-path advisory layer

Completed:

- Added machine-level policy feedback derived from machine coordination state
- Made the feedback prefer the busiest compute-capable domain when domain progress makes that meaningful
- Added a fallback to global compute scope when no domain has distinguishing compute progress yet
- Kept the feedback read-only and advisory so it does not yet alter scheduler behavior
- Added regression coverage for:
  - busiest-domain preference
  - global-compute fallback

Representative shape:

```rust
struct MachinePolicyFeedback {
    preferred_scope: Option<HelmCoreScope>,
    busiest_domain: Option<HelmCluster>,
    busiest_domain_progress: Option<HelmClusterProgress>,
}
```

Key outcomes:

- the project now has its first explicit machine-owned policy-feedback layer above the session/scheduler boundary
- higher-level orchestration can reason about domains, roles, and progress using a stable advisory API before any live scheduling coupling is introduced
- this creates a safe path to future policy-driven heterogeneous coordination without putting new logic into the instruction hot path

#### Slice 28: Remaining RISC-V control-plane mutation paths moved behind session APIs

Completed:

- Added session-owned APIs for remaining RISC-V mode and syscall-handler mutations
- Switched engine cold-path setup flows to use session-owned RISC-V control-plane APIs instead of mutating runtime internals directly
- Kept active-runtime cache refresh semantics localized to the session layer
- Added regression coverage proving the session-owned RISC-V mode API updates the active-mode cache correctly

Representative shape:

```rust
impl HelmMachine {
    fn set_riscv_mode(&mut self, mode: ExecMode) -> bool { ... }
    fn set_riscv_syscall_handler(&mut self, handler: Option<Box<dyn SyscallHandler>>) -> bool { ... }
}
```

Key outcomes:

- `HelmEngine` now owns less per-runtime control-plane mutation logic directly
- active-runtime cache coherency for RISC-V setup paths is enforced at the session boundary instead of being open-coded in the engine
- this continues the long-term move toward “engine orchestrates, session owns runtime coordination/control state”

### Current in-progress focus

The next architectural step is no longer “introduce a runtime container.” That slice is now in place. The next step is to build on it:

- push session ownership beyond runtimes and selection policy alone, so a future machine/session object can own compute runtimes plus richer heterogeneous coordination state
- move from scoped/triggered session scheduler policy plus cached topology/domains/progress toward richer heterogeneous runtime coordination policy
- decide what coordination state belongs with the session versus with future machine/platform session objects
- reduce or eliminate duplicated engine-level ISA bookkeeping once session-owned runtime identity fully carries execution dispatch
- keep moving per-runtime mode/session state out of `HelmEngine` where it still remains engine-owned
- connect runtime metadata to broader heterogeneous scheduling and topology decisions
- decide which progress signals remain session-local versus which should belong to a higher-level heterogeneous machine coordinator
- extend the session progress hook from scoped/triggered advancement to richer scheduler-controlled selection
- decide how coordination domains interact with roles, topology, and future heterogeneous machine objects
- decide whether progress bookkeeping remains purely observational or begins feeding higher-level heterogeneous scheduling policy
- decide whether machine-owned policy feedback should remain advisory or start driving scheduler policy selection
- extract any ISA-owned helper/state-transition logic discovered during that work back into `helm-arch`
- keep orchestration, session, syscall integration, and platform boot logic in `helm-engine`

### Recommended next implementation order

1. Expand session ownership beyond runtimes and selection policy, so heterogeneous coordination has a real home.
2. Expand the session scheduler from scoped/triggered policy handling plus cached topology/domains/progress into richer heterogeneous runtime coordination semantics.
3. Decide how multi-runtime coordination is partitioned between session ownership and higher-level machine/session objects.
4. Reduce or eliminate duplicated engine-level ISA bookkeeping where session-owned runtime identity can be authoritative.
5. Keep moving per-runtime mode/session state out of `HelmEngine` where appropriate.
6. Connect runtime metadata to heterogeneous scheduling/topology decisions beyond the current CPU-vs-sidecar split.
7. Decide whether to wire machine-owned policy feedback back into scheduler policy selection, or introduce coordination groups first, while keeping the hot path unchanged.
8. Move any ISA-owned helpers discovered during that work back into `helm-arch`.
9. Keep platform/session/syscall orchestration in `helm-engine`.
10. Revisit deeper `MemoryMap` / `HelmAddressSpace` convergence after the runtime/session shape is established.

### Resume checklist

When resuming, start from:

1. this document
2. the worktree branch state
3. current `git diff` in the worktree
4. targeted crate tests before new edits

Suggested first commands:

```bash
git -C /home/pmallapp/proj/personal/helm-ng/.claude/worktrees/perf-safe-api status --short
git -C /home/pmallapp/proj/personal/helm-ng/.claude/worktrees/perf-safe-api diff -- runtime/helm-engine runtime/helm-memory runtime/helm-platform runtime/helm-python
```

### Verification status

The following checks passed during the current refactor series:

- `cargo test -p helm-platform --lib`
- `cargo test -p helm-memory --lib`
- `cargo test -p helm-engine --lib`
- `cargo test -p helm-python --lib`

### Known environment issue

`cargo test -p helm-engine` may still fail in integration tests that expect external/untracked binary assets, especially paths under `assets/binaries/`. That is an environment/input issue rather than a known regression from the architecture refactor slices above.

## Final Recommendation

The repo does not need a broad rewrite. It needs a boundary cleanup.

If the project:

- protects the hot path,
- converges duplicate control-plane abstractions,
- makes build-vs-run explicit,
- and keeps docs tied to reality,

then API quality, extension ergonomics, and long-term maintainability will improve substantially without compromising performance.
