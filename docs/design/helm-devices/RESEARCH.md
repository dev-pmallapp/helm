# helm-devices — Design Question Research

> Research findings for all 40 design questions across 4 domains.
> For each question: Industry Standard approach, Creative approach, Pros/Cons, and Helm-NG Suitability.
>
> Cross-references: [DESIGN-QUESTIONS.md](./DESIGN-QUESTIONS.md) · [HLD.md](./HLD.md) · [LLD-device-trait.md](./LLD-device-trait.md)
>
> **Sources consulted**: QEMU source (hw/, target/arm/), gem5 (src/dev/, src/arch/arm/),
> ARM Fast Models documentation, SystemC TLM-2.0 standard, svd2rust, embassy, abi_stable crate,
> Intel SIMICS documentation, Renode framework, DynamoRIO, Valgrind.

---

## Domain 1: Device Modeling Infrastructure

## Q1.1 -- Interior Mutability Strategy for `Device::read(&self)`

### Industry Standard

**QEMU (C):** The `MemoryRegionOps` structure passes the device state as an opaque `void *opaque` pointer to both `read` and `write` callbacks. Since C has no `const`-correctness enforcement on opaque pointers, the `read` callback freely mutates device state for clear-on-read registers (e.g., ivshmem ISR clears on read). There is no aliasing protection -- the BQL serializes all MMIO access, so concurrent mutation is impossible. The signature is effectively `uint64_t read(void *opaque, hwaddr addr, unsigned size)` -- mutable by convention, safe by the BQL.

**gem5 (C++):** The `PioDevice::read(PacketPtr pkt)` method takes a mutable `this` pointer (no const). Every MMIO read has mutable access to the device. gem5 does not distinguish `read` from `write` at the const-correctness level. The `RegisterBank` framework's `read()` method also takes non-const `this`.

**svd2rust (Rust):** Memory-mapped registers use `VolatileCell<u32>` (which wraps `UnsafeCell`) at the bottom layer. The generated `read()` method on a register takes `&self` but performs a volatile read through `UnsafeCell`. For clear-on-read registers, the hardware itself performs the clear on the physical read operation -- svd2rust does not need to model mutation because the hardware does it. However, svd2rust [issue #540](https://github.com/rust-embedded/svd2rust/issues/540) documents that W1C/clear-on-read semantic handling remains a pain point with counterintuitive `modify()` behavior.

**embassy (Rust):** Uses HAL-level ownership where peripherals are moved into drivers via Rust's move semantics. Shared access is via `critical_section::Mutex<RefCell<T>>` for interrupt-context sharing, or `embassy_sync::Mutex` for async tasks. The [Embedded Rust Book](https://docs.rust-embedded.org/book/concurrency/) documents this as the standard pattern.

### Creative Approach

**Split-signature with phantom marker**: Instead of one `Device` trait, define two levels:

```rust
pub trait DeviceRead {
    fn read(&self, offset: u64, size: usize) -> u64;
}
pub trait DeviceWrite {
    fn write(&mut self, offset: u64, size: usize, val: u64);
}
```

Devices without clear-on-read implement `DeviceRead` with a pure `&self`. Devices with stateful reads implement `DeviceRead` using `Cell<u32>` for their register backing (all register fields are `Copy` types -- `u32`/`u64` -- so `Cell` works). The `MemoryMap` dispatch calls `DeviceRead::read(&device)` and `DeviceWrite::write(&mut device)` through different dispatch paths. This avoids `RefCell` runtime overhead and avoids `&mut self` on `read()`.

The key insight: register banks are always `Copy`-type fields (`u32`, `u64`). FIFOs (`VecDeque<u8>`) are the only non-`Copy` device state that might mutate on read. FIFOs can be wrapped in `RefCell` individually rather than wrapping the entire device.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **`&mut self` on `read()`** | Simplest, no interior mutability needed, no runtime checks, matches gem5/QEMU semantics | Forces exclusive borrow for reads; `World::mmio_read(&self)` cannot call `read(&mut device)` without the World itself being `&mut`; prevents concurrent read observers (HelmEventBus callbacks during read) |
| **`Cell<u32>` per register field** | Zero runtime cost, no `RefCell` borrow panics, works for all register fields (all are `Copy`) | Cannot wrap FIFOs; device author must remember to use `Cell`; slightly unusual Rust ergonomics (`cell.get()`/`cell.set()` vs direct field access) |
| **`RefCell<RegisterBank>`** | Works for everything including FIFOs; standard Rust interior mutability | Runtime borrow check on every MMIO read in hot loop; panics if re-entrant read from HelmEventBus callback; debug builds add overhead |
| **`UnsafeCell` with documented invariant** | Zero overhead; maximum flexibility | Requires `unsafe` blocks in every device; manual aliasing safety arguments; breaks if HelmEventBus callback re-enters |

### Helm-NG Suitability

**Recommended: `&mut self` on `read()`.** Change the `Device` trait signature from `fn read(&self, ...)` to `fn read(&mut self, ...)`.

Rationale grounded in helm-ng's actual types:

1. The current `Device::read(&self, ...)` in `crates/helm-devices/src/lib.rs` line 29 is at odds with the `register_bank!` macro, which generates `mmio_read(&mut self, ...)` (documented in LLD-register-bank-macro.md section 10). The LLD even notes this gap: "The outer `Device::read(&self, ...)` delegates via interior mutability... The most pragmatic approach for Phase 0: make `Device::read()` take `&mut self`."

2. `World::mmio_read(&self, ...)` in LLD-object-model.md currently takes `&self` on World, but `World::mmio_write(&mut self, ...)` takes `&mut self`. Making `mmio_read` also take `&mut self` is consistent -- the World is already exclusively borrowed during simulation step dispatch (single-threaded hot loop, no concurrent reads by design rule 8: "Determinism by default -- no wall-clock, no background threads in the hot loop").

3. `Cell<u32>` is a reasonable alternative for Phase 0 since all register bank fields are `u32`/`u64`. But it adds ergonomic friction for every device author and does not help with FIFO state (`VecDeque<u8>` in `Uart16550`'s `rx_fifo`/`tx_fifo`). `&mut self` is simpler and matches every other simulator's approach.

4. For future multi-hart (Q1.9), the `MemoryMap` dispatch layer serializes device access (one hart accesses a device at a time), so `&mut self` remains correct.

---

## Q1.2 -- Banked Register Sets Selected by Control Register

### Industry Standard

**QEMU:** Banking is handled manually in `MemoryRegionOps` callbacks. The UART 16550 `serial_ioport_read()` function checks `s->lcr & 0x80` (DLAB bit) and dispatches to either the normal register or the divisor latch. There is no macro or framework for banking -- every device implements its own switch logic. For the GIC, per-CPU banking is handled by the `gic_get_current_cpu()` function that indexes into per-CPU arrays based on the accessing CPU ID.

**gem5:** The GIC PL390 model maintains [per-CPU banked state](https://m5-dev.m5sim.narkive.com/AuFyUsKV/gem5-dev-changeset-in-gem5-arm-bank-gic-registers-per-cpu) (`bankedRegs`, `cpuPpiActive`, `cpuPpiPending`, etc.) alongside shared state (`intEnabled`, `intPriority`). Banking is implemented as manual per-CPU array indexing in the `read()`/`write()` methods, not through `RegisterBank` type-level dispatch.

**SIMICS DML:** DML supports register groups and arrays natively. A register bank can contain `group` declarations with `parameter index_offset` to create indexed register arrays. For per-CPU banking, DML uses `bank` declarations with `parameter mapped_registers` that can be parameterized by the accessing processor context. [DML 1.4](https://github.com/intel/device-modeling-language/wiki/3.-DML-1.4) provides `saved` variables and `unmapped` registers for internal state that is automatically checkpointed.

**svd2rust:** SVD has `<cluster>` elements that describe register arrays and repeated groups (e.g., `<dim>4</dim>` for 4 instances of a register group at different offsets). svd2rust generates array-indexed access methods. There is no SVD concept of runtime-selected banking -- SVD describes fixed hardware, not software-selected views.

### Creative Approach

**Overlay registers with trait-based view selection**: Instead of adding a `banked_by` clause to the macro, define register overlays as separate types that share backing storage:

```rust
register_bank! {
    pub struct UartRegsNormal { reg RBR @ 0x00 is read_only; reg IER @ 0x01; ... }
    device = Uart16550;
}
register_bank! {
    pub struct UartRegsDLAB { reg DLL @ 0x00; reg DLM @ 0x01; ... }
    device = Uart16550;
}
```

The device holds `normal: UartRegsNormal` and `dlab: UartRegsDLAB` as separate fields. The `Device::read()` implementation dispatches based on `self.regs_normal.lcr_dlab()`:

```rust
fn read(&mut self, offset: u64, size: usize) -> u64 {
    if self.regs_normal.lcr_dlab() != 0 && offset <= 1 {
        self.dlab.mmio_read(offset, size, self)
    } else {
        self.regs_normal.mmio_read(offset, size, self)
    }
}
```

This keeps the macro grammar simple, makes banking explicit, and allows the two views to have completely independent register definitions, hook methods, and checkpoint fields.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Manual dispatch in `on_read_*` hooks** | Zero macro complexity; device author has full control; matches QEMU/gem5 approach | Boilerplate in every banked device; easy to forget a bank case; banking logic scattered across hooks |
| **`banked_by = CTRL.FIELD` macro clause** | DRY; compiler-checked; generates dispatch table automatically | Significant macro complexity; harder to reason about generated code; composability issues with multi-level banking (DLAB within a CPU-banked GIC) |
| **Overlay register banks (creative)** | Simple macro; explicit dispatch; separate checkpoint fields; independent hook namespaces | Two separate structs share no backing storage (DLL and RBR may share physical state in HW); device author must reconcile shared state manually; more memory per device |

### Helm-NG Suitability

**Recommended: Manual dispatch in hooks for Phase 0; overlay banks for Phase 1+.**

For Phase 0, helm-ng has only the UART 16550 as a concrete device (per AGENT.md `examples/plugin-uart/`). The DLAB mux is a 2-way switch on 2 registers -- trivially handled in `on_read_rbr_thr` and `on_write_rbr_thr` hooks checking `self.regs.lcr_dlab()`. This matches the existing LLD-register-bank-macro.md design (section 5, "the device author's on_write_thr and on_read_rbr hooks handle the DLAB muxing logic -- the macro does not model DLAB").

For the GIC (Phase 1+), the overlay approach scales better: `GicDistRegs`, `GicRedistRegs`, and `GicCpuIfRegs` would be separate `register_bank!` invocations with separate structs, which is already implied by the multi-bank design in Q1.5. The `banked_by` clause adds macro complexity disproportionate to its benefit -- banking logic varies wildly between devices (2-way DLAB, N-way per-CPU, nested combinations) and resists uniform macro treatment.

---

## Q1.3 -- MmioHandler Concrete Type vs. `dyn DeviceCallbacks` vs. Generic

### Industry Standard

**QEMU:** The `MemoryRegionOps` callback receives `void *opaque` -- the concrete device pointer cast from `void *`. No vtable dispatch; the function pointer in `MemoryRegionOps` is the dispatch mechanism. Each device defines its own `MemoryRegionOps` struct with its own read/write function pointers. This is monomorphized by construction (function pointers are per-device-type, not polymorphic).

**gem5:** `RegisterBank` is a template class (`RegisterBank<BankByteOrder>`). The `read()`/`write()` methods dispatch to individual register callbacks through a table of `Register` objects. Each `Register` can have custom `read()`/`write()` overrides. The callback mechanism is virtual dispatch (C++ `virtual` methods on `Register` subclasses), but the dispatch is per-register-type, not per-device-type.

**svd2rust:** Generated code is entirely monomorphic. Each peripheral gets its own concrete types (`USART1`, `USART2`) with no trait abstraction. `read()` and `write()` are generic methods on concrete register types. There is no reuse of register implementations across peripherals with different behavior -- each peripheral is independently generated.

### Creative Approach

**Const-generic register layout with runtime hook table**: Instead of passing `&mut ConcreteDevice` or `&mut dyn DeviceCallbacks`, generate a static dispatch table as `const` data:

```rust
// Generated by register_bank!
const UART_DISPATCH: &[RegEntry] = &[
    RegEntry { offset: 0x00, read: |bank, dev| { ... }, write: |bank, dev, v| { ... } },
    RegEntry { offset: 0x01, read: |bank, dev| { ... }, write: |bank, dev, v| { ... } },
    // ...
];
```

Where `RegEntry` is generic over the device type. This puts the dispatch table in `.rodata`, enables branch prediction, and avoids both vtable overhead and per-device code generation for common layouts. Devices sharing a layout but differing in hooks simply define different `const` tables.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Concrete `&mut Uart16550`** | Zero vtable overhead; full type safety; compiler can inline hook calls; IDE navigation works | Each `register_bank!` invocation generates device-specific code; no reuse across devices with same register layout |
| **`&mut dyn DeviceCallbacks`** | Reusable `MmioHandler` across devices with same layout; no device-type dependency in generated code | Vtable dispatch on every register write in hot loop; loses type safety on hook method set; trait must enumerate all possible hook methods |
| **Generic `<D: RegisterHooks>`** | Monomorphized (zero overhead); reusable definition across devices; compiler checks hook compliance | Requires an additional trait (`RegisterHooks`) independent of the macro; more complex type signatures; debugging with generic types harder |

### Helm-NG Suitability

**Recommended: Concrete `&mut ConcreteDevice` (current design).**

The LLD already specifies `mmio_write(&mut self, offset, size, val, device: &mut Uart16550)` with the concrete type. This is correct for helm-ng because:

1. **Hot-path performance** is paramount (design rule 6: "ExecContext is hot-path only... must be statically dispatched"). MMIO dispatch happens on every device register access. Vtable dispatch adds an indirect call per register write.

2. **Register layout reuse is rare in practice.** Two devices that share an identical register layout but differ in behavior (hooks) is an edge case -- real devices have different register sets. Even "compatible" devices like NS16550A vs. 16650 have different register semantics.

3. **The generic `<D: RegisterHooks>` option** adds complexity without clear benefit. The `register_bank!` macro already generates the `SimpleRegsHooks` trait (LLD-register-bank-macro.md section 10) specific to each bank's register set. Making this generic across banks with different register names is not meaningful.

4. The generated code size per device is minimal (one `match` statement with ~8-16 arms). Compile-time cost is negligible compared to ISA decoder codegen.

---

## Q1.4 -- Device Obtaining `EventQueue` Without helm-event Dependency

### Industry Standard

**QEMU:** Devices call `timer_new_ns(QEMU_CLOCK_VIRTUAL, callback, opaque)` directly. The timer API is a global facility -- there is no dependency injection or trait abstraction. The `timer.h` header is included by every device. [QEMU timer docs](https://airbus-seclab.github.io/qemu_blog/timers.html) show this as a simple function call creating a nanosecond-resolution virtual clock timer.

**gem5:** Each `SimObject` inherits from `EventManager`, which provides `schedule(event, tick)` directly. The `EventWrapper<MyDevice, &MyDevice::processEvent>` template binds a member function as the callback. There is no dependency injection -- the event scheduling facility is [built into the SimObject base class](https://www.gem5.org/documentation/learning_gem5/part2/events/).

**Renode:** Timer peripherals are constructed with `machine.ClockSource` passed as a constructor argument (dependency injection via constructor). `LimitTimer` and `ComparingTimer` take the clock source as their first parameter. This is classical constructor injection -- the clock is passed at creation time, not discovered at runtime.

**SIMICS:** DML's `after` and `event` constructs are built into the DML language. A device model writes `after (delay) call method()` and the DML compiler generates the event scheduling code. The event queue is implicit in the language runtime -- no explicit dependency.

### Creative Approach

**Trait-based timer facade in helm-core**: Define a minimal trait in `helm-core` that `helm-event`'s `EventQueue` implements:

```rust
// In helm-core (zero deps)
pub trait TimerScheduler: Send + Sync + 'static {
    fn schedule_callback(&self, delay_ticks: u64, callback: Box<dyn FnOnce() + Send>);
    fn current_tick(&self) -> u64;
}
```

Devices store `Arc<dyn TimerScheduler>` populated at `elaborate()` time. `helm-event::EventQueue` implements `TimerScheduler`. This inverts the dependency without `Any` downcasting, without breaking the `helm-devices -> helm-core only` constraint, and without the indirection of HelmEventBus translation.

The overhead of `dyn TimerScheduler` is one vtable call per timer schedule -- this is cold-path (timer scheduling happens orders of magnitude less often than MMIO reads). The `Arc` clone at elaborate time is a one-time cost.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Break dep constraint (add helm-event)** | Simplest; direct `Arc<EventQueue>` in device; no abstraction overhead | Violates the `helm-devices -> helm-core only` hard constraint from HLD; plugins must link against helm-event; circular dep risk |
| **Trait in helm-core (dependency inversion)** | Clean dependency inversion; type-safe; cold-path vtable acceptable; device ergonomics good (`self.timer.schedule_callback(...)`) | Adds a trait to helm-core; every device that needs timers stores `Arc<dyn TimerScheduler>`; slightly enlarges helm-core's API surface |
| **Opaque `Any` field at elaborate** | No new traits; maximum flexibility; device can store anything | No compile-time type checking; `Any` downcast panics at runtime if wrong type; device author must know the concrete type string |
| **HelmEventBus translation** | Reuses existing infrastructure; no new deps or traits | Synchronous bus was not designed for deferred events; the bus fires-and-forgets; translating a synchronous bus event to an async queue event requires World glue code; breaks the separation between sync observability and async scheduling |

### Helm-NG Suitability

**Recommended: Trait in helm-core (`TimerScheduler`).**

This matches the Renode pattern (constructor injection of clock source) adapted to Rust's trait system. Specifically:

1. The `ClassDescriptor::finalize` function in LLD-object-model.md already shows `let event_queue = world.event_queue().clone()` being passed to device state. The only issue is that `EventQueue` is in `helm-event`, which `helm-devices` cannot depend on.

2. A `TimerScheduler` trait in `helm-core` costs a single trait definition (~4 methods). `helm-event::EventQueue` implements it. The device stores `Arc<dyn TimerScheduler>` populated during `finalize()`. The vtable overhead is on the timer-schedule path (cold), not the MMIO path (hot).

3. This preserves the hard dependency constraint: `helm-devices -> helm-core only`. Plugins link against `helm-devices` and `helm-core`, never `helm-event`.

4. The `Any` approach is fragile -- it pushes type errors to runtime and makes device code harder to review. The trait approach provides compile-time safety.

---

## Q1.5 -- Multi-Bank Hook Borrow Conflicts

### Industry Standard

**QEMU GIC:** The GIC distributor, redistributor, and CPU interface are separate C structs (`GICv3State`, `GICv3CPUState`, `GICv3ITSState`). They share a parent `GICv3State` pointer. When writing to an enable register, the handler freely accesses priority arrays through the shared parent pointer -- C has no borrow checker. The [GIC v3 model in QEMU](https://github.com/qemu/qemu/blob/master/hw/intc/arm_gicv3_dist.c) uses a flat struct with all distributor state in one place.

**gem5 GIC (PL390):** The [PL390 model](https://pages.cs.wisc.edu/~swilson/gem5-docs/classPl390.html) stores all GIC state (enable, priority, pending, active, target) as flat arrays in a single class. `softInt`, `intEnabled`, `intPriority`, `cpuTarget` are all direct member fields. Writing to enable triggers `updateIntState()` which reads priority -- no borrow conflict because everything is in `this->`.

**ARM Fast Models:** Fast Models (SystemC-based) use a single SystemC `SC_MODULE` per GIC component. All register banks within a component share the module's member variables directly. No ownership or aliasing restrictions.

### Creative Approach

**Index-based split borrowing with a flat state vector**: Instead of separate `register_bank!` structs for enable and priority, store all GIC registers in a single flat `Vec<u32>` (or fixed array) indexed by register type and index:

```rust
struct GicState {
    regs: [u32; 256],  // flat storage
}

impl GicState {
    fn enable(&self, n: usize) -> u32 { self.regs[ENABLE_BASE + n] }
    fn priority(&self, n: usize) -> u32 { self.regs[PRIORITY_BASE + n] }
    fn set_enable(&mut self, n: usize, v: u32) {
        self.regs[ENABLE_BASE + n] = v;
        self.update_pending();  // can freely read priority
    }
}
```

The `on_write_enable` hook has `&mut self` on the `GicState` struct and can access all banks through indexed offsets. No borrow conflict because there is only one struct with one `&mut self`.

A more structured version: use `register_bank!` only for the MMIO dispatch table, but store all register values in a shared `GicSharedState` struct that both banks reference. The generated `MmioHandler` writes to `GicSharedState` rather than to separate bank structs.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Single flat struct (bypass register_bank!)** | No borrow conflicts; maximum flexibility; matches QEMU/gem5; optimal for complex devices like GIC | Loses register_bank! benefits (auto checkpoint, field accessors, Python introspection); manual MMIO dispatch |
| **Shared state struct with bank wrappers** | Banks use register_bank! for dispatch; shared state struct holds all values; hooks access shared state via `&mut self` on the device | Requires the register_bank! macro to support writing to an external struct rather than internal fields; macro complexity increases |
| **Two register_bank! with raw-ptr cross-access** | Both banks get register_bank! benefits; cross-access via raw pointer to sibling bank | `unsafe` required; fragile; violates Rust safety invariants if not carefully managed |
| **Single register_bank! with large register set** | One bank = one struct = one `&mut self`; all registers co-located | register_bank! grows very large for GIC (~100+ registers); harder to read/maintain; loses conceptual bank grouping |

### Helm-NG Suitability

**Recommended: Complex multi-bank devices (GIC, SMMU) bypass register_bank! and implement `Device::read/write` manually with a single flat state struct.**

Reasoning:

1. The GIC has ~100+ registers across distributor, redistributor, and CPU interface banks, with complex cross-bank dependencies (writing GICD_ISENABLER must re-evaluate pending state from GICD_IPRIORITYRn). This is exactly the case where register_bank!'s per-field code generation adds complexity without proportionate benefit.

2. Simple devices (UART, GPIO, timer) benefit from `register_bank!` because their register sets are small (~8-16 registers), side effects are local, and automatic checkpoint/introspection saves significant boilerplate.

3. The LLD-register-bank-macro.md already establishes that "The macro does not implement Device directly." The device author implements `Device` by delegating to the bank. For multi-bank devices, the device author skips the macro and implements `Device` directly with manual offset dispatch and a flat state struct. This is not a limitation of the macro -- it is the correct architectural boundary.

4. A future enhancement could support `state = SharedState;` in the macro grammar, where both banks write to fields on an external struct. But this is Phase 2+ work and should not delay Phase 0.

---

## Q1.6 -- ParamSchema Validation Timing

### Industry Standard

**gem5:** SimObject parameters are validated at Python instantiation time through `TypeParam` descriptors with `__set__` that perform type checking and range validation. The Python class hierarchy inherits parameter definitions. Errors surface immediately when the user writes `uart.clock_hz = "not_a_number"` in the config script.

**SIMICS:** Object attributes are validated at object creation time. DML `parameter` declarations include type information, and the Simics configuration system validates types when attributes are set. Errors occur when `SIM_set_attribute()` is called.

**QEMU:** QOM (QEMU Object Model) properties use getter/setter functions with type-checked property descriptors. Properties can be set at any time before realize, but `realize()` performs additional validation (e.g., checking that required properties are set). QEMU distinguishes between type validation (at set time) and semantic validation (at realize time).

**pydantic:** [Pydantic v2](https://docs.pydantic.dev/latest/concepts/validators/) uses `model_validator(mode='before')` for pre-construction validation and `@field_validator` for per-field checks. Validation runs at instantiation time -- errors surface immediately. The Rust-based `pydantic-core` engine (built on PyO3) makes validation fast.

### Creative Approach

**Dual-phase validation with Python descriptor protocol**: Use Python's `__set_name__`/`__set__` descriptor protocol on the generated Python classes, with the `ParamSchema` providing the validation rules. This works without the Python class string needing schema awareness:

```python
class TypedParam:
    def __init__(self, schema_field):
        self.field = schema_field
    def __set__(self, obj, value):
        if not self.field.validate(value):
            raise ValueError(f"{self.field.name}: {self.field.error_msg(value)}")
        obj.__dict__[self.field.name] = value
```

When the `DeviceDescriptor` is registered, the Python class is dynamically augmented with `TypedParam` descriptors for each field in the `ParamSchema`. Assignment-time validation happens through Python's descriptor protocol. `realize()` then performs cross-field semantic validation (e.g., "baud_divisor must be > 0 when fifo_enabled is true").

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Assignment-time only** | Immediate error feedback; matches gem5/SIMICS; best UX for config script authors | Requires Python class to be schema-aware; cannot validate cross-field constraints at assignment time (field A's validity depends on field B's value); complicates PyO3 class injection |
| **`realize()` time only** | Simplest implementation; Python class is dumb data container; all validation in Rust | Late error detection; user discovers errors only at `build_simulator()` time; multiple errors reported simultaneously (confusing) |
| **Both (assignment + realize)** | Best of both worlds; type/range at assignment, semantic at realize; matches QEMU model | Most complex; dual validation code paths; must keep validation logic consistent between Python descriptors and Rust `realize()` |

### Helm-NG Suitability

**Recommended: Type/range validation at assignment time (Python descriptors); cross-field semantic validation at `realize()` time.**

1. The Python config API in AGENT.md shows `Uart16550(clock_hz=1_843_200)` -- the user expects Python-level type checking. Writing `Uart16550(clock_hz="hello")` should fail immediately, not at `build_simulator()`.

2. The `DeviceDescriptor` already has both `param_schema: fn() -> ParamSchema` and `python_class: &str`. The loader can parse the `ParamSchema` at registration time and inject `TypedParam` descriptors into the Python class -- this does not require the embedded Python class string to contain validation logic.

3. Cross-field validation belongs in `realize()` because field dependencies cannot be expressed in individual descriptors. This is where QEMU's model is correct -- property setters check types, `realize()` checks semantics.

4. Using pydantic `BaseModel` as the Python config class is an option but adds a heavyweight dependency. The descriptor approach achieves the same early-validation UX with minimal machinery.

---

## Q1.7 -- Registers Wider Than 32 Bits

### Industry Standard

**gem5:** `RegisterBank` is templated on the register data type: `Register<uint32_t>`, `Register<uint64_t>`. Each register in the bank can have a different width. The `RegisterBank` template handles mixed-width registers by storing each register's data in its declared type and handling byte-order conversion per-register.

**QEMU:** `MemoryRegionOps` specifies `.impl.min_access_size` and `.impl.max_access_size`. The ops structure can declare access sizes of 1/2/4/8 bytes. For 64-bit registers (e.g., ARM TTBR), the device handler receives a `uint64_t` value regardless of the declared register width. Register backing storage is simply `uint64_t` fields in the device state struct.

**svd2rust:** The SVD `<size>` element specifies per-register width (8/16/32/64 bits). svd2rust generates the appropriate type: `ReadWrite<u8>`, `ReadWrite<u16>`, `ReadWrite<u32>`, or `ReadWrite<u64>`. The generated register type matches the SVD declaration. Mixed-width banks are supported natively.

**SIMICS DML:** Each register in a DML bank has an explicit `size` parameter (in bytes). A 64-bit register declares `parameter size = 8;`. DML supports 1/2/4/8-byte registers within the same bank, with automatic byte-order handling.

### Creative Approach

**Always `u64` internally; width annotation for masking and hook signatures**: Instead of per-register generic types, store everything as `u64` but generate field accessors and masks based on a declared width:

```rust
register_bank! {
    pub struct TimerRegs {
        reg CTRL   @ 0x00 width 32 { field ENABLE [0]; }
        reg CVAL   @ 0x08 width 64 { field VALUE [63:0]; }
        reg STATUS @ 0x10 width 32 is clear_on_read { field EXPIRED [0]; }
    }
    device = Timer;
}
```

Internally, `ctrl: u64`, `cval: u64`, `status: u64`. But the generated `on_write_ctrl` hook receives `(old: u32, new: u32)` (narrowed from `u64`), the mask for CTRL is `0xFFFF_FFFF`, and checkpoint serialization writes 4 bytes for CTRL but 8 bytes for CVAL.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Per-register width qualifier (`width 32`/`width 64`)** | Explicit; hook signatures match register width; serde format is space-efficient; matches SVD/DML model | Changes generated struct field types; `u64` and `u32` hooks have different signatures; more complex macro codegen |
| **Per-bank default width** | Simple configuration; consistent hook signatures within a bank | Cannot mix 32-bit and 64-bit registers in the same bank; ARM system register banks commonly mix widths |
| **Always u64 internally** | Uniform storage; simple codegen; one hook signature `(old: u64, new: u64)` for everything | Wastes 4 bytes per 32-bit register; hook signature does not reflect hardware width; masks must be applied manually or generated |

### Helm-NG Suitability

**Recommended: Per-register `width` qualifier defaulting to 32.**

1. The current macro generates `u32` fields (LLD-register-bank-macro.md: "a flat struct of `u32` fields"). ARM system registers like TTBR0_EL1 are 64 bits. AXI DMA descriptor addresses are 64 bits. A `width 64` qualifier is the minimal extension.

2. The generated hook signatures should match the width: `on_write_ctrl(&mut self, old: u32, new: u32)` for width-32, `on_write_cval(&mut self, old: u64, new: u64)` for width-64. This avoids device authors receiving a `u32` for a 64-bit register and losing the upper bits.

3. The default should be `width 32` (matching the current behavior) for backward compatibility. Only registers that declare `width 64` get the wider type.

4. For serde, the field type in the generated struct determines the serialization width. A `u32` field serializes to 4 bytes in bincode; a `u64` field to 8 bytes. This is automatic and correct.

5. The "always u64" approach is simpler to implement but makes every hook signature `(old: u64, new: u64)` even for 8-bit UART registers. This loses type information and forces device authors to mask manually. The per-register qualifier is worth the additional macro complexity.

---

## Q1.8 -- Checkpoint Format Compatibility Contract

### Industry Standard

**QEMU:** The [VMState framework](https://www.qemu.org/docs/master/devel/migration/main.html) uses `version_id` and `minimum_version_id` fields in `VMStateDescription`. Adding a new field to a device's vmstate requires bumping `version_id`. The `load_state` function receives the version and can conditionally load old-format checkpoints. The wire format is raw big-endian values with no self-describing metadata -- field order matters, field names are not in the stream. QEMU maintains forward migration compatibility (v_n -> v_n+1).

**SIMICS:** [DML attributes are automatically checkpointed](https://intel.github.io/tsffs/simics/model-builder-user-guide/programming-with-dml.html) by the Simics configuration system. Each attribute has a name, and the checkpoint format uses named key-value pairs. Adding a new register adds a new named attribute -- old checkpoints that lack the attribute get the default value on restore. Field reordering does not break compatibility because attributes are name-keyed. Removal of an attribute causes a warning but does not fail (the orphaned attribute value is ignored).

**gem5:** Checkpoint format uses Python-like text serialization with `serialize()`/`unserialize()` methods. Each field is named (`SERIALIZE_SCALAR(intEnabled)`). Adding a field requires adding a new `SERIALIZE_SCALAR` call. Restoring an old checkpoint with a new field uses `optParamIn()` to provide a default if the field is absent. Named fields make reordering safe.

### Creative Approach

**Schema hash with optional migration closures**: Generate a compile-time hash of the register bank's field names, types, and order. Store this hash in the checkpoint header. On restore, compare hashes:

```rust
// Generated by register_bank! at compile time
const UART_REGS_SCHEMA_HASH: u64 = const_fnv1a_hash("rbr:u32,thr:u32,ier:u32,...");

impl Uart16550Regs {
    fn checkpoint_header(&self) -> CheckpointHeader {
        CheckpointHeader {
            schema_hash: UART_REGS_SCHEMA_HASH,
            version: CKPT_VERSION,
        }
    }
}
```

If the hash matches, deserialize directly (fast path). If the hash mismatches, look up a registered migration closure `fn migrate(old_version: u32, data: &[u8]) -> Vec<u8>`. If no migration exists, fail with a clear error naming the changed fields (the hash includes field names, so a diff can be computed).

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Manual version_id (current design)** | Simple; device author controls compatibility; matches QEMU pattern | Error-prone (device author forgets to bump version); no automatic detection of breaking changes; no field-level migration |
| **Self-describing format (serde_json/postcard)** | Field reordering safe; missing fields get `#[serde(default)]`; human-readable (JSON) | Larger checkpoint size; slower serialization; JSON not suitable for multi-GB checkpoint streams; postcard is more compact but still carries field names |
| **Schema hash + migration (creative)** | Automatic detection of breaking changes; fast path for compatible checkpoints; migration closures for evolution | More complex infrastructure; hash must be stable across compilations; migration closure development burden |
| **bincode with version tag (current impl)** | Fast; compact; works for Phase 0 | Non-self-describing; field order changes break compatibility; no automatic field addition |

### Helm-NG Suitability

**Recommended: bincode with manual `CKPT_VERSION` for Phase 0; schema-hash detection for Phase 2+.**

1. Phase 0 has minimal device state (one UART). Manual version tagging is sufficient and matches the current LLD design.

2. For Phase 2+ (when checkpoint compatibility across releases matters), adopt the schema-hash approach: the `register_bank!` macro computes a compile-time hash of field names+types and embeds it in the checkpoint header. This automatically detects incompatible checkpoints without device author action.

3. The serialization format should remain bincode for performance. The schema hash provides the compatibility detection that bincode's non-self-describing format lacks. This gives the speed of bincode with the safety of self-describing formats.

4. `#[serde(default)]` on generated structs provides backward compatibility for field addition (new fields get defaults). This is trivial to add to the macro's `#[derive(Deserialize)]` output and handles the most common migration case (adding a register field to a device).

5. Field removal (breaking change) should require a version bump. The schema hash makes this detectable.

---

## Q1.9 -- Thread Safety for Multi-Hart Parallel Simulation

### Industry Standard

**QEMU:** The [Big QEMU Lock (BQL)](https://www.qemu.org/docs/master/devel/multi-thread-tcg.html) serializes all device access. Each vCPU runs on its own thread but acquires the BQL before any MMIO access. Recent work (2025) introduces [BQL-free fine-grained MMIO](https://www.mail-archive.com/qemu-devel@nongnu.org/msg1130140.html) for specific devices (ACPI PM timers, HPET) that implement their own locking via `memory_region_enable_lockless_io()`. This is opt-in per device.

**gem5:** In multi-threaded timing mode (gem5.opt with `-DUSE_POSIX=True`), each CPU runs on its own event queue thread. Device access is [serialized per-object via mutex](https://gem5.googlesource.com/public/gem5/+/refs/heads/master/src/dev/pci/device.hh). The GIC model uses atomic operations for per-CPU state and mutex for shared distributor state.

**SIMICS:** Uses serialized event delivery -- all events are processed in a single thread per cell (a group of processors that share a virtual time domain). Cross-cell communication is serialized. Devices never see concurrent access within a cell.

**embassy/RTIC (Rust):** Embassy uses `critical_section::Mutex<RefCell<T>>` for shared peripheral access between tasks/interrupts. RTIC uses Stack Resource Policy (SRP) -- priority ceiling protocol where acquiring a resource temporarily raises the task priority, preventing preemption by tasks that use the same resource. Both approaches make shared access [`Sync`](https://docs.rust-embedded.org/book/concurrency/) through compile-time or runtime guarantees.

### Creative Approach

**MemoryMap-layer serialization with per-device `SeqCst` ticket**: Instead of making devices `Sync` or adding per-device mutexes, serialize access at the `MemoryMap` dispatch layer using a per-device atomic ticket lock:

```rust
struct DeviceSlot {
    device: UnsafeCell<Box<dyn Device>>,
    ticket: AtomicU64,
    serving: AtomicU64,
}

impl DeviceSlot {
    fn with_mut<R>(&self, f: impl FnOnce(&mut dyn Device) -> R) -> R {
        let my_ticket = self.ticket.fetch_add(1, Ordering::Relaxed);
        while self.serving.load(Ordering::Acquire) != my_ticket { core::hint::spin_loop(); }
        let result = f(unsafe { &mut *self.device.get() });
        self.serving.store(my_ticket + 1, Ordering::Release);
        result
    }
}
```

Devices remain `!Sync`. The `MemoryMap` provides serialized access. No `Mutex` overhead (ticket lock is lighter), no `RefCell` (no runtime borrow checking), and the device author writes code as if single-threaded. The serialization happens at the dispatch boundary, invisible to device implementations.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **MemoryMap-level serialization (QEMU BQL model)** | Devices stay `!Sync`; device code is simple; matches QEMU's proven approach; no device author burden | Global lock = contention bottleneck for multi-hart workloads with shared devices; serializes all device access even when devices are accessed by only one hart |
| **Per-device Mutex** | Fine-grained locking; no global bottleneck; device is `Sync` | Lock overhead on every MMIO access; deadlock risk if device A's handler calls device B; device author must think about concurrency |
| **Per-device ticket lock at dispatch (creative)** | Lighter than Mutex; FIFO fairness; devices remain `!Sync`; no device author burden; dispatch-layer concern | Spin-waiting wastes CPU if contention is high; `UnsafeCell` requires careful safety argument; slightly more complex dispatch layer |
| **`!Sync` + one hart per device** | Simplest; Rust type system enforces safety; no locking overhead | Artificially limits multi-hart simulation; shared devices (UART, GIC) cannot be accessed by multiple harts; unrealistic |

### Helm-NG Suitability

**Recommended: MemoryMap-level serialization (BQL equivalent) for Phase 0-2; per-device opt-in fine-grained locking for Phase 3+.**

1. Design rule 8 mandates "Determinism by default -- no wall-clock, no background threads in the hot loop." Phase 0-2 is single-threaded. Multi-hart parallelism is a Phase 3+ concern.

2. The `Device` trait currently requires `Send` but not `Sync` (`pub trait Device: Send`). This is correct -- devices can be moved between threads but not accessed concurrently.

3. For Phase 3+ multi-hart, the `MemoryMap` dispatch layer should serialize device access per-device (not globally). This matches QEMU's latest direction (per-device `lockless_io` opt-in rather than global BQL). The ticket-lock approach at the dispatch layer keeps devices `!Sync` and device authors unaware of threading.

4. The `InterruptPin` in `crates/helm-devices/src/lib.rs` already uses `AtomicBool` for the pin state and `Arc<dyn InterruptSink + Send + Sync>` for the sink -- this is correctly thread-safe for interrupt assertions from any thread.

---

## Q1.10 -- Write-1-to-Clear Hook Contract

### Industry Standard

**gem5:** The `RegisterBank` framework's W1C registers apply the W1C logic (new = old & ~written) before invoking the callback. The hook sees the post-W1C value. This is documented in the gem5 `reg_bank.hh` implementation where W1C register types override `write()` to apply the clear mask.

**QEMU:** Device handlers implement W1C manually. The pattern is: `old = reg; reg = old & ~val; handler(old, reg);`. The handler sees both the old value and the post-W1C value. The raw write value is available as `val` but is not passed to a separate hook -- the handler itself performs the W1C arithmetic. For example, the GIC's GICD_ICPENDR handler clears pending bits by ANDing with the complement of the written value.

**svd2rust:** The [W1C handling issue](https://github.com/rust-embedded/svd2rust/issues/540) documents that svd2rust does NOT handle W1C differently from normal fields. `modify()` on a register with mixed RW and W1C fields will accidentally clear W1C bits that the programmer did not intend to touch. The `modifiedWriteValues = oneToClear` SVD attribute exists but is rarely used in vendor SVDs. svd2rust treats W1C as a known pain point.

**SystemRDL:** The [SystemRDL standard](https://github.com/orgs/SystemRDL/discussions/270) defines W1C/W1S field types with explicit semantics: on write, the hardware applies the W1C logic. The register model generates both the raw write value and the computed post-write value.

### Creative Approach

**Three-argument hook with write intent enum**: Instead of `(old, new)`, pass `(old, raw_write, computed_new)` along with a write-intent discriminator:

```rust
enum WriteAction {
    Normal,
    Write1ToClear,
    Write1ToSet,
    Write0ToClear,
}

fn on_write_status(&mut self, old: u32, raw: u32, computed: u32, action: WriteAction) {
    // For tracing: raw tells you what was written (0x05)
    // For state: computed tells you the result (0x02)
    // For debugging: action tells you the semantic (W1C)
    log::trace!("STATUS: wrote {raw:#x} (W1C) -> {old:#x} -> {computed:#x}");
}
```

This gives hooks complete information without requiring the hook to re-derive the W1C logic. Tracing tools see the raw write value. State management uses the computed value. The hook signature is slightly wider but fully informative.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Hook sees raw write value** | Hook can log exactly what was written; distinguishes "clear bit 0" from "set register to 0x02"; tracing-friendly | Hook must re-implement W1C logic to get the actual new register value; error-prone for device authors |
| **Hook sees post-W1C value** | Hook works with the final register state; simpler device logic; matches gem5 | Cannot reconstruct what was actually written; tracing tools lose information; cannot distinguish intentional writes from W1C side-effects |
| **Three-argument (old, raw, computed)** | Complete information; no re-derivation needed; tracing-friendly; hooks can choose which value they care about | Wider hook signature; most hooks only need `(old, new)` and ignore `raw`; API complexity for simple devices |

### Helm-NG Suitability

**Recommended: Hook receives `(old: u32, new: u32)` where `new` is the post-W1C value, with the raw write value available via a separate accessor.**

Specifically:

1. The generated `mmio_write` for a W1C register should:
   - Compute `computed = old & !written`
   - Store `computed` in the register field
   - Call `device.on_write_status(old, computed)`
   - Store the raw write value in a transient field `self.last_write_raw: u32` accessible to tracing

2. This matches gem5's approach (hook sees post-W1C value) while preserving the raw value for `HelmEventBus::MemWrite` tracing (which already logs `val` -- the raw value written by the CPU).

3. The `HelmEventBus::MemWrite { addr, size, val, cycle }` event already fires the raw CPU write value. The hook contract does not need to duplicate this for tracing -- the event bus handles it.

4. Device authors working with W1C registers care about the resulting state ("which interrupts are still pending?"), not the raw write ("what did the CPU write?"). Passing post-W1C value to the hook is the ergonomically correct choice.

5. The macro grammar already supports `is write_1_to_clear` (LLD-register-bank-macro.md section 2). The generated code just needs to apply the W1C transform before calling the hook and storing the result.

---

Sources referenced across this analysis:

- [QEMU Memory API](https://www.qemu.org/docs/master/devel/memory.html)
- [QEMU Internals: Memory Regions](https://airbus-seclab.github.io/qemu_blog/regions.html)
- [QEMU Internals: Timers](https://airbus-seclab.github.io/qemu_blog/timers.html)
- [QEMU Migration Framework](https://www.qemu.org/docs/master/devel/migration/main.html)
- [QEMU Multi-threaded TCG](https://www.qemu.org/docs/master/devel/multi-thread-tcg.html)
- [QEMU BQL-free MMIO patch](https://www.mail-archive.com/qemu-devel@nongnu.org/msg1130140.html)
- [gem5 Event-driven Programming](https://www.gem5.org/documentation/learning_gem5/part2/events/)
- [gem5 GIC per-CPU banking](https://m5-dev.m5sim.narkive.com/AuFyUsKV/gem5-dev-changeset-in-gem5-arm-bank-gic-registers-per-cpu)
- [gem5 IDE RegisterBank conversion](https://www.mail-archive.com/gem5-dev@gem5.org/msg36726.html)
- [svd2rust W1C issue #540](https://github.com/rust-embedded/svd2rust/issues/540)
- [svd2rust interior mutability discussion](https://users.rust-lang.org/t/why-are-memory-mapped-registers-implemented-with-interior-mutability/116119)
- [Intel SIMICS DML programming](https://intel.github.io/tsffs/simics/model-builder-user-guide/programming-with-dml.html)
- [Intel DML 1.4](https://github.com/intel/device-modeling-language/wiki/3.-DML-1.4)
- [Embedded Rust Book: Concurrency](https://docs.rust-embedded.org/book/concurrency/)
- [Embassy shared data](https://dev.to/theembeddedrustacean/sharing-data-among-tasks-in-rust-embassy-synchronization-primitives-59hk)
- [RTIC vs Embassy comparison](https://www.willhart.io/post/embedded-rust-options/)
- [Pydantic Validators](https://docs.pydantic.dev/latest/concepts/validators/)
- [Renode NRF52840 RTC timer](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/Timers/NRF52840_RTC.cs)
- [SystemRDL W1C/W1S discussion](https://github.com/orgs/SystemRDL/discussions/270)
The analysis above covers all 10 questions (Q1.1 through Q1.10) with the four requested sections per question. Key files that were central to this analysis:

- `/home/pmallapp/proj/personal/helm-ng/crates/helm-devices/src/lib.rs` -- the current `Device` trait definition showing `read(&self, ...)` signature that Q1.1 addresses
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/LLD-device-trait.md` -- the full Device trait contract including the acknowledged gap between `&self` on `read()` and `&mut self` needed by `register_bank!`
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/LLD-register-bank-macro.md` -- the complete macro grammar, generated code patterns, W1C/clear-on-read qualifiers, and hook signatures
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/HLD.md` -- the hard dependency constraint (`helm-devices -> helm-core only`) central to Q1.4
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/LLD-object-model.md` -- World's `finalize()` lifecycle where devices obtain `EventQueue` refs (Q1.4) and the `ClassDescriptor` registration pattern
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/DESIGN-QUESTIONS.md` -- the original design questions providing full context for each question
- `/home/pmallapp/proj/personal/helm-ng/AGENT.md` -- design rules, crate map, and phase plan that constrain every recommendation

---

## Domain 2: Built-in SoC Devices (ARM-centric)

## Q2.1 -- GIC-v3 Distributor/Redistributor/ITS: Separate Devices or Monolith

### Industry Standard

**QEMU** implements GIC-v3 as a **single QOM device** (`arm_gicv3`) that internally creates multiple `MemoryRegion` children. The `GICv3State` structure owns a single `MemoryRegion iomem_dist` for the Distributor (64KB) and an array of `GICv3RedistRegion` structs for Redistributors. Each `GICv3RedistRegion` contains its own `MemoryRegion`, a backpointer to `GICv3State`, and a `cpuidx` field to identify which CPU it serves. The key mechanism: MMIO reads/writes dispatch through `gicv3_dist_read`/`gicv3_redist_read` MemoryRegionOps, both operating directly on the shared `GICv3State` -- no locking, because QEMU is single-threaded per-vCPU in the main loop. The ITS (`arm_gicv3_its.c`) is a separate QOM device but holds a pointer back to `GICv3State` for LPI table access. Recent patches (Peter Maydell, 2021; Francisco Iglesias/Luc Michel, 2025) added `GICv3RedistRegion` to support non-contiguous redistributor address ranges and `first-cpu-index` for multi-cluster GIC instances.

**gem5** takes the opposite approach: `Gicv3` is a parent SimObject that instantiates **separate C++ objects** -- `Gicv3Distributor`, `Gicv3Redistributor` (one per PE), and `Gicv3CPUInterface`. These objects share state through **mutual raw pointers** (backpointers to the parent `Gicv3` and each other). The `Gicv3::read()` method checks whether the address falls in `distRange` or `redistRange` and dispatches to the appropriate sub-object's `read()`. The separate objects can directly access each other's fields (e.g., `Gicv3Redistributor` reads `distributor->DS` for group configuration). No `Mutex`; gem5 is single-threaded.

**ARM Fast Models** uses a single SystemC component (`GICV3`) with internal sub-modules exposed as separate register interfaces.

### Creative Approach

**Facade pattern with interior mutability**: A single `GicV3` struct owns all state in a flat `GicState` struct. Three thin wrapper types (`GicDistributor`, `GicRedistributor`, `GicIts`) each hold `Arc<UnsafeCell<GicState>>` and implement `Device`. Each wrapper implements `read()`/`write()` operating on offset-appropriate fields of the shared `GicState`. Since helm-ng's hot loop is single-threaded, `UnsafeCell` (or `Cell` for scalar fields) avoids `Mutex` overhead entirely. The `unsafe` is encapsulated within the GIC module and documented as "safe because the simulation is single-threaded during RUN phase." This gives three separate `Device` impls (so `MemoryMap` maps them independently at GICD, GICR, and GITS addresses), with zero synchronization cost.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Monolithic device** (QEMU-style) | Zero sharing overhead; all state local; simple checkpoint; matches QEMU's proven pattern | `region_size()` API returns single u64 -- cannot represent 3 non-contiguous MMIO frames; requires `MemoryMap` API extension for multi-region devices; one giant struct is harder to test |
| **Separate devices, Arc\<Mutex\<GicState\>\>** (naive gem5-style) | Natural mapping to MemoryMap (each device has its own base/size); per-component testing; mirrors HW boundaries | Mutex contention on every redistributor read that consults distributor state (GICD_CTLR.ARE_NS check); lock ordering between GICD and GICR; checkpoint must serialize shared state exactly once |
| **Separate devices, Arc\<UnsafeCell\<GicState\>\>** (creative facade) | Three independent `Device` impls, each with own `region_size()`; zero lock overhead; clean MemoryMap mapping; shared state is just a pointer dereference | Requires `unsafe`; cannot support future multi-threaded MMIO without adding synchronization; `UnsafeCell` demands careful documentation of safety invariant |

### Helm-NG Suitability

The facade pattern with `Arc<UnsafeCell<GicState>>` is the right fit for helm-ng. The rationale:

1. **MemoryMap constraint**: helm-ng's `Device::region_size()` returns a single `u64`. The GICD (64KB), each GICR (128KB per PE), and ITS (128KB) are at non-contiguous addresses. Making them separate `Device` impls is the only option that avoids extending `MemoryMap` to support multi-region devices.

2. **No hot-loop locking**: Helm-ng's design rule #6 (no dynamic dispatch or overhead in hot loop) and rule #8 (determinism by default, no background threads) mean the simulation is single-threaded during `RUN`. `UnsafeCell` is safe here and avoids `Mutex` overhead on every interrupt delivery.

3. **`SimObject` lifecycle**: Each sub-device (GICD, GICR, ITS) implements both `Device` and `SimObject`. The shared `GicState` is owned by a parent `GicV3` coordinator object that also implements `SimObject` and handles checkpoint (serializing `GicState` exactly once). The sub-devices delegate `checkpoint_save`/`checkpoint_restore` to the parent.

4. **Wiring**: During `elaborate()`, the platform Python script maps each sub-device independently. `InterruptSink` is implemented on the Distributor wrapper. Redistributor-to-CPU interrupt pin wiring is per-PE.

Concrete types: `GicState` (shared state struct), `GicV3` (owns `GicState`, implements `SimObject` for checkpoint), `GicDistributor` (implements `Device`, holds `Arc<UnsafeCell<GicState>>`), `GicRedistributor { pe_index: usize }` (implements `Device`), `GicIts` (implements `Device`). All in a single `gic_v3` module under a future `helm-devices-arm` crate.

---

## Q2.2 -- CPU System Registers: ArchState Fields, Device MMIO, or SysRegHandler Trait

### Industry Standard

**QEMU** uses the `ARMCPRegInfo` table -- a static array of register descriptors, each with `{name, state, opc0, opc1, crn, crm, opc2, access, type, readfn, writefn, resetfn, fieldoffset}`. This table is populated in `helper.c` for CPU-local sysregs and in `arm_gicv3_cpuif.c` for ICC_* registers. At CPU realize time, the GIC calls `define_arm_cp_regs()` to inject ICC register entries into the CPU's sysreg table. The key design: a **flat hash table** keyed by `(opc0, opc1, crn, crm, opc2)` with function pointers for read/write. MRS/MSR instructions do a hash lookup and call the registered `readfn`/`writefn`. Pure CPU state (like MPIDR) uses `fieldoffset` -- a byte offset into `CPUARMState`, avoiding any function call. Device state (like ICC_PMR_EL1) uses `readfn/writefn` that call into the GIC device. The `ARM_CP_IO` flag marks sysregs that have side effects, triggering TB end (so state is synchronized at register access boundaries).

**gem5** uses an `ArmISA::MiscReg` enum with `ISA::readMiscReg(MiscRegIndex)` dispatch. The `GenericTimer` and `GicV3CPUInterface` objects register themselves with the ISA via `connectCPUPorts()`. When a `MRS CNTPCT_EL0` executes, the ISA executor calls `readMiscReg(MISCREG_CNTPCT_EL0)`, which dispatches to `GenericTimer::readMiscReg()`, which calls `systemCounter->value()`.

**ARM Fast Models**: Uses a `SystemRegisterInterface` port that devices implement. The CPU connects to the port at construction time.

### Creative Approach

**Dual-dispatch SysRegMap in helm-core**: Define a `SysRegMap` type in `helm-core` (no dependency on `helm-devices` or `helm-arch`) as a `HashMap<SysRegKey, SysRegEntry>` where `SysRegKey = (u8, u8, u8, u8, u8)` (op0/op1/crn/crm/op2) and `SysRegEntry` is an enum:

```rust
enum SysRegEntry {
    /// Direct field in ArchState -- zero-cost access via offset
    Inline { read_offset: usize, write_offset: usize },
    /// Dynamic handler -- function pointer pair, injected at elaborate()
    Handler(Box<dyn SysRegHandler>),
}

trait SysRegHandler: Send {
    fn read(&self, ctx: &ExecContext) -> u64;
    fn write(&mut self, ctx: &mut ExecContext, val: u64);
}
```

MPIDR, SCTLR, TTBR0 are `Inline` entries pointing directly into `ArchState` fields. CNTPCT uses a `Handler` pointing to the timer device. ICC_* uses handlers pointing to the GIC CPU interface. The MRS/MSR executor does one `HashMap::get()`, then branches: `Inline` is a pointer dereference; `Handler` is a trait object call. The HashMap is built during `elaborate()` and is immutable during RUN.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **All in ArchState** | Zero dispatch cost on MRS/MSR; simple checkpoint | Bloats ArchState with device-specific state (ICC regs, timer values); creates dependency from ArchState to device semantics; CNTPCT needs clock reference in ArchState |
| **All via Device MMIO** | Clean separation; device owns its state | MMIO dispatch overhead on every MRS/MSR; wrong semantic model (sysregs are not memory-mapped in GICv3); adds MemoryMap regions for non-MMIO things |
| **SysRegMap with Inline/Handler** | Zero cost for pure CPU regs (MPIDR, SCTLR); trait-object cost only for device regs; decouples helm-arch from helm-devices; devices inject handlers at elaborate() | Adds a HashMap lookup per MRS/MSR (mitigated by caching in a fixed-size array for common ops); requires SysRegHandler trait in helm-core |

### Helm-NG Suitability

The **SysRegMap** approach is the best fit. The reasoning:

1. **Dependency direction**: `helm-arch` depends on `helm-core`, not on `helm-devices`. The `SysRegHandler` trait lives in `helm-core`. The GIC CPU interface and timer implement `SysRegHandler` in their respective crates. During `elaborate()`, these handlers are injected into the `SysRegMap` held by the engine. `helm-arch`'s MRS/MSR executor receives the map as part of `ExecContext`.

2. **Hot-path optimization**: For the ~20 most frequently accessed sysregs (NZCV, FPCR, SCTLR, MPIDR, etc.), use `Inline` entries that resolve to a field offset in `ArchState` -- literally a pointer add and dereference, zero function calls. Only device-backed sysregs (ICC_*, CNTPCT, CNTP_CTL) use the `Handler` path.

3. **QEMU precedent validates this**: QEMU's `ARMCPRegInfo` is exactly this pattern. The `fieldoffset` path is `Inline`; the `readfn/writefn` path is `Handler`. QEMU's 15+ years of ARM support demonstrate this decomposition works at scale.

4. **MRS/MSR dispatch cost**: A `HashMap::get()` per sysreg access is acceptable because sysreg accesses are far less frequent than ALU ops (typically < 1% of instruction mix except during timer-heavy boot). For extreme cases, the map can be converted to a perfect hash or a direct-indexed array keyed by `(op0 << 14 | op1 << 11 | crn << 7 | crm << 3 | op2)` -- 17 bits, 128K entries, fits in L2.

---

## Q2.3 -- ARM SMMU: Page Table Walk in Device or Delegated to helm-memory

### Industry Standard

**QEMU** implements the SMMU's page walk **entirely within the SMMU device** (`hw/arm/smmu-common.c`). The `smmu_ptw()` function performs VMSAv8-64 table walks with its own `SMMUIOTLBEntry` TLB (a hash table keyed by `SMMUIOTLBKey` containing IOVA, ASID, VMID, and level). The SMMU TLB allows multiple entries for the same address at different page table levels. TLB invalidation is SMMU-specific (TLBI commands via the SMMU command queue: `CMD_TLBI_NH_VA`, `CMD_TLBI_S2_IPA`, etc.). The walk code is "largely inspired from intel_iommu.c" per the QEMU commit messages. **No code is shared** between the CPU MMU walk (`target/arm/ptw.c`) and the SMMU walk (`hw/arm/smmu-common.c`). Recent patches (Mostafa Saleh, Google, 2024) added nested stage-1/stage-2 translation with its own combined TLB format.

**gem5** implements the SMMU walk in `src/dev/arm/smmu_v3/` with its own `SMMUv3SlaveInterface`, walk engine, and TLB hierarchy. gem5's CPU MMU walker (`src/arch/arm/table_walker.cc`) and the SMMU walker are separate codebases. The SMMU walker does DMA-style memory reads (via port-based memory access) to walk page tables, while the CPU walker uses the TLB's private port.

**Common pattern**: Both QEMU and gem5 keep the SMMU walk **completely separate** from the CPU MMU walk. The rationale is that the SMMU has fundamentally different invalidation semantics (command queue vs. TLBI broadcast), different TLB tagging (stream ID + SubstreamID vs. ASID alone), different fault reporting (SMMU event queue vs. CPU synchronous data abort), and different access patterns (SMMU walks are typically triggered by DMA device access, not by CPU instruction execution).

### Creative Approach

**Shared walk primitives, separate walk engines**: Extract the pure VMSAv8-64 page table descriptor parsing logic (4KB/16KB/64KB granule detection, level 0-3 descriptor format parsing, permission bit extraction, output address computation) into a `vmsa_walk` module in `helm-memory`. This module exposes stateless functions like `fn parse_descriptor(desc: u64, level: u8, granule: Granule) -> WalkResult`. Both the CPU MMU and the SMMU use these parsing functions but have their own walk loop, their own TLB, and their own fault/invalidation paths. Code duplication is limited to the walk loop orchestration (~50 lines), while descriptor parsing (~200 lines) is shared.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Fully separate SMMU walk** (QEMU/gem5) | Zero coupling; SMMU can evolve independently; different invalidation/fault semantics are natural; proven at scale | Duplicates ~200 lines of VMSAv8 descriptor parsing; bugs fixed in CPU walk may not be fixed in SMMU walk |
| **Shared walk engine** | Single implementation to maintain; consistent behavior | SMMU needs stream ID/SubstreamID tagging, SMMU event queue for faults, command-queue-based invalidation, DMA memory access path -- all different from CPU MMU. Coupling would be a maintenance burden. |
| **Shared primitives, separate engines** (creative) | Descriptor parsing is shared (consistency); walk orchestration is separate (different semantics fit naturally); ~50 lines of walk loop duplication vs ~200 lines of parsing saved | Adds a `vmsa_walk` module to `helm-memory`; SMMU device needs a dependency path to `helm-memory` for the parsing functions (but SMMU is naturally an `helm-memory`-adjacent component) |

### Helm-NG Suitability

**Shared walk primitives with separate walk engines** is the right approach. The key factors:

1. **Dependency constraint**: `helm-devices` depends only on `helm-core`. An SMMU device model should live in a separate crate (e.g., `helm-devices-arm`) that can depend on `helm-memory` for the `vmsa_walk` primitives. The SMMU is fundamentally a memory-system component, not a pure device in the `helm-devices` sense.

2. **Bug consistency**: VMSAv8 descriptor parsing has many corner cases (contiguous bit, guarded pages, hierarchical permission overrides). Having one implementation that both CPU MMU and SMMU call ensures bugs are fixed once.

3. **Separate TLB**: The SMMU TLB tags entries by `{stream_id, substreamid, vmid, iova, level}` while the CPU TLB tags by `{asid, va, level}`. These are different structures. The SMMU has its own IOTLBKey/IOTLBEntry types.

4. **Fault path**: CPU walk faults are synchronous data aborts (set ESR_EL1, trigger exception). SMMU walk faults post events to the SMMU event queue (a guest-memory ring buffer). These paths share nothing.

5. **Phase timing**: The SMMU is a Phase 3 feature (full system). By then, the CPU MMU walk in `helm-memory` will be mature, and extracting the `vmsa_walk` parsing module will be a natural refactoring.

---

## Q2.4 -- ARM Generic Timer CNTPCT_EL0: VirtualClock Direct or Device Read

### Industry Standard

**QEMU**: `gt_cnt_read()` in `target/arm/helper.c` reads `qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL)` and divides by `GTIMER_SCALE` (typically 16ns, giving 62.5MHz). This is a **direct clock read** from the ISA executor -- it does NOT go through `MemoryRegion` dispatch. The counter read is effectively free: just reading a 64-bit integer and doing a division. The `ARMCPRegInfo` entry for `CNTPCT_EL0` has `readfn = gt_cnt_read` and `type = ARM_CP_IO` (marking it as having side effects, forcing TB termination before read).

**gem5**: `GenericTimer::readMiscReg(MISCREG_CNTPCT_EL0)` calls `physTimer.value()`, which calls `_systemCounter.value()`. The `SystemCounter` is a separate `SimObject` that tracks the counter based on simulation ticks. Both the per-CPU architected timer and the memory-mapped timer frames (`GenericTimerMem`) share the same `SystemCounter` instance.

**Spike** (RISC-V): The `mtime` counter is read directly from the `processor_t::state` struct -- no device dispatch.

**Frequency of CNTPCT reads**: During a typical Linux boot on AArch64, the kernel reads CNTPCT/CNTVCT tens of thousands of times per second (for `sched_clock()`, `ktime_get()`, timer calibration). In steady-state workloads, frequency is lower but still significant -- `gettimeofday()` and `clock_gettime()` in vDSO read CNTVCT on every call.

### Creative Approach

**SysRegHandler with TimingModel-aware callback**: The timer registers a `SysRegHandler` during `elaborate()`. The handler holds a reference to the engine's `VirtualClock`. On read, it returns `clock.current_tick() / cntfrq_divisor`. The handler itself is parameterized by the timing model variant (passed as a closure at elaborate time):

- Virtual: return `insns_executed * ticks_per_insn`
- Interval: return `estimated_cycles * ticks_per_cycle`
- Accurate: return `pipeline_cycles * ticks_per_cycle`

This avoids the `helm-arch -> helm-engine` dependency because the handler is a closure injected at elaborate time via the `SysRegMap`, and the clock abstraction (`VirtualClock` trait) lives in `helm-core`.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Direct VirtualClock read** (QEMU-style, inline) | Fastest possible -- just a field read; no dispatch; matches QEMU's proven approach | Couples helm-arch to the clock implementation; the ISA executor needs a clock reference; TimingModel variant must be known to helm-arch |
| **Device::read() via MemoryMap** | Clean separation; SystemCounter is a proper Device | MMIO dispatch overhead on every CNTPCT read; semantically wrong for a system register; adds unnecessary MemoryMap entries |
| **SysRegHandler with clock closure** | No helm-arch -> helm-engine dependency; timer handles TimingModel differences; one pointer dereference + division; injectable at elaborate time | Slightly slower than direct field read (trait object call); requires VirtualClock trait in helm-core |

### Helm-NG Suitability

**SysRegHandler with clock closure** is the right choice. The analysis:

1. **Dependency**: `helm-arch` must not depend on `helm-engine` or `helm-timing`. The `SysRegHandler` approach (from Q2.2) resolves this cleanly. The timer device registers a handler at `elaborate()` that captures a reference to `VirtualClock`.

2. **Performance**: A single trait object call (`SysRegHandler::read()`) is fast enough. CNTPCT reads are frequent but not per-instruction -- they are < 0.1% of the instruction mix in typical workloads. The QEMU model shows that even a `qemu_clock_get_ns()` function call is acceptable.

3. **TimingModel coupling**: The handler closure captures the timing-model-specific logic at elaborate time. In Virtual mode, the closure reads `insns_executed`. In Interval mode, it reads the cycle estimate. In Accurate mode, it reads the pipeline cycle counter. The ISA executor sees a uniform `SysRegHandler::read()` call and is unaware of the timing model variant.

4. **Consistency**: The same `VirtualClock` instance is shared between the timer `SysRegHandler` and the `EventQueue`'s time base. The comparator (`CNTP_CVAL_EL0 <= CNTPCT`) is evaluated using the same clock source that schedules the comparator event.

Define `VirtualClock` as a trait in `helm-core`:
```rust
pub trait VirtualClock: Send {
    fn current_tick(&self) -> u64;
    fn ticks_per_second(&self) -> u64;
}
```

The timer handler stores `Arc<dyn VirtualClock>` and computes `CNTPCT = clock.current_tick() * (cntfrq / clock.ticks_per_second())`.

---

## Q2.5 -- ARM PSCI: Device, Engine Logic, or PowerController Trait

### Industry Standard

**QEMU**: PSCI is handled in `target/arm/psci.c` as a **special SMC/HVC handler within the CPU target code**, not a device. When an `EXCP_SMC` or `EXCP_HVC` exception is taken, the exception handler checks whether the function ID matches a PSCI function. If so, it handles CPU_ON (calling `cpu_resume()` to start a halted vCPU), CPU_OFF (calling `cpu_pause()`), SYSTEM_RESET (calling `qemu_system_reset_request()`), etc. The `psci_conduit` property on the CPU determines whether SMC or HVC is the conduit. PSCI is NOT a device and has no MMIO region.

**gem5**: PSCI is handled through the TF-A (Trusted Firmware-A) running as actual firmware in the simulation. gem5 models the full EL3 firmware stack, so PSCI calls are handled by real TF-A code executing in the simulator. For simpler configs, the `ArmSystem::callSemihosting()` or SMP boot sequence handles CPU bring-up directly. There is no explicit `PsciProxy` SimObject in mainline gem5.

**ARM Fast Models**: PSCI is part of the firmware model or handled by a `PSCI` component that interfaces with the scheduler.

### Creative Approach

**PowerController trait in helm-core, PSCI as an SMC hook in helm-engine**: Define a minimal trait in `helm-core`:

```rust
pub trait PowerController: Send {
    fn cpu_on(&self, target_affinity: u64, entry_point: u64, context_id: u64) -> PsciResult;
    fn cpu_off(&self) -> PsciResult;
    fn system_reset(&self) -> PsciResult;
    fn system_off(&self) -> PsciResult;
    fn affinity_info(&self, target_affinity: u64) -> PsciResult;
}
```

`HelmEngine<T>` implements `PowerController` (it owns the Scheduler and can park/unpark harts). The PSCI handler is a closure registered in the SMC/HVC dispatch path during `elaborate()`. When an SMC with a PSCI function ID arrives, the ISA executor calls through `PowerController` rather than handling it inline. The PSCI function ID decoding is a thin function in `helm-engine/src/psci.rs`.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Built-in engine logic** (QEMU-style) | Simplest; PSCI has direct access to Scheduler; no abstraction overhead; proven pattern | Hardcoded; cannot be replaced by custom power management; tightly couples ISA exception handling to engine internals |
| **Device implementation** | Pluggable; testable in isolation | Wrong semantic model (PSCI is not MMIO); device would need back-reference to engine/Scheduler (wrong dependency direction: helm-devices -> helm-engine) |
| **PowerController trait in helm-core** | Clean interface; engine implements it; ISA executor calls through trait without knowing engine internals; replaceable for testing; custom power management possible | Adds one more trait to helm-core; slight indirection cost on SMC path (but SMC is very rare -- once per CPU bring-up) |

### Helm-NG Suitability

**PowerController trait in helm-core** is the correct design. The reasoning:

1. **Dependency direction**: The ISA executor in `helm-arch` must be able to handle PSCI-conduit SMC/HVC instructions. It cannot depend on `helm-engine`. A `PowerController` trait in `helm-core` lets `helm-arch` call through the trait while `helm-engine` provides the implementation.

2. **Scheduler access**: `HelmEngine<T>` owns the `Scheduler`. It implements `PowerController` by calling `scheduler.park_hart(affinity)` / `scheduler.unpark_hart(affinity, entry_point)`. This is natural -- the engine already manages hart scheduling.

3. **Cold path**: PSCI calls happen at most once per secondary CPU bring-up (during Linux SMP boot). `CPU_ON` fires ~N-1 times for N cores. This is firmly on the cold path. Trait object indirection is irrelevant.

4. **SMC dispatch**: The ISA executor's SMC handler checks the function ID against the PSCI range (`0x84000000..=0xC400001F`). If matched, it calls `power_controller.cpu_on(...)`. Otherwise, it falls through to the normal SMC handler (PSCI is firmware-level, so in SE mode all SMC with PSCI function IDs are intercepted; in FS mode, they trap to EL3 where TF-A handles them).

5. **Injection point**: During `elaborate()`, `HelmEngine<T>` registers itself (as `Arc<dyn PowerController>`) into the `ExecContext`'s `power_controller` field. The ISA executor accesses it via `ctx.power_controller().cpu_on(...)`.

---

## Q2.6 -- GIC-v3 LPI Tables: MemInterface, MemoryMap, or Direct FlatMem

### Industry Standard

**QEMU**: The GIC reads LPI configuration tables using `address_space_read()` / `address_space_ldub()` -- QEMU's equivalent of `MemoryMap`. This goes through the full address space dispatch (RAM, MMIO, alias resolution). The LPI property table read (`arm_gicv3_lpi_prop_read()` in `hw/intc/arm_gicv3_its.c`) uses `address_space_ldub()` to read a single byte per LPI. Performance was a significant concern: early implementations scanned the entire LPI table on every state change, causing boot slowdowns. The redesign scans only on actual LPI pending state changes (`gicv3_redist_update_lpi()`), dramatically reducing the number of `address_space_read` calls.

**gem5**: GIC-v3 accesses guest memory via `Port::sendAtomicSnoop()` -- a port-based memory access that goes through the memory system (including cache hierarchy if configured). This is the standard DMA-like access mechanism used by all bus-master devices in gem5.

### Creative Approach

**DMA port with cached read-back**: Give the GIC a `DmaPort` (defined in `helm-core`) -- a read-only memory access handle that bypasses MMIO dispatch (since LPI tables are always in RAM, never in MMIO regions) but still goes through `MemoryMap` for address translation. The GIC caches the LPI configuration table locally (a small `Vec<u8>` indexed by LPI number) and invalidates the cache on GICR_INVLPIR or GICR_INVALLR writes. On LPI delivery, the GIC reads from its local cache -- zero memory system overhead. On invalidation, it re-reads the affected entries via `DmaPort`.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **MemoryMap (QEMU-style)** | Correct semantics (GIC is a bus master); observable via HelmEventBus; SMMU can translate if needed | MemoryMap dispatch overhead on interrupt critical path; MMIO dispatch for what is always a RAM read is wasteful |
| **Direct FlatMem reference** | Fastest possible -- pointer dereference; no dispatch | Breaks abstraction; bypasses SMMU; not observable; FlatMem is an internal type of helm-memory |
| **DMA port with cached read-back** | Cache hit on hot path is zero-cost; invalidation is correct; SMMU-translatable on the cold invalidation path; observable on cache fill | Adds complexity for cache management; cache coherence with guest writes (guest rarely writes LPI tables after boot, so this is acceptable); adds DmaPort to helm-core |

### Helm-NG Suitability

**DMA port with cached LPI table** is the right approach. The analysis:

1. **Hot path**: LPI delivery is on the interrupt critical path. Every MSI-X interrupt from a PCIe device triggers an LPI lookup. Going through full `MemoryMap` dispatch on every interrupt delivery is wasteful.

2. **Guest behavior**: LPI configuration tables are written by the OS during GIC initialization and rarely changed afterward. Caching is highly effective. The GIC's INVLPIR/INVALLR commands provide a well-defined invalidation protocol.

3. **DMA abstraction**: Define a `trait DmaPort` in `helm-core` with `fn dma_read(&self, paddr: u64, buf: &mut [u8])`. `helm-engine` provides the implementation that routes through `MemoryMap` (for SMMU translation and observability). The GIC's `elaborate()` stores an `Arc<dyn DmaPort>`. On cache miss or invalidation, it calls `dma_port.dma_read()`. On cache hit, it reads from its internal `Vec<u8>`.

4. **SMMU integration**: When SMMU is present (Phase 3), the `DmaPort` implementation routes through SMMU translation before hitting RAM. This is correct -- GIC LPI table reads are DMA operations and should be SMMU-translated if the GIC's stream ID is configured in the SMMU.

5. **Observability**: `DmaPort` reads can fire `HelmEventBus::MemRead` events if configured, providing tracing and debugging visibility.

---

## Q2.7 -- GICv3 ICC_* System Registers: Dispatch Without helm-devices Dependency

### Industry Standard

**QEMU**: The GIC CPU interface registers ICC_* handlers into the CPU at realize time using `define_arm_cp_regs()`. The GIC code in `hw/intc/arm_gicv3_cpuif.c` defines an `ARMCPRegInfo` array (`gicv3_cpuif_reginfo[]`) with entries like `{.name = "ICC_PMR_EL1", .readfn = icc_pmr_read, .writefn = icc_pmr_write, ...}`. These are injected into the CPU's sysreg hash table. When the CPU executes `MRS ICC_PMR_EL1`, the hash table lookup finds the GIC's `readfn` and calls it. The key pattern: **the device registers function pointers into the CPU at initialization time; the CPU never knows it's calling a GIC function**.

**gem5**: `Gicv3CPUInterface::init()` stores a pointer to itself in the ISA object via `ArmISA::ISA::setGIC()`. The CPU's `readMiscReg(MISCREG_ICC_PMR_EL1)` calls `gic->getCPUInterface(cpu_id)->readMiscReg(reg)`.

**ARM Fast Models**: Uses a `SystemRegisterInterface` port -- the CPU has a port, the GIC CPU interface connects to it.

### Creative Approach

This is directly solved by the **SysRegMap** approach from Q2.2. The GIC CPU interface implements `SysRegHandler` for each ICC_* register. During `elaborate()`, the GIC registers these handlers into the CPU's `SysRegMap`. The MRS/MSR executor dispatches through `SysRegHandler::read()`/`write()` without any `helm-devices` dependency in `helm-arch`.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **SysRegMap injection (Q2.2 pattern)** | Clean layering; no helm-arch -> helm-devices dependency; GIC injects handlers at elaborate(); proven by QEMU's ARMCPRegInfo pattern | One trait object call per ICC register access; SysRegHandler in helm-core |
| **ExecContext method** | Slightly faster (direct function call vs trait object) | ExecContext is in helm-core; adding GIC-specific methods to it is wrong -- it shouldn't know about GIC |
| **Special MemInterface region** | Reuses existing MMIO dispatch | Semantically wrong; ICC_* are system registers, not memory-mapped in GICv3; adds fake MMIO regions |

### Helm-NG Suitability

**SysRegMap injection** is the clear answer, and it is the same mechanism recommended in Q2.2. During `elaborate()`, the GIC CPU interface (one per PE) calls:

```rust
sysreg_map.register(
    SysRegKey::new(3, 0, 12, 12, 5),  // ICC_PMR_EL1 encoding
    SysRegEntry::Handler(Box::new(self.icc_pmr_handler())),
);
```

The `icc_pmr_handler()` returns a `SysRegHandler` implementation that reads/writes the GIC CPU interface's PMR field. The `SysRegMap` is owned by the engine and passed to the ISA executor via `ExecContext`. No dependency from `helm-arch` to `helm-devices`.

**ICC register access frequency**: ICC_IAR1_EL1 (interrupt acknowledge) and ICC_EOIR1_EL1 (end of interrupt) are called once per interrupt. In a busy system, this might be thousands of times per second -- but compared to billions of instructions per second, this is firmly on a cold path. Trait object dispatch is acceptable.

---

## Q2.8 -- EventQueue Callback Timing: Guaranteed at Instruction Boundaries?

### Industry Standard

**QEMU**: Timer callbacks registered via `timer_new_ns(QEMU_CLOCK_VIRTUAL, ...)` execute in the **main loop between translation block (TB) executions**. QEMU's TCG compiles guest code into translation blocks. At the end of each TB, the CPU exits to the main loop, which checks for pending timers. The `ARM_CP_IO` flag on timer registers forces a TB end before accessing the register, ensuring the virtual clock is up-to-date. Timer callbacks **cannot fire mid-instruction**. They fire at TB boundaries, which are instruction boundaries (each TB ends at a branch, exception, or max-TB-size limit).

**gem5**: Event callbacks execute between ticks in the event-driven simulation loop. In the `AtomicSimpleCPU`, events fire between instruction completions. In the `O3CPU`, events fire between pipeline stages at cycle boundaries. Events are never processed mid-instruction -- the event queue is drained after the current instruction (or cycle, in O3) completes.

**Spike** (RISC-V): No event queue; `mtime` is checked at every instruction boundary by polling. Timer interrupts are injected at the next instruction boundary.

**Key insight**: All major simulators guarantee that event callbacks fire at instruction boundaries, never mid-instruction. This is necessary because: (1) architectural state must be consistent (PC points to a valid instruction, all register writes from the current instruction are committed), and (2) interrupt injection must happen between instructions to match real hardware behavior.

### Creative Approach

**Explicit instruction-boundary contract in EventQueue::drain_until()**: Document and enforce that `drain_until()` is only called from the instruction step loop, after the current instruction has committed its results. Add a debug assertion:

```rust
impl EventQueue {
    pub fn drain_until(&mut self, until_cycle: Cycles) {
        debug_assert!(
            self.caller_at_insn_boundary.get(),
            "drain_until() called mid-instruction -- state may be inconsistent"
        );
        // ... drain events
    }
}
```

The step loop sets `caller_at_insn_boundary = true` after each instruction commits and before calling `drain_until()`.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Drain at instruction boundaries only** (QEMU/gem5) | Safe; consistent architectural state; interrupt injection is correct; matches all real hardware behavior | Adds up to one instruction of latency between the scheduled event time and when the callback runs |
| **Drain at arbitrary points** | Lower latency between event time and callback execution | Callbacks see inconsistent state; interrupt injection mid-instruction violates ARM architecture spec; impossible to reason about correctness |
| **Per-instruction drain check** | Exact timing -- event fires at the exact instruction that crosses the threshold | Overhead of checking `peek_next_tick()` on every instruction; unnecessary for Virtual/Interval modes where per-cycle accuracy is not needed |

### Helm-NG Suitability

**Drain at instruction boundaries only**, with the frequency determined by the timing model:

1. **Virtual mode**: `drain_until()` is called every N instructions (quantum size, e.g., every 1000 instructions). Events scheduled between two drain points fire at the next drain point. This is the temporal decoupling model from AGENT.md.

2. **Interval mode**: `drain_until()` is called at interval boundaries (e.g., every basic block or every N instructions). The interval timing model advances the cycle count by the estimated CPI, and events are drained at interval boundaries.

3. **Accurate mode**: `drain_until()` is called every cycle. Events fire with cycle-accurate timing. The per-cycle drain is part of the pipeline model.

**The contract**: `EventQueue::drain_until()` documentation states: "Callbacks execute at instruction boundaries. Architectural state (PC, registers, flags) is consistent at callback time. Interrupt state modifications in callbacks take effect at the next instruction fetch." This matches QEMU/gem5 semantics and is enforced by placing `drain_until()` calls after `step()` returns in the engine's main loop.

**For the Generic Timer specifically**: The timer schedules an event at `CNTP_CVAL_EL0` ticks. When `drain_until()` fires the event, the callback calls `timer_irq_pin.assert()`, which propagates to the GIC, which sets a pending bit, which evaluates CPU interrupt delivery. At the next instruction fetch, the engine checks `pending_interrupt` and enters the exception vector. This matches the ARM architecture spec: "Timer interrupts are edge-triggered and are asserted when the counter reaches the compare value."

---

## Q2.9 -- Watchdog Device-Initiated System Reset

### Industry Standard

**QEMU**: Watchdog timeout calls `watchdog_perform_action()` (or historically `qemu_system_reset_request()`), a **global function** that schedules a reset to be executed asynchronously in the main loop. The function is accessible from any device. QEMU's reset mechanism is three-phase: enter (reset local state), hold (reset with cross-object effects like deasserting IRQs), exit (leave reset state). The key insight: `qemu_system_reset_request()` does NOT execute the reset immediately -- it sets a flag, and the main loop processes it between TB executions. This makes it safe to call from any context, including `Device::write()`.

**gem5**: Devices post a `SimExitEvent("system_reset")` to the global event queue. The event handler in the main loop calls `system->reset()` on all SimObjects.

**SIMICS**: Devices raise a `"core-reset"` attribute write, which the platform handler catches and initiates a system-wide reset sequence.

### Creative Approach

**DeviceAction return value from write()**: Instead of changing `write()` from infallible to fallible, introduce a `DeviceAction` mechanism where `write()` remains `-> ()` but devices that need to signal system-level actions store a pending action in a `Cell<Option<DeviceAction>>`:

```rust
pub enum DeviceAction {
    None,
    SystemReset,
    SystemOff,
    WarmReset,
}
```

After each `Device::write()`, the engine checks `device.pending_action()` and processes it. This avoids changing the `Device` trait signature, avoids making reset implicit (unlike HelmEventBus), and gives the engine full control over when the reset actually executes.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **HelmEventBus custom event** | No trait change; event-driven; multiple subscribers can react | Implicit; hard to trace causality; event may be missed if no subscriber; reset timing depends on subscriber ordering |
| **DeviceAction return from write()** | Explicit; engine controls timing; no trait signature change (uses side channel); easy to trace (the engine checks after every write) | Requires checking after every `write()` call; adds `Cell<Option<DeviceAction>>` to Device state |
| **Global reset function (QEMU-style)** | Simple; proven | Requires global state or a reference to the engine from the device; violates helm-ng's "no global state" principle |

### Helm-NG Suitability

**HelmEventBus with explicit DeviceAction enum** is the right hybrid. The mechanism:

1. The watchdog `write()` handler detects timeout and calls `self.event_bus.fire(&HelmEvent::DeviceSignal { device: self.object_id(), port: "watchdog_reset".into(), asserted: true })`.

2. `HelmEngine<T>` subscribes to `HelmEvent::DeviceSignal` during `elaborate()`. When it sees `port == "watchdog_reset"`, it queues a reset to execute at the next instruction boundary (similar to QEMU's deferred reset).

3. The reset is processed after the current instruction completes, calling `system.reset_all()` which invokes `SimObject::reset()` on every component in registration order.

This works because:
- `HelmEventBus` is already available to devices (the bus is passed via `Arc` during `elaborate()`).
- `HelmEventBus` fire is synchronous -- the engine's subscriber runs immediately and sets an internal `pending_reset` flag.
- The actual reset is deferred to the next instruction boundary (safe state).
- The event is observable (trace logger can log it, GDB can break on it).
- No `Device` trait change required.

For a cleaner API in the future, consider adding `DeviceAction` to the `HelmEvent` enum: `HelmEvent::SystemAction { action: DeviceAction, source: HelmObjectId }`. This makes system-level actions first-class events rather than encoded in `DeviceSignal::port` strings.

---

## Q2.10 -- GIC Affinity Routing vs World's Flat Object Namespace

### Industry Standard

**QEMU**: GIC-v3 stores redistributors in an array indexed by CPU index (not MPIDR affinity). The mapping from MPIDR affinity to redistributor is built at realize time: each CPU registers with the GIC, and the GIC stores `cpu->mp_affinity` in the redistributor's state. When routing an SGI or affinity-routed SPI, the GIC iterates over redistributors and matches `Aff3.Aff2.Aff1.Aff0` from `GICD_IROUTER` against each redistributor's stored affinity value. The function `gicv3_get_redist_by_affinity()` performs this lookup.

**gem5**: `Gicv3::getRedistributorByAffinity(uint32_t affinity)` iterates over the redistributor vector and matches the affinity value. The mapping is set up during `Gicv3::init()` when each CPU's `MPIDR_EL1` value is read. The gem5 Python config exposes per-CPU `mpidr` configuration.

**ARM TF-A**: MPIDR_EL1 is read directly from the hardware register. The PSCI implementation uses MPIDR as the PE identifier in `cpu_on(target_affinity, ...)`.

### Creative Approach

**AffinityMap registered at elaborate() time**: Define an `AffinityMap` type in `helm-core` (or `helm-engine`):

```rust
pub struct AffinityMap {
    // MPIDR affinity → hart object ID
    by_affinity: HashMap<u64, HelmObjectId>,
    // hart object ID → MPIDR affinity
    by_object: HashMap<HelmObjectId, u64>,
}
```

During `elaborate()`, the platform Python script registers each CPU's MPIDR value:

```python
system.register_affinity(cpu0, mpidr=0x00000000)  # Aff3=0, Aff2=0, Aff1=0, Aff0=0
system.register_affinity(cpu1, mpidr=0x00000001)  # Aff3=0, Aff2=0, Aff1=0, Aff0=1
system.register_affinity(cpu2, mpidr=0x00000100)  # Aff3=0, Aff2=0, Aff1=1, Aff0=0  (cluster 1)
```

The GIC queries `World::affinity_map()` to resolve MPIDR-based routing. The SMMU can use a similar `StreamIdMap` for stream ID to device mapping. PCI can use `RequesterIdMap` for BDF to device mapping.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **GIC-internal lookup table** (QEMU/gem5-style) | Self-contained; GIC manages its own routing; no World API needed | Duplicates topology knowledge inside the GIC; PSCI also needs MPIDR mapping (separate duplication); SMMU needs stream ID mapping (another duplication) |
| **World::affinity_map() API** | Single source of truth for PE topology; GIC, PSCI, SMMU all query the same map; Python config is the authority; extends naturally to stream IDs and requester IDs | Adds topology concepts to World; World becomes more than a flat object namespace |
| **Object naming conventions** (e.g., `cpu_0_0_0_0` encodes MPIDR) | Zero API cost; naming IS topology | Fragile; parsing names is error-prone; breaks if naming conventions change; not type-safe |

### Helm-NG Suitability

**World::affinity_map() API** is the right approach. The reasoning:

1. **Single source of truth**: MPIDR values are a platform integration concern, just like base addresses and IRQ routing. The Python config script defines them. `World` stores them. All consumers (GIC, PSCI, SMMU) query the same map.

2. **Python config API**:
```python
system.register_affinity(cpu0, mpidr=0x00000000)
system.register_affinity(cpu1, mpidr=0x00000001)
```

This is consistent with `system.map_device()` and `system.wire_interrupt()` -- it's another piece of platform wiring performed at elaborate time and frozen after startup.

3. **GIC usage**: During `elaborate()`, the GIC Distributor stores `Arc<AffinityMap>`. When routing an affinity-based interrupt (`GICD_IROUTER[n]` specifies target affinity), the GIC calls `affinity_map.by_affinity(target_mpidr)` to find the `HelmObjectId` of the target PE, then looks up the corresponding redistributor.

4. **MPIDR as ArchState**: Each hart's MPIDR_EL1 is set during construction from the Python config. It is an `Inline` entry in the `SysRegMap` (from Q2.2), pointing to a field in `ArchState`. The `AffinityMap` and the `ArchState` MPIDR field must agree -- this is validated during `validate_wiring()` after all `elaborate()` calls complete.

5. **Extensibility**: The same pattern extends to stream IDs (SMMU), requester IDs (PCI), and cluster topology. `World` can provide:
```rust
fn affinity_map(&self) -> &AffinityMap;
fn stream_id_map(&self) -> &StreamIdMap;  // Phase 3
fn requester_id_map(&self) -> &RequesterIdMap;  // Phase 3
```

All three are populated from Python config and frozen after startup.

---

## Domain 3: Dynamically Loadable Devices

## Q3.1 -- Hot-Reload of .so Plugins at Runtime

### Industry Standard

**QEMU**: No hot-reload. Device models are compiled into the binary or loaded once via QOM `type_init()` constructors at startup. Unloading a type after registration would leave dangling pointers in the type table, FlatView, and interrupt wiring graph. **gem5**: No hot-reload. SimObjects are constructed via `Params::create()` during `instantiate()` and the component tree is frozen after `startup()`. **DynamoRIO**: Supports a [detach/reattach cycle](https://dynamorio.org/page_design_docs.html) where all instrumented threads are stopped, the client `.so` is unloaded, and a new one is loaded. This is not true hot-reload -- it is a stop-the-world swap. **Valgrind**: No hot-reload; tools are loaded once at process startup. **SIMICS**: No hot-reload of DML device models; modules are loaded at startup and remain for the session lifetime.

The industry consensus across all major simulators is: **no hot-reload of device model code**. The safety concerns are real -- any reference held by the host (vtable pointers, `Box<dyn Device>`, interrupt wiring `Arc`s) would become dangling if the `.so` were unloaded.

**Erlang/OTP** is the gold standard for hot code reloading in production systems. The BEAM VM maintains [two versions of every module simultaneously](https://www.erlang.org/doc/system/code_loading.html) -- a "current" and an "old" version. Running processes continue executing old code until they make a fully-qualified call (`Module:Function`), at which point they transition to the current version. The OTP framework wraps this in a [safe upgrade protocol](http://lrascao.github.io/fing-hot-code-load-how-does-it-work/): `sys:suspend` -> `sys:change_code` (triggers the GenServer `code_change/3` callback to migrate state) -> `sys:resume`. This only works because Erlang processes are isolated, garbage-collected, and communicate exclusively via message passing -- there are no shared mutable pointers between processes.

**Extism/WebAssembly**: The [Extism framework](https://github.com/extism/extism) allows loading Wasm modules as plugins. Since Wasm modules execute in a sandboxed linear memory with no direct pointer sharing with the host, a module can be unloaded and replaced without dangling references. The host simply creates a new `Plugin` instance from the new `.wasm` bytes. However, [Wasmtime I/O overhead can be 10x slower than native](https://medium.com/the-rise-of-device-independent-architecture/the-benchmark-bake-off-which-runtime-actually-wins-in-2025-ebf69ec5a080) in some configurations, and the host-guest function call boundary adds latency -- problematic for MMIO-frequency calls.

### Creative Approach

**Checkpoint-bracketed hot-swap**: Stop the simulation, call `checkpoint_save()` on every `SimObject`, `dlclose()` the old `.so`, `dlopen()` the new `.so`, re-register the new `DeviceDescriptor`, re-instantiate the device from the factory, call `checkpoint_restore()` with the saved blob, then re-wire interrupts and MMIO mappings. This leverages the existing checkpoint machinery to handle state migration across code versions. The key insight is that helm-ng already requires checkpoint/restore for correctness -- hot-reload becomes "checkpoint, swap code, restore" with no new primitives.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **No hot-reload** (QEMU/gem5 approach) | Zero complexity; no dangling pointer risk; deterministic | Must restart entire simulation to test device changes |
| **Checkpoint-bracketed swap** | Uses existing checkpoint infra; safe (no dangling refs); state migrated via serde | Requires full simulation quiesce; checkpoint version must be compatible across plugin versions; interrupt re-wiring overhead |
| **Wasm-based plugins** (Extism/Wasmtime) | True isolation; can unload/reload freely; sandbox prevents crashes | 10-88% overhead per function call; MMIO at device-register frequency (millions/sec) makes this prohibitive; complex marshalling |

### Helm-NG Suitability

**Checkpoint-bracketed swap is the right answer for helm-ng.** The simulator already requires `checkpoint_save()`/`checkpoint_restore()` on every `SimObject`. The swap protocol would be:

1. `HelmEngine::pause()` -- quiesce the event queue and drain IO
2. `World::checkpoint_save()` -- serialize all device state
3. `DeviceRegistry::unload_plugin(name)` -- drop the `Box<dyn Device>`, remove from `MemoryMap`, drop `Library` handle (triggers `dlclose`)
4. `DeviceRegistry::load_plugin(new_path)` -- `dlopen` new `.so`, ABI version check, register new descriptor
5. `DeviceRegistry::create(name, params)` -- instantiate new device
6. `World::wire_and_restore()` -- re-map MMIO, re-wire interrupts, call `checkpoint_restore()`
7. `HelmEngine::resume()`

This should be a Phase 2+ feature gated behind a `--dev-reload` flag. True hot-reload (no pause) is not worth pursuing -- the wiring graph is frozen after `startup()` by design rule, and unfreezing it introduces unbounded complexity.

---

## Q3.2 -- Minimum Stable ABI Surface

### Industry Standard

**QEMU**: Plugins use a pure [C ABI with stable struct layouts](https://qemu-project.gitlab.io/qemu/devel/qom.html). `TypeInfo`, `ObjectClass`, and `Property` are `#[repr(C)]` structs. The plugin entry point is `type_init()` which calls `type_register_static()`. The entire interface is C function pointers and C structs -- no C++ vtables cross the boundary. **DynamoRIO**: [C ABI with explicit versioned headers](https://dynamorio.org/page_design_docs.html). Clients link against `dr_api.h` which is versioned; the runtime checks the client's declared API version at load time. **PIN**: C++ ABI, but Intel controls the compiler -- plugins must be compiled with Intel's provided kit. **gem5**: Requires the same compiler and build for plugins and host -- no stable ABI boundary.

**Rust ecosystem**: The [`abi_stable` crate](https://github.com/rodrimati1992/abi_stable_crates) provides `StableAbi` derive macro, FFI-safe standard library replacements (`RStr`, `RVec`, `RArc`), and load-time type layout checking. It uses a three-crate model (interface/implementation/user) and checks type layouts recursively at load time. However, each `0.y.0` version defines its own incompatible ABI. The [`stabby` crate](https://github.com/ZettaScaleLabs/stabby) takes a different approach -- it provides compact sum-type representations with niche optimization across FFI, using type-system-level layout proofs. [`cbindgen`](https://github.com/mozilla/cbindgen) generates C headers from `#[repr(C)]` Rust types, providing the thinnest possible ABI surface.

The critical problem with Rust's `TypeId` is that it is [not stable across compilation units](https://github.com/rust-lang/rust/issues/61553). Two `.so` files compiled separately (even with the same compiler version but different workspaces) can produce different `TypeId` values for the same type, breaking `Any`-based downcasting.

### Creative Approach

**Thin C ABI with Rust-side wrapper**: Export exactly 3 C-ABI symbols from each plugin: `HELM_DEVICES_ABI_VERSION` (a `u32`), `helm_device_register` (a `extern "C" fn`), and optionally `helm_device_capabilities` (a `extern "C" fn` returning a JSON string of host requirements). The `DeviceDescriptor` passed through `helm_device_register` contains only C-ABI-safe types: `&'static str` as `*const c_char`, function pointers as `extern "C" fn`, and `Box<dyn Device>` as an opaque `*mut c_void` with a vtable struct of `extern "C" fn` pointers. This way, the ABI surface is exactly 6-8 function pointer signatures plus 3 scalar fields -- small enough to hand-audit for every ABI bump.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Pure C ABI** (cbindgen + `#[repr(C)]`) | Survives any Rust toolchain upgrade; understood by C/C++ plugins too; minimal surface | Must manually define vtable structs for trait objects; no Rust enums across boundary; verbose |
| **`abi_stable` crate** | Rich Rust-to-Rust FFI; load-time type checking; FFI-safe std types | Each `0.y.0` is a new ABI; 327MB docs suggests complexity; `abi_stable` itself becomes a versioned dependency |
| **`stabby` crate** | Compact enums across FFI; type-system layout proofs | Newer, less battle-tested; performance regression on Rust >=1.78; still requires same `stabby-abi` version |

### Helm-NG Suitability

**Use the pure C ABI approach already designed in helm-ng's `LLD-device-registry.md`.** The existing design is correct: 2 exported symbols (`HELM_DEVICES_ABI_VERSION: u32` and `helm_device_register: extern "C" fn`), a `DeviceDescriptor` containing function pointers and `&'static str`, and `Box<dyn Device>` returned from the factory. The minimum stable surface is:

1. **`HELM_DEVICES_ABI_VERSION`** -- single `u32`, bumped on any breaking change
2. **`helm_device_register(registry: *mut DeviceRegistry)`** -- the entry point
3. **`DeviceDescriptor` struct** -- 6 fields, all C-ABI-safe (`&'static str` = `*const u8` + len, `fn` pointers)
4. **`Device` trait vtable** -- 4 methods: `read`, `write`, `region_size`, `signal`
5. **`SimObject` trait vtable** -- 7 methods: `name`, `init`, `elaborate`, `startup`, `reset`, `checkpoint_save`, `checkpoint_restore`
6. **`ParamValue` enum** -- 5 variants, representable as a tagged union `{ tag: u32, payload: [u8; 16] }`

Do **not** adopt `abi_stable` or `stabby` -- they add a versioned dependency that itself must remain compatible. The ABI surface is small enough (under 20 function signatures) that manual `#[repr(C)]` structs with `cbindgen`-generated headers are manageable and indefinitely stable. Use `cbindgen` in the build to generate a `helm_devices_plugin.h` for any C/C++ plugin authors.

---

## Q3.3 -- Transparent Mixing of Plugin and Built-in Devices

### Industry Standard

**QEMU**: Built-in and plugin devices use the [same `TypeInfo` -> `type_register_static()` -> `type_init()` pattern](https://qemu-project.gitlab.io/qemu/devel/qom.html). Whether a device is compiled into the binary or loaded from a `.so` module, it registers the same way into the same type table. The QOM tree contains both seamlessly -- `object_new("uart16550")` works identically regardless of where the type was registered from. QEMU modules use `scripts/modinfo-generate.py` to create metadata databases so QEMU knows about module dependencies and QOM objects implemented by modules. **gem5**: The build system auto-generates `Params::create()` factory methods from Python SimObject class definitions. All SimObjects go through the same `instantiate()` path. There is no separate "plugin" path -- gem5 does not support dynamically loaded SimObjects (everything is compiled into one binary).

### Creative Approach

**Dual-path registration with trait-object unification**: Built-in devices use `inventory::submit!` (linker-magic `.init_array` self-registration) to populate the `DeviceRegistry` before `main()`. Plugin devices use `dlopen` + `helm_device_register()` to populate the same registry at config time. Both paths produce the same `DeviceDescriptor` struct stored in the same `HashMap<&'static str, DeviceDescriptor>`. The `DeviceRegistry::create()` method is the sole factory -- it does not know or care whether the descriptor came from `inventory` or from a plugin. The only subtle difference: built-in devices get zero-cost static dispatch for their `Device` trait methods (the compiler can see through `Box<dyn Device>` when the concrete type is known), while plugin devices always go through the vtable. This difference is invisible at the API level and irrelevant for performance (MMIO calls are cold-path by design rule 6).

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Unified registry** (helm-ng current design) | Single code path; Python config is identical for both; no "special" plugin handling | Plugin `Library` handles must be kept alive; `inventory` crate uses linker magic that can surprise |
| **Separate registries** (built-in vs plugin) | Clear ownership boundary; can unload plugins without touching built-in registry | Python config must know which registry to query; code duplication; "which registry?" becomes a FAQ |
| **Everything-is-a-plugin** (no built-in distinction) | Maximum uniformity; forces ABI surface to be complete | Startup overhead for `dlopen` on built-in devices; testing overhead; build complexity |

### Helm-NG Suitability

**The existing design in `LLD-device-registry.md` is already correct and should be kept as-is.** The `DeviceRegistry` uses `inventory::collect!` for built-ins and `load_plugin()` for externals. Both produce `DeviceDescriptor` entries in the same `HashMap`. Python class injection follows the same `exec()` path for both. The `create()` method is source-agnostic. No changes needed.

The one subtle difference to document: built-in device Python classes are injected at `#[pymodule]` init time (import `helm_ng`), while plugin Python classes are injected when `helm_ng.load_plugin(path)` is called. Platform scripts must call `load_plugin()` before referencing plugin device classes. This ordering requirement should be enforced with a clear error message: `"Unknown device class 'FooDev' -- did you forget helm_ng.load_plugin()?"`.

---

## Q3.4 -- Plugin Isolation Without Full Process Isolation

### Industry Standard

**QEMU, gem5, DynamoRIO, Valgrind, PIN**: All accept the risk. Plugins run in the same address space. A buggy plugin can corrupt arbitrary memory, and the only recourse is a crash dump. **WebAssembly runtimes (Wasmtime, Wasmer)**: Provide [full memory isolation via linear memory](https://docs.wasmtime.dev/). A Wasm module cannot access host memory outside its linear memory sandbox. Function calls cross the boundary through a well-defined import/export ABI. Wasmtime achieves [88% of native speed for compute-bound workloads](https://medium.com/the-rise-of-device-independent-architecture/the-benchmark-bake-off-which-runtime-actually-wins-in-2025-ebf69ec5a080) with JIT, and cold start under 1ms. **seccomp-BPF**: Linux kernel facility that [restricts syscalls for the calling thread/process](https://docs.kernel.org/userspace-api/seccomp_filter.html). Can be installed after `dlopen()` to prevent a loaded plugin from making dangerous syscalls (`execve`, `socket`, `ptrace`). However, seccomp operates at the process level -- it restricts the entire process, not just the plugin code.

### Creative Approach

**Layered defense: Rust type safety + seccomp post-load + AddressSanitizer in debug builds**. Since helm-ng plugins return `Box<dyn Device>` (a Rust trait object), the plugin can only interact with the host through the `Device` trait methods. The plugin cannot call arbitrary host functions -- it only sees what `helm-devices` exports. After all plugins are loaded, install a seccomp-BPF filter that blocks `execve`, `fork`, `socket`, `ptrace`, and other dangerous syscalls. In debug builds, compile with `-Zsanitizer=address` to catch out-of-bounds accesses early. This provides three layers: (1) Rust's type system prevents most memory corruption, (2) seccomp prevents syscall abuse, (3) ASan catches what Rust's borrow checker misses (unsafe blocks in plugins).

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Accept the risk** (QEMU/gem5) | Zero overhead; simplest implementation | Buggy plugin crashes everything; no containment |
| **Wasm sandbox** (Wasmtime/Extism) | Full memory isolation; safe unload/reload | 10-50% overhead on function calls; marshalling complex types across boundary; plugins must be compiled to Wasm (limits ecosystem) |
| **seccomp post-load** | Zero runtime overhead after installation; prevents dangerous syscalls | Cannot isolate memory access; whole-process restriction; cannot be removed once installed |
| **Layered defense** (Rust types + seccomp + ASan) | Good defense-in-depth; zero overhead in release; catches most bugs in dev | Not true isolation; a determined attacker with `unsafe` can still corrupt memory; ASan has 2x overhead |

### Helm-NG Suitability

**Use the layered defense approach for Phase 0-2, with Wasm as an optional Phase 3 feature.** The reasoning:

1. **Rust type safety is the primary defense.** Plugin devices implement `Device: SimObject + Send`. They cannot access `World`, `MemoryMap`, or `EventQueue` directly -- those are in `helm-engine`, which plugins do not link. The `Device` trait surface is tiny: `read`, `write`, `region_size`, `signal`.

2. **seccomp post-load is cheap insurance.** After all `load_plugin()` calls complete (during Python config phase), install a seccomp filter blocking `execve`, `fork`, `socket(AF_INET)`, `ptrace`. This prevents a compromised plugin from opening network connections or spawning processes. Cost: zero at runtime (filter is a BPF program evaluated by the kernel only on syscalls).

3. **Wasm isolation is a Phase 3 stretch goal.** If users demand truly untrusted plugin execution, offer a `WasmDeviceAdapter` that wraps an Extism `Plugin` and implements `Device` by marshalling `read`/`write` calls across the Wasm boundary. This would be opt-in per device, not a global requirement. The overhead is acceptable for devices accessed infrequently (UART, GPIO) but not for high-frequency devices (DMA controller).

---

## Q3.5 -- Python Class `__init__` Params vs ParamSchema Divergence

### Industry Standard

**gem5**: [Python SimObject params are authoritative](https://www.gem5.org/documentation/learning_gem5/part2/helloobject/). The C++ `SimObject` reads its configuration from a `Params` struct that is auto-generated from the Python class definition. If the Python class declares `clock_hz = Param.Int("Clock frequency")`, the C++ code accesses `params().clock_hz`. There is no separate C++ schema -- the Python IS the schema. The build system generates `FooParams.hh` from `Foo.py`. Divergence is impossible by construction. **SIMICS**: DML (Device Modeling Language) interface definitions are the contract. Python wrappers are auto-generated from DML, not hand-written. Divergence is impossible by construction. **SQLAlchemy's declarative approach**: The Python class IS the database schema. `class User(Base): name = Column(String(50))` -- the class definition and the schema are the same object.

### Creative Approach

**Generate the Python class string from `ParamSchema` at registration time.** Instead of having plugin authors write the `python_class` string by hand (where it can diverge from `param_schema()`), generate the Python class definition programmatically from the `ParamSchema` fields. The `DeviceDescriptor` would contain `param_schema: fn() -> ParamSchema` but NOT `python_class: &'static str`. Instead, `DeviceRegistry::register()` would call `param_schema()`, iterate the fields, and generate the Python class string automatically:

```rust
fn generate_python_class(name: &str, schema: &ParamSchema) -> String {
    let class_name = to_camel_case(name);
    let mut py = format!("class {}(Device):\n", class_name);
    for field in schema.fields() {
        let py_type = match field.kind {
            ParamType::Int => "Param.Int",
            ParamType::Bool => "Param.Bool",
            ParamType::MemorySize => "Param.MemorySize",
            ParamType::String => "Param.String",
            ParamType::Enum(_) => "Param.Enum",
        };
        py += &format!("    {}: {} = {}\n", field.name, py_type, field.default.to_python());
    }
    py
}
```

This makes `ParamSchema` the single source of truth. Divergence is impossible by construction.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **ParamSchema is authoritative** (auto-generate Python class) | Single source of truth; divergence impossible; DRY | Plugin authors cannot add Python docstrings or custom methods; less flexible |
| **Python class is authoritative** (parse Python to extract params) | Rich Python customization; docstrings, validators, custom methods | Requires a Python parser in Rust; error-prone extraction; gem5-style complexity |
| **Both exist independently** (current helm-ng design) | Maximum flexibility for plugin authors; supports hand-crafted Python classes | Divergence is the default; runtime validation needed; bugs guaranteed |

### Helm-NG Suitability

**Make `ParamSchema` authoritative and auto-generate the Python class.** Modify `DeviceDescriptor` to remove the `python_class: &'static str` field. Instead, add an optional `python_class_extra: &'static str` field for docstrings and additional methods. The registry generates the base class with all params from `ParamSchema`, then `exec()`s the optional extras to add docstrings or validators. The factory always validates against `ParamSchema` before calling the Rust constructor.

For plugin authors who need full control over the Python class (rare), provide a `python_class_override: Option<&'static str>` that bypasses generation. Add a `#[cfg(debug_assertions)]` check that validates the override's params match the schema at load time.

---

## Q3.6 -- Checkpoint Migration When Plugin is Upgraded

### Industry Standard

**QEMU**: Uses [VMState with `version_id` and `minimum_version_id`](https://www.qemu.org/docs/master/devel/migration/main.html) per device. Saving always creates a section with the current `version_id`. Loading checks: if incoming `version_id > local version_id`, reject ("too new"); if incoming `version_id < local minimum_version_id`, reject ("too old"). Version-conditional fields use `VMSTATE_*_V` macros to include/exclude fields based on version. `pre_load`, `post_load`, and `pre_save` hooks handle state transformation.

**gem5**: [Text-based checkpoints with a version tag list](https://www.gem5.org/documentation/general_docs/checkpoints/). The `util/cpt_upgrader.py` tool applies upgrade scripts from `src/util/cpt_upgraders/`. Each upgrader has an `upgrade()` method that transforms the checkpoint INI file. However, [not all changes are covered](https://github.com/gem5/gem5/issues/2430) -- some require regenerating checkpoints.

**SIMICS**: DML `set()` method on attributes handles version migration. Each attribute can inspect the incoming version and transform data accordingly.

### Creative Approach

**Serde-based migration with `#[serde(default)]` and a version field in the checkpoint blob.** Each device checkpoint blob starts with a `u32` version tag (already in helm-ng's design). For forward compatibility, use serde's `#[serde(default)]` on new fields -- if a field is absent in an old checkpoint, it gets its default value. For backward compatibility (removing or renaming fields), implement a `migrate(old_version: u32, data: &[u8]) -> Vec<u8>` method on the device that transforms old checkpoint formats to the current one. Register these migrators in the `DeviceDescriptor`.

```rust
pub struct DeviceDescriptor {
    // ... existing fields ...
    /// Checkpoint version for this device implementation.
    pub checkpoint_version: u32,
    /// Minimum checkpoint version this device can restore from.
    pub min_checkpoint_version: u32,
    /// Optional migrator: transforms checkpoint blob from old_version to current.
    pub checkpoint_migrate: Option<fn(old_version: u32, data: &[u8]) -> Result<Vec<u8>, PluginError>>,
}
```

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **QEMU VMState approach** (version_id + min_version_id + conditional fields) | Battle-tested; fine-grained field-level versioning; QEMU uses it for 500+ devices | Complex macro system; tight coupling between serialization and versioning; not natural in Rust/serde |
| **Serde + version tag + migrator fn** | Natural Rust/serde idiom; `#[serde(default)]` handles additive changes for free; migrator is opt-in | Migrator must handle arbitrary old versions; binary format changes require careful testing |
| **gem5 approach** (external upgrade scripts) | Checkpoint format is text, easy to manipulate; upgraders are independent scripts | Requires a checkpoint upgrade tool; not all changes can be scripted; text format is slower |

### Helm-NG Suitability

**Use the serde + version tag + migrator function approach.** This fits helm-ng because:

1. The existing design already mandates a `u32` version tag as the first 4 bytes of every checkpoint blob.
2. `#[serde(default)]` handles the common case (adding new optional fields) with zero effort.
3. The `checkpoint_migrate` function in `DeviceDescriptor` handles the uncommon case (removing/renaming fields) explicitly.
4. The version range (`checkpoint_version` / `min_checkpoint_version`) follows QEMU's proven pattern of rejecting incompatible checkpoints early.
5. Plugin authors must increment `checkpoint_version` when they change serialized state and provide a migrator if `min_checkpoint_version < checkpoint_version`.

Add `checkpoint_version` and `min_checkpoint_version` to `DeviceDescriptor`. The `World::checkpoint_restore()` method checks the version before calling `SimObject::checkpoint_restore()` and invokes the migrator if the version is in the compatible range but not current.

---

## Q3.7 -- Device Type Aliasing and Versioned Names

### Industry Standard

**QEMU**: QOM type inheritance provides implicit backward compatibility. A new `uart16550a` type can have `parent = "uart16550"`, inheriting all its properties. The old name continues to work. Explicit aliases are done via separate `TypeInfo` registrations that point to the same `class_init`. **gem5**: SimObject type names are fixed strings. No aliasing mechanism exists. If a type is renamed, old config scripts break. **npm**: Package aliases via `"dependencies": { "new-name": "npm:old-name@^1.0" }` and deprecated packages. **OSGi**: Uses [semantic versioning with version ranges](https://docs.osgi.org/whitepaper/semantic-versioning/) for import/export. Bundle symbolic names are fixed, but version ranges allow consumers to accept a range of compatible versions. **Cargo**: `[package]` name is fixed; `[dependencies]` can alias via `package = "..."`.

### Creative Approach

**Alias registry with deprecation warnings**: Add an `aliases: &'static [&'static str]` field to `DeviceDescriptor`. When `DeviceRegistry::create("old_name", params)` is called and `"old_name"` is not found as a primary name, search the alias lists. If found, emit a deprecation warning and redirect to the current name. For versioned names, use a convention like `"uart16550"` (unversioned, always points to latest) plus `"uart16550@1"` (pinned to major version 1). The version suffix is not SemVer -- it is a single integer major version, because minor/patch version changes should not require config script changes.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Alias field in DeviceDescriptor** | Simple; backward compatible; deprecation warnings guide migration | Alias lookup adds O(n) scan on miss; must track which aliases point where |
| **Versioned names** (`uart16550@1`) | Explicit version pinning; multiple major versions can coexist | Config scripts become cluttered; version suffix parsing adds complexity |
| **No aliasing** (gem5 approach) | Simplest; no ambiguity | Rename breaks all config scripts; no migration path |

### Helm-NG Suitability

**Add `aliases` field to `DeviceDescriptor` and support unversioned names only for Phase 0-2.** Versioned names (`@N` suffix) are premature -- helm-ng has no devices yet, and version proliferation is a post-1.0 concern.

```rust
pub struct DeviceDescriptor {
    pub name: &'static str,
    pub aliases: &'static [&'static str],  // legacy names that redirect here
    // ... rest unchanged
}
```

When `create("old_name", params)` fails primary lookup, scan all descriptors' `aliases` arrays. On match, log `warn!("Device type 'old_name' is deprecated; use '{}' instead", desc.name)` and proceed. This is O(n * m) on miss (n devices, m aliases each) but only happens at config time, never in the hot loop. Add a `DeviceRegistry::resolve_name(&self, name: &str) -> Option<&str>` helper that Python can call for IDE autocompletion of deprecated names.

---

## Q3.8 -- Plugin Host Capability Requirements Declaration

### Industry Standard

**QEMU**: Devices check for capabilities in `realize()` and [fail with `error_setg()`](https://qemu-project.gitlab.io/qemu/devel/qom.html) if requirements are not met. For example, `vfio-pci` checks for IOMMU group access, and VirtIO-net checks for TAP device access. The checks are imperative, not declarative -- there is no manifest that lists requirements before instantiation. **gem5**: SimObjects check in `init()` and `warn()`/`fatal()`. No declarative requirements. **Linux kernel modules**: `MODULE_DEPEND()` declares dependencies on other modules. `MODULE_INFO()` provides metadata. **pkg-config**: `Requires:` field in `.pc` files declares build-time dependencies.

### Creative Approach

**Declarative `capabilities` field in `DeviceDescriptor` checked at two points**: (1) at `load_plugin()` time for early fail, and (2) at `realize()`/`elaborate()` time for runtime-dependent capabilities. Define a `Capability` enum:

```rust
pub enum Capability {
    Kvm,              // requires KVM (/dev/kvm accessible)
    Vfio,             // requires VFIO (/dev/vfio accessible)
    RawSocket,        // requires CAP_NET_RAW
    TapDevice,        // requires /dev/net/tun
    Custom(&'static str),  // plugin-defined capability
}
```

The `DeviceDescriptor` gains a `required_capabilities: &'static [Capability]` field. `DeviceRegistry::load_plugin()` checks each capability against the host after registration and emits warnings (not errors -- the device might never be instantiated). `World::elaborate()` checks capabilities for actually-instantiated devices and fails with a clear error.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Declarative in DeviceDescriptor** (checked at load + elaborate) | Early detection; clear error messages; self-documenting; Python help() can show requirements | Must enumerate all capabilities upfront; Custom capabilities need a string registry |
| **Imperative in realize()** (QEMU approach) | Maximum flexibility; can check arbitrary runtime conditions | Late failure (after config is written); poor error messages; no introspection from Python |
| **Both** (declarative for common, imperative for exotic) | Best of both worlds; common cases caught early, exotic cases handled in code | Two code paths; potential for redundant checks |

### Helm-NG Suitability

**Use the "both" approach: declarative for well-known capabilities, imperative for device-specific checks.** Add `required_capabilities: &'static [Capability]` to `DeviceDescriptor`. The `DeviceRegistry::load_plugin()` method logs warnings for unmet capabilities. The device's `elaborate()` (or `realize()` in the config builder pattern) performs the authoritative check and returns `DeviceError` if unmet.

The well-known capabilities should be a small, stable enum. Do not add `Custom(&'static str)` yet -- it adds string comparison overhead and versioning concerns. Start with `Kvm`, `Vfio`, `TapDevice`, `HostFilesystem`. Extend only when a concrete plugin needs it.

---

## Q3.9 -- HelmEventBus Event Type Identity Across .so Boundaries

### Industry Standard

**Windows COM**: Uses [GUIDs](https://learn.microsoft.com/en-us/windows/win32/com/com-objects-and-interfaces) for interface identity. Each interface has a 128-bit IID that is globally unique and stable across compilation units. **D-Bus**: Uses [string-based message types](https://dbus.freedesktop.org/doc/dbus-specification.html) (e.g., `"org.freedesktop.DBus.Properties.PropertiesChanged"`). Identity is string equality. **QEMU**: Uses `QemuNotifier` with [function pointer comparison in the same address space](https://qemu-project.gitlab.io/qemu/devel/qom.html). Since all code is in one process, function pointers for the same function are identical. However, across `.so` boundaries, the same function can have different addresses due to PLT indirection.

The key challenge: Rust's `TypeId` is [not stable across `.so` boundaries](https://github.com/rust-lang/rust/issues/61553). If the host binary and a plugin both define `HelmEvent::Custom { name: "FooEvent", data: Arc<dyn Any> }`, the `TypeId` of the `data` payload will differ between them, making `downcast_ref::<FooEventData>()` fail. This is a fundamental limitation of Rust's compilation model.

### Creative Approach

**String-keyed events with typed wrappers**: Use the existing `HelmEvent::Custom { name: &'static str, data: Arc<dyn Any + Send + Sync> }` variant with string-based identity for cross-plugin events. The `name` field is the event type identity -- string comparison is unambiguous across `.so` boundaries. For the `data` payload, avoid `downcast_ref()` across `.so` boundaries entirely. Instead, serialize event data to a stable format (e.g., `serde_json::Value` or a flat `#[repr(C)]` struct) and deserialize on the receiving side. Within a single compilation unit (e.g., built-in events), `TypeId`-based downcasting works fine -- only cross-`.so` events need the string path.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **String-keyed `Custom(String)` events** | Works across .so boundaries; simple identity; no TypeId issues | String comparison cost; no compile-time type safety for cross-plugin events; name collisions possible |
| **Typed enum variants** (`HelmEvent::Exception`, etc.) | Zero-cost identity (enum discriminant); exhaustive matching; compile-time safety | Cannot be extended by plugins; adding a variant is a breaking ABI change; plugin events forced into `Custom` |
| **GUID-based identity** (COM-style) | Globally unique; stable; no collisions | Verbose; requires a GUID registry; not idiomatic Rust |
| **Hybrid** (typed core + string-keyed plugin) | Core events are fast and type-safe; plugins can define custom events | Two identity mechanisms; subscribers must handle both |

### Helm-NG Suitability

**Use the hybrid approach already implicit in `HelmEvent`.** The existing `HelmEvent` enum defines typed variants for core events (`Exception`, `CsrWrite`, `MemWrite`, etc.) and a `Custom { name: &'static str, data: Arc<dyn Any + Send + Sync> }` catch-all for plugin-defined events.

For cross-`.so` event data, adopt this rule: **plugin-defined events must use `#[repr(C)]` data types passed as raw bytes, not `dyn Any`.** Change the `Custom` variant to:

```rust
Custom {
    name: &'static str,
    data: Vec<u8>,  // serialized event data, not dyn Any
}
```

This avoids the `TypeId` problem entirely. Plugin authors serialize their event data (using `bincode`, `serde_json`, or manual byte packing) and subscribers deserialize it by `name`. Within the same compilation unit, helper methods can provide typed wrappers. The `Vec<u8>` allocation per event is acceptable because `Custom` events are observability/debugging events, not hot-path.

For built-in typed events (`Exception`, `MemWrite`, etc.), keep the existing enum variants with direct field access -- no serialization overhead.

---

## Q3.10 -- Plugin-Registered Bus Trait Implementations and Non-Register Protocols

### Industry Standard

**QEMU**: New bus types can be added by plugins via [QOM type registration](https://qemu-project.gitlab.io/qemu/devel/qom.html). The `BusClass` provides a base class that concrete bus types (PCI, I2C, SPI, USB) inherit from. Each bus type defines its own transaction interface. The `CharDev` backend is QEMU's primary example of a message-based (not register-based) extension point -- it uses `qemu_chr_fe_write()` / `qemu_chr_fe_read()` for byte-stream communication rather than MMIO register reads/writes. CharDev backends (serial, pipe, socket, pty) all implement the same `ChardevClass` vtable.

**gem5**: Uses [Port-based connections](https://www.gem5.org/documentation/learning_gem5/part2/simplecache/) -- `RequestPort` and `ResponsePort` classes handle variable-payload transactions. A `CachePort` sends `Packet` objects containing address, size, and data payload. The port abstraction is protocol-agnostic; the `Packet` carries all the information.

**CAN bus (ISO 11898)**: Message-based, not address-based. A CAN frame contains: arbitration ID (11 or 29 bits), data length code, and 0-8 bytes of payload. There is no "register at offset" concept. **USB**: Packet-based with endpoint addressing. USB transfers are control, bulk, interrupt, or isochronous -- none maps to MMIO.

### Creative Approach

**`BusTransaction` enum with `Any`-based variant dispatch**: Define a core set of bus transaction types, plus an extensible `Custom` variant for plugin-defined protocols:

```rust
pub enum BusTransaction {
    /// Standard MMIO register access (the common case)
    Mmio { offset: u64, size: usize, is_write: bool, data: u64 },
    /// Byte-stream message (UART, CharDev, CAN, SPI)
    Message { payload: &[u8] },
    /// Packet-based with endpoint (USB)
    Packet { endpoint: u16, payload: &[u8] },
    /// Plugin-defined bus protocol
    Custom { protocol: &'static str, data: Vec<u8> },
}
```

A `BusDevice` trait accepts `BusTransaction` instead of raw `(offset, size, val)`:

```rust
pub trait BusDevice: SimObject + Send {
    fn transact(&mut self, txn: &BusTransaction) -> BusResult;
    fn bus_protocol(&self) -> &'static str;  // "mmio", "can", "usb", "spi", etc.
}
```

The existing `Device` trait (with `read`/`write`) becomes a specialization of `BusDevice` for the `Mmio` variant. This keeps backward compatibility while enabling non-register protocols.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **`BusTransaction` enum** | Single dispatch point; protocol-extensible via `Custom`; clean separation | Enum growth over time; `Custom` variant requires serialization; not zero-cost for MMIO (enum match overhead) |
| **Separate traits per bus** (`MmioDevice`, `CanDevice`, `UsbDevice`) | Zero-cost dispatch; compile-time protocol safety; clean interfaces | Proliferation of traits; plugins cannot define new bus traits; `World` must know about every bus type |
| **`Any`-based dispatch** (`fn transact(&mut self, msg: &dyn Any)`) | Maximum flexibility; plugins define arbitrary protocols | No type safety; downcast failures at runtime; `TypeId` problem across .so boundaries |
| **Port-based** (gem5 `Packet` approach) | Protocol-agnostic; single `Packet` type carries all payloads | `Packet` becomes a god-object; overhead for simple MMIO; complex for plugin authors |

### Helm-NG Suitability

**Keep the existing `Device` trait for MMIO devices (the 95% case) and add a separate `BusDevice` trait for non-register protocols in Phase 3.** The reasoning:

1. **Phase 0-2 only need MMIO.** All planned devices (UART, PLIC, CLINT, VirtIO) use register-based MMIO. Adding a `BusTransaction` enum now would add complexity to the hot path with no users.

2. **Phase 3 introduces USB and potentially CAN.** At that point, add:
   ```rust
   pub trait MessageDevice: SimObject + Send {
       fn send(&mut self, data: &[u8]) -> Result<(), DeviceError>;
       fn recv(&mut self, buf: &mut [u8]) -> Result<usize, DeviceError>;
       fn protocol(&self) -> &'static str;
   }
   ```
   This trait is separate from `Device` -- a USB controller implements `Device` for its MMIO control registers AND `MessageDevice` for its data path.

3. **Plugins can register `MessageDevice` implementations** via a second registration path in `DeviceDescriptor`. The descriptor gains an optional `message_factory` field alongside the existing `factory`.

4. **Do NOT use `Any`-based dispatch.** `TypeId` is broken across `.so` boundaries. Use `&[u8]` payloads with protocol-specific serialization, similar to the `Custom` event approach in Q3.9.

5. **The bus module in `helm-devices/src/bus/`** already has `pci/` and `amba/` submodules. A new `can/`, `usb/`, or `spi/` submodule would define protocol-specific transaction types that `MessageDevice` implementations use internally. The `MessageDevice` trait itself remains protocol-agnostic -- protocol specifics are in the bus submodule types.

---

**Key files referenced in this analysis:**

- `/home/pmallapp/proj/personal/helm-ng/AGENT.md` -- Agent onboarding, crate map, design rules
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/HLD.md` -- Device crate high-level design
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/LLD-device-registry.md` -- DeviceRegistry, plugin loading protocol, ABI versioning, Python class injection
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/LLD-device-trait.md` -- Device trait definition, method contracts
- `/home/pmallapp/proj/personal/helm-ng/docs/design/helm-devices/LLD-interrupt-model.md` -- InterruptPin, InterruptWire, InterruptSink, wiring protocol
- `/home/pmallapp/proj/personal/helm-ng/docs/object-model.md` -- SimObject lifecycle, checkpoint protocol
- `/home/pmallapp/proj/personal/helm-ng/docs/traits.md` -- All trait definitions and dispatch strategy

**Sources:**

- [abi_stable crate](https://github.com/rodrimati1992/abi_stable_crates)
- [stabby crate](https://github.com/ZettaScaleLabs/stabby)
- [cbindgen](https://github.com/mozilla/cbindgen)
- [Plugins in Rust: Reducing the Pain with Dependencies](https://nullderef.com/blog/plugin-abi-stable/)
- [Erlang Compilation and Code Loading](https://www.erlang.org/doc/system/code_loading.html)
- [Hot Code Reloading in Erlang](http://malloc.dog/blog/2026/01/31/hot-reloading-code-in-erlang-how-does-it-work/)
- [Erlang Hot Code Loading Internals](http://lrascao.github.io/fing-hot-code-load-how-does-it-work/)
- [QEMU Object Model (QOM)](https://qemu-project.gitlab.io/qemu/devel/qom.html)
- [QEMU Migration Framework](https://www.qemu.org/docs/master/devel/migration/main.html)
- [QEMU Checkpoint and Restart (CPR)](https://www.qemu.org/docs/master/devel/migration/CPR.html)
- [gem5 Checkpoints](https://www.gem5.org/documentation/general_docs/checkpoints/)
- [gem5 SimObject Tutorial](https://www.gem5.org/documentation/learning_gem5/part2/helloobject/)
- [gem5 Checkpoint Upgrader](https://github.com/uart/gem5-mirror/blob/master/util/cpt_upgrader.py)
- [WebAssembly Runtime Benchmarks 2025-2026](https://medium.com/the-rise-of-device-independent-architecture/the-benchmark-bake-off-which-runtime-actually-wins-in-2025-ebf69ec5a080)
- [Extism WebAssembly Plugin Framework](https://github.com/extism/extism)
- [Seccomp BPF Kernel Documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html)
- [Rust TypeId across compilation units (issue #61553)](https://github.com/rust-lang/rust/issues/61553)
- [We Need Type Information, Not Stable ABI](https://blaz.is/blog/post/we-dont-need-a-stable-abi/)
- [OSGi Semantic Versioning](https://docs.osgi.org/whitepaper/semantic-versioning/)

---

## Domain 4: Bus and Protocol Modeling

## Q4.1 -- PCIe ECAM Config Space: PciBus Internal Decode vs. Per-Function MemoryMap Regions

### Industry Standard

**QEMU**: `pcie_host.c` maps the entire ECAM window as a single `MemoryRegion`. When an access arrives, `pci_host_config_read()`/`pci_host_config_write()` decode the BDF from the address offset internally: `bus = (addr >> 20) & 0xFF; devfn = (addr >> 12) & 0xFF; reg = addr & 0xFFF`. The PCI host bridge owns the decode. Individual PCI devices are never registered as separate `MemoryRegion` entries in the ECAM window. See [QEMU pcie_host.c](https://github.com/qemu/qemu/blob/master/hw/pci/pcie_host.c) and the [Airbus SecLab QEMU PCI deep dive](https://airbus-seclab.github.io/qemu_blog/pci.html).

**gem5**: `GenericPciHost` similarly owns decode. `decodeAddress()` extracts BDF from the physical address using a configurable number of bits per device (12 for ECAM, 8 for CAM). `getDevice()` returns a `PciDevice*` from an internal `devices` map keyed by `PciBusAddr{bus, dev, func}`. See [gem5 GenericPciHost](https://pages.cs.wisc.edu/~swilson/gem5-docs/classGenericPciHost.html).

**SystemC TLM-2.0**: The initiator socket sends the full address. An interconnect component (bus fabric model) decodes the address and routes to the correct target socket. No per-function memory regions are created -- the PCI host bridge target performs internal decode.

**SIMICS**: PCI config space is a separate address space object. The PCI bus object receives config reads/writes and decodes BDF internally to dispatch to the correct device's `read_config` / `write_config` interface methods.

All four major simulators use bus-internal decode, not per-function MemoryMap splitting.

### Creative Approach

**Hybrid: single ECAM MemoryRegion with a 4KB-aligned fast-path cache.** Keep PciBus as a single mapped region (bus-internal decode), but maintain a flat `[Option<&PciEndpoint>; MAX_FUNCTIONS]` array indexed by `(offset >> 12)` for O(1) dispatch instead of HashMap lookup. On hot-plug events, update the array. This gives the simplicity of a single mapping with the dispatch speed of per-function regions.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **PciBus internal decode** (QEMU/gem5 approach) | Single MemoryMap region -- simple FlatView; PciBus owns all PCI semantics; BAR remapping is internal to PciBus; matches all reference implementations | HashMap lookup per config access (cold path, acceptable); PciBus becomes a large struct managing many children |
| **Per-function 4KB MemoryMap regions** | O(1) FlatView dispatch directly to each function; no PciBus decode logic needed | 256 x 32 x 8 = 65536 potential regions destroys FlatView performance; BAR remapping requires MemoryMap mutation from inside Device::write(); loses bus-level semantics (multifunction discovery, type 1 header forwarding); no reference simulator does this |
| **Flat array cache inside PciBus** | O(1) dispatch via array index; still single MemoryMap entry; trivial to invalidate on hot-plug | Wastes ~512KB for a fully populated bus hierarchy (65536 pointers); overkill for typical 8-16 device setups |

### Helm-NG Suitability

The existing `LLD-bus-framework.md` already specifies exactly the right approach: `PciBus` is a `Device` mapped as a single ECAM region in `MemoryMap`, and `PciBus::read()` / `PciBus::write()` call `decode_ecam()` internally to extract BDF and dispatch to the correct `PciEndpoint`. This matches QEMU, gem5, and SIMICS unanimously. The `HashMap<(u8, u8, u8), Box<dyn PciEndpoint>>` is fine for config space (cold path). For BAR MMIO dispatch (hot path), consider the flat array optimization only if profiling shows HashMap overhead. **Recommendation: keep PciBus internal decode as already designed.**

---

## Q4.2 -- MSI-X: MemoryMap MMIO Dispatch vs. Dedicated MSI Routing Shortcut

### Industry Standard

**QEMU**: `msix_notify()` calls `msi_send_message()` which calls `address_space_stl_le()` -- a DMA write to the MSI target address (e.g., the GIC ITS `GITS_TRANSLATER` register or x86 LAPIC). This write goes through the full `AddressSpace` (MemoryMap equivalent), hits `memory_region_dispatch_write()`, and arrives at the interrupt controller's MMIO handler. The address-space path is deliberate: it allows SMMU/IOMMU interception of MSI writes and provides full observability via memory listeners. See [QEMU msix.c](https://github.com/Xilinx/qemu/blob/master/hw/pci/msix.c) and the [QEMU memory API docs](https://www.qemu.org/docs/master/devel/memory.html).

**gem5**: `PciDevice::intrPost()` bypasses the memory system entirely and calls `platform->postInt()` directly -- a shortcut. This is simpler but means MSI writes are invisible to the memory system, preventing IOMMU modeling and making MSI traffic invisible to memory traces.

**Real hardware**: MSI-X is architecturally a posted memory write to a physical address. It goes through the system interconnect and can be intercepted by an IOMMU/SMMU. ARM GICv3 ITS processes MSI writes through the `GITS_TRANSLATER` register at a physical address.

### Creative Approach

**Two-tier MSI path**: Provide a `MsiRouter` trait with a default implementation that does `MemoryMap::write(msi_addr, msi_data)` (the correct architectural path), and an `OptimizedMsiRouter` variant that calls the interrupt controller directly when no SMMU is present. At `elaborate()` time, the platform config selects which path is wired. When an SMMU device is added, the system automatically falls back to the address-space path.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Address-space path** (QEMU) | Architecturally correct; SMMU interception works naturally; MSI writes appear in memory traces; one code path for all interrupt delivery | Slower than direct call (MemoryMap lookup + FlatView binary search + MMIO dispatch); overkill when no SMMU exists |
| **Direct shortcut** (gem5) | Fastest possible interrupt delivery; simple code | Breaks SMMU/IOMMU correctness; MSI writes invisible to tracing; two interrupt delivery mechanisms to maintain |
| **Two-tier with automatic selection** | Correct when SMMU present, fast when absent; single `MsiRouter` trait unifies both | Complexity of the trait + selection logic; risk of misconfiguration if SMMU is added after MSI wiring |

### Helm-NG Suitability

**Recommendation: address-space path as the default, with the shortcut as an optimization for Phase 3+.** For helm-ng Phase 0-2, correctness matters more than MSI latency (MSI delivery is not on the per-instruction hot path). The device calls `self.msi_pin.send(addr, data)`, and the `MsiPin` implementation performs `MemoryMap::write(addr, 4, data)`. This routes through `FlatView` to the GIC ITS's MMIO handler, which is the architecturally correct path. This also ensures that when SMMU support is added, MSI writes are automatically intercepted. The gem5-style shortcut can be added later as an optimization behind a `cfg` flag or timing model selection (`Virtual` mode could use the shortcut since it has no SMMU).

---

## Q4.3 -- I2C Multi-Master Arbitration

### Industry Standard

**QEMU**: `hw/i2c/core.c` models a single-master I2C bus. There is no multi-master arbitration. The `I2CBus` struct has a single master that initiates all transactions. This is sufficient for 99%+ of embedded system modeling use cases.

**gem5**: I2C is not implemented in gem5's standard device library.

**Renode**: The `II2CPeripheral` interface is single-master. No arbitration modeling.

**SystemC**: Academic SystemC-AMS models exist that implement I2C multi-master arbitration with bit-level SDA wired-AND simulation and clock synchronization (see [HAL I2C SystemC-AMS paper](https://hal.science/hal-01335119)), but these are research prototypes, not part of any mainstream simulator's standard library.

**Real-world usage**: Multi-master I2C is rare in practice. Most embedded systems use a single I2C master (the main CPU/MCU). The few multi-master scenarios (e.g., BMC sharing an I2C bus with a host CPU) are typically handled by hardware arbitration that is invisible to software -- the losing master sees a NACK or bus-busy condition and retries.

### Creative Approach

**Software-visible arbitration without bit-level modeling.** Instead of simulating wired-AND at the bit level, model arbitration at the transaction level: when two masters attempt to START simultaneously (within the same simulation tick), a deterministic tie-breaking rule (lower object ID wins) grants the bus. The losing master's `I2cBus::process_control()` sets the BUSY flag and the `BusError::Arbitration` status. This gives software-visible arbitration behavior without cycle-accurate SDA modeling.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Single-master only** (QEMU/Renode) | Simple; covers 99% of use cases; no arbitration complexity; matches all reference implementations | Cannot model BMC shared-bus scenarios; must document limitation |
| **Transaction-level arbitration** | Software sees BUSY/arbitration-lost status; tests multi-master firmware; no bit-level complexity | Still not bit-accurate; adds complexity to I2cBus for a rare use case; deterministic tie-breaking may not match real hardware |
| **Bit-level SDA/SCL simulation** | Cycle-accurate arbitration; models clock stretching | Massive performance cost; requires sub-bit timing model; no mainstream simulator does this; overkill for system simulation |

### Helm-NG Suitability

**Recommendation: single-master I2C as already designed in `LLD-bus-framework.md`, with the limitation documented.** The existing `I2cBus` design is single-master and matches QEMU/Renode. Multi-master arbitration should be deferred indefinitely -- it is an extremely rare requirement and no mainstream simulator implements it. If ever needed (Phase 3+ for BMC/host shared-bus scenarios), the transaction-level approach is the right compromise for helm-ng's abstraction level. The `BusError::Arbitration` variant already exists in the `BusError` enum, which is forward-looking.

---

## Q4.4 -- AMBA AXI Backpressure: Model READY/VALID or Collapse to Zero-Latency?

### Industry Standard

**QEMU**: No AXI bus modeling whatsoever. All MMIO accesses are zero-latency synchronous calls. `Device::read()` and `Device::write()` return immediately. QEMU is purely functional for bus transactions.

**gem5**: `TimingSimpleCPU` models memory access latency (cache hit/miss, DRAM latency) but does not model AXI channel-level READY/VALID handshaking. `AtomicSimpleCPU` collapses everything to zero-latency. gem5's `Ruby` memory model provides detailed bandwidth/latency modeling including interconnect contention, but this is at the cache coherence protocol level (MESI/MOESI), not at the AXI signal level.

**SystemC TLM-2.0**: Two coding styles are relevant. LT (Loosely Timed) uses `b_transport()` -- blocking, zero-latency from the caller's perspective, with an annotated delay returned. AT (Approximately Timed) uses `nb_transport()` with `BEGIN_REQ` / `END_REQ` / `BEGIN_RESP` / `END_RESP` phases. The [ARM AMBA TLM 2.0 Library](https://documentation-service.arm.com/static/647e101c3071ab482ad10798) maps each AXI channel (AW, W, B, AR, R) to VALID/READY phase pairs for AT-level modeling. See [Doulos AT Example](https://www.doulos.com/knowhow/systemc/tlm-20/complete-tlm-20-at-example/).

**SIMICS**: `stall_cycle()` on a transaction stalls the initiator for a specified number of cycles. No AXI-specific modeling, but the timing effect is captured.

**Boot correctness**: AXI backpressure is NOT needed for boot correctness. No guest OS or bootloader depends on observing READY/VALID timing. Backpressure modeling is purely a performance accuracy concern.

### Creative Approach

**TimingModel-gated backpressure.** In `Virtual` timing mode, all bus transactions are zero-latency (QEMU-equivalent). In `Interval` mode, `MemoryMap::read()`/`write()` returns an estimated latency based on a simple bandwidth model (bytes_in_flight / bandwidth_bps). In `Accurate` mode, a per-channel queue enforces `END_REQ` before the next `BEGIN_REQ` (TLM-2.0 AT exclusion rule). This maps cleanly onto helm-ng's `TimingModel` generic parameter.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Zero-latency** (QEMU) | Simplest; fastest simulation speed; sufficient for boot and functional correctness | No interconnect contention modeling; useless for performance analysis of bus-heavy workloads |
| **Estimated latency per access** (gem5 Atomic/Interval) | Simple bandwidth model captures most contention effects; 10-15% MAPE achievable; no per-channel state | Does not model per-channel backpressure; cannot distinguish AR vs AW contention |
| **Per-channel AT modeling** (TLM-2.0 AT) | Signal-accurate AXI behavior; models all 5 channels independently; catches real deadlocks | Massive complexity; requires per-channel queues, phase tracking, exclusion rule enforcement; 10-100x slower; overkill for Phase 0-2 |

### Helm-NG Suitability

**Recommendation: zero-latency for `Virtual` mode (Phase 0-1), estimated latency for `Interval` mode (Phase 1+), defer AT-level per-channel modeling to Phase 3+.** This aligns with helm-ng's `TimingModel` architecture. The `MemInterface` trait already supports three access modes (Atomic, Functional, Timing). In `Atomic` mode, return `(data, estimated_cycles)` where the cycle estimate comes from a simple bandwidth model in the `Interval` timing model. In `Timing` mode, the `Accurate` timing model can eventually model per-channel contention via `EventQueue` callbacks. AXI READY/VALID handshaking at the signal level is out of scope for helm-ng -- it is RTL-level detail that belongs in Verilator, not a system simulator.

---

## Q4.5 -- DMA Engine Memory Access: MemoryMap, BusMaster Trait, or Direct FlatMem?

### Industry Standard

**QEMU**: All DMA devices use `dma_memory_read()` / `dma_memory_write()` which goes through the device's `AddressSpace`. PCI devices specifically use `pci_dma_read()` / `pci_dma_write()` which calls `dma_memory_*` on the PCI device's address space. This address space may include an IOMMU/SMMU memory region that translates IOVA to physical addresses. VirtIO, e1000, and AHCI all use this path. See [QEMU Load and Store APIs](https://www.qemu.org/docs/master/devel/loads-stores.html).

**gem5**: DMA devices use a `DmaPort` which sends `Request`/`Response` packets through the memory hierarchy. The port goes through the normal interconnect, allowing caches and IOMMU to intercept.

**SIMICS**: Devices use `memory_space.write()` which goes through the device's memory space (which may include IOMMU translation).

All three use the address-space path, not direct memory access. The address-space path is essential for IOMMU correctness and observability.

### Creative Approach

**`BusMaster` trait with compile-time selection.** Define a `BusMaster` trait with `dma_read(addr, size)` and `dma_write(addr, size, data)`. Provide two implementations: `MemoryMapBusMaster` (goes through `MemoryMap`, supports SMMU) and `DirectBusMaster` (bypasses to `FlatMem`, fastest, no SMMU). At `elaborate()` time, devices receive a `Box<dyn BusMaster>` based on whether an SMMU is present in the platform. For Phase 0 SE mode with no SMMU, `DirectBusMaster` is the default.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Through MemoryMap** (QEMU/gem5/SIMICS) | SMMU/IOMMU works; DMA visible in traces; architecturally correct; one path for all DMA | Slower (FlatView lookup per DMA access); MemoryMap must be accessible from Device (borrow concern in Rust) |
| **Direct FlatMem** | Fastest; no borrow conflict; trivial implementation | No SMMU support; DMA invisible to tracing; cannot model DMA to MMIO regions; architecturally wrong |
| **BusMaster trait** | Clean abstraction; compile-time or runtime selection; testable interface | Another trait to maintain; adds indirection; must resolve Rust borrow issue (Device holds &mut self, MemoryMap needs &mut) |

### Helm-NG Suitability

**Recommendation: DMA goes through `MemoryMap` via a `DmaContext` reference stored during `elaborate()`.** The Rust borrow challenge (Device::write has `&mut self` while DMA needs `&mut MemoryMap`) is solved the same way QEMU solves it: the DMA reference is stored as an `Arc<RefCell<MemoryMap>>` or similar shared-ownership pattern, obtained during `elaborate()` (rule 6: "store all refs during elaborate()"). The device calls `self.dma_context.write(addr, size, data)` which routes through `MemoryMap` including any SMMU translation. For Phase 0 SE mode where no MMIO devices exist in the DMA path, this still works correctly -- `MemoryMap` routes to RAM regions directly. The `BusMaster` trait abstraction is unnecessary indirection when the `DmaContext` can be a concrete wrapper around the shared `MemoryMap` reference.

---

## Q4.6 -- PCI BAR Re-Programming from Inside a Config Space Write Handler

### Industry Standard

**QEMU**: When the guest writes to a BAR register, `pci_default_write_config()` detects the BAR write and calls `pci_update_mappings()`. This function iterates over the device's I/O regions, computes the new BAR address via `pci_bar_address()`, and calls `memory_region_add_subregion_overlap()` / `memory_region_del_subregion()` to remap the device's BAR-backed `MemoryRegion`. The key insight: QEMU's `MemoryRegion` tree mutation is safe because `pci_update_mappings()` modifies the *tree* (not the `FlatView` directly), and `FlatView` is lazily recomputed on the next access. No borrow conflict exists because tree mutation and access dispatch use different data structures. See [QEMU pci.c](https://github.com/qemu/qemu/blob/master/hw/pci/pci.c) and the [Airbus SecLab PCI slave deep dive](https://airbus-seclab.github.io/qemu_blog/pci_slave.html).

**gem5**: `PciDevice::writeConfig()` calls `pioPort.sendRangeChange()` which notifies the memory system to re-query the device's address ranges. The device does not directly modify the memory map -- it signals that its ranges have changed, and the memory system pulls the new ranges.

### Creative Approach

**Deferred remap via post-write command queue.** Instead of mutating `MemoryMap` from inside `Device::write()`, the device pushes a `MemoryMapCommand::Remap { device_id, old_base, new_base, size }` onto a per-device command queue. After `Device::write()` returns, `MemoryMap` drains the command queue and applies remaps. This cleanly separates the `&mut Device` borrow from the `&mut MemoryMap` borrow.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Lazy FlatView + tree mutation** (QEMU) | Proven at scale; tree mutation is cheap; FlatView rebuilt only on next access; no deferred queue | Requires separate tree vs. flat-view data structures; helm-ng's MemoryRegion enum already supports this |
| **Range change notification** (gem5) | Clean separation; device signals change, memory system pulls new ranges | Pull model requires the memory system to re-query all device ranges; more complex protocol |
| **Post-write command queue** | No borrow conflict; deterministic ordering; testable queue | Extra allocation per BAR write; slight delay before remap takes effect (next access, which is fine); queue must be drained at the right time |

### Helm-NG Suitability

**Recommendation: post-write command queue combined with lazy FlatView recomputation.** When `PciBus::write()` detects a BAR config write, it pushes a `RemapCommand` onto a `Vec<RemapCommand>` stored on the `PciBus` itself. After `MemoryMap::dispatch_write()` returns from calling `PciBus::write()`, it checks `PciBus::pending_remaps()` and applies them. This is a natural fit for Rust's borrow model: `PciBus::write(&mut self)` mutates itself (including the command queue), and then `MemoryMap` (which holds `Box<dyn Device>`) processes the commands after the mutable borrow on the device ends. The existing lazy `FlatView` design (Q26: dirty flag, recomputed on next lookup) handles the rest. The `MemoryRegion::Container` type already supports `add_subregion` / `del_subregion` for BAR remapping.

---

## Q4.7 -- SPI Flash XIP: Dual-View Modeling (Command Registers + Memory-Mapped Read)

### Industry Standard

**QEMU**: SPI flash XIP is handled via the `romd` (ROM-direct) mechanism in `MemoryRegion`. A QSPI controller like `aspeed_smc.c` has two MMIO views: (1) the controller's register set (APB interface -- command registers, status, configuration), and (2) a memory-mapped flash window (AHB interface -- direct read for XIP). In QEMU, the flash window is initially a `MemoryRegion` with `romd=true`, meaning reads go directly to the backing store (ROM-like) while writes go through the MMIO handler (for SPI commands). When the controller enters command mode, `memory_region_rom_device_set_romd(false)` switches the region to pure MMIO mode where all accesses go through the handler. See the [Aspeed SMC discussion](https://lists.gnu.org/archive/html/qemu-arm/2016-07/msg00046.html).

**SiFive SPI in QEMU**: The `sifive_spi` model does NOT implement direct memory-mapped flash mode (XIP). It only provides the SPI controller registers. XIP is listed as unsupported.

**Real hardware**: STM32 QUADSPI, Aspeed SMC, and SiFive FU540 SPI all have two separate bus interfaces: an APB/register interface for commands and an AHB/memory-mapped interface for XIP reads. These are two distinct address ranges in the system memory map.

### Creative Approach

**Mode-switching MemoryRegion with read-path fast-path.** Define the QSPI controller as owning two `MemoryRegion` entries registered in `MemoryMap`: (1) a small MMIO region for control registers, and (2) a larger region that switches between `MemoryRegion::Rom` (XIP mode) and `MemoryRegion::Mmio` (command mode). The mode switch is triggered by the controller's mode register write. When in ROM mode, reads bypass the device entirely and go to the backing store -- maximum performance. When in command mode, reads go through `Device::read()` for SPI command processing.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Two separate MemoryRegions** (registers + XIP window) | Clean separation; XIP reads are ROM-speed; matches hardware's two bus interfaces; mode switch = swap MemoryRegion type | Requires MemoryMap mutation on mode switch (FlatView rebuild); two regions to manage per controller |
| **Single MemoryRegion with mode flag** | Simpler MemoryMap; no FlatView rebuild on mode switch | Every XIP read goes through Device::read() dispatch (slower); mode flag checked on every access (hot path) |
| **romd-style switchable region** (QEMU) | Proven in QEMU; reads bypass handler in ROM mode; transparent mode switching | Requires a `romd` flag in MemoryRegion (new concept for helm-ng); FlatView must understand romd semantics |

### Helm-NG Suitability

**Recommendation: two separate MemoryRegions mapped at different addresses, matching the hardware's two bus interfaces.** The QSPI controller device owns both regions. Region 1 (small, e.g., 64 bytes) is always `MemoryRegion::Mmio` for control registers. Region 2 (large, e.g., 16 MB) starts as `MemoryRegion::Rom` backed by the flash contents and switches to `MemoryRegion::Mmio` when the controller enters command mode. The mode switch calls `MemoryMap::replace_region()` which sets the dirty flag and triggers lazy FlatView rebuild. This avoids adding a `romd` concept to `MemoryRegion` (simpler type system) while achieving the same performance benefit: XIP reads from ROM backing go through the fast `FlatView -> Ram/Rom` path, not through `Device::read()`. This is a Phase 2+ concern since SPI flash is not needed for Phase 0 SE mode.

---

## Q4.8 -- PCIe AER Error Propagation Through Bus Hierarchy

### Industry Standard

**QEMU**: QEMU has basic AER support for passthrough devices (VFIO). Error injection is supported via the QMP command `pcie_aer_inject_error`. However, QEMU does not implement hierarchical AER propagation for emulated devices -- there is no internal model of SERR propagation from endpoint through switch to root port. Error messages are not routed through the PCIe hierarchy. See the [AER in QEMU presentation](http://events17.linuxfoundation.org/sites/events/files/slides/AER%20functionality%20of%20pass-through%20PCI-e%20device%20in%20Qemu.pdf).

**gem5**: No AER modeling. `PciDevice` does not implement advanced error reporting capabilities.

**Real PCIe**: AER error messages propagate from the detecting endpoint up through the hierarchy to the root port. The root port logs the error in its AER capability registers and optionally generates an MSI/INTx interrupt. The Linux kernel's `pcieaer-howto` documents the full flow: correctable errors are logged, non-fatal errors trigger `error_detected()` callbacks in device drivers, fatal errors trigger link reset. See [Linux PCIe AER HOWTO](https://docs.kernel.org/PCI/pcieaer-howto.html).

**Linux driver dependency**: The Linux `pci_error_handlers` callbacks (`error_detected`, `mmio_enabled`, `slot_reset`, `resume`) are registered by PCI device drivers. If AER is not modeled, these callbacks are never exercised in simulation, which means driver error-handling code paths are untested.

### Creative Approach

**Lazy AER with error injection API.** Do not model continuous AER monitoring, but provide a `PciBus::inject_error(bdf, error_type)` method callable from Python/GDB. When called, it: (1) sets the appropriate bits in the endpoint's AER capability registers, (2) propagates an error message up the hierarchy by calling `root_port.on_aer_message(source_id, severity)`, (3) the root port logs the error in its Root Error Status register and fires an MSI if configured. This exercises the guest OS error-handling paths without the overhead of continuous AER monitoring.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **No AER modeling** (current gem5) | Simplest; no overhead; fine for Phase 0-2 | Linux AER driver tests fail; error-handling code paths untested; Linux may warn about missing AER capability |
| **Full hierarchical AER** | Exercises all guest error-handling paths; architecturally complete | Complex (AER capability structure is 48+ bytes of registers per device); needs error classification, masking, logging; overkill for simulation |
| **Error injection API only** | Exercises guest error handling on demand; no overhead when not injecting; Python-scriptable test scenarios | Does not model spontaneous errors; requires manual injection |

### Helm-NG Suitability

**Recommendation: Phase 0-2, no AER modeling. Phase 3+, error injection API.** For early phases, PCI config space should include the AER Extended Capability header (so Linux does not complain about its absence), but with all status bits reading as zero. In Phase 3, add `PciBus::inject_error(bdf, PcieError)` as a Python-callable method. The `PciEndpoint` trait gains an `aer_capability() -> Option<&AerCapability>` method. The `PciBus` propagates errors by walking its internal device tree upward to the root port. This is sufficient for driver testing without the complexity of continuous AER monitoring. The `HelmEventBus` can fire a `HelmEvent::Custom` for AER events, giving Python scripts observability.

---

## Q4.9 -- Bus::attach() and PCIe Hot-Plug

### Industry Standard

**QEMU**: PCIe native hotplug uses `pcie_cap_slot_write_config()` in the root port. When the guest writes to the Slot Control register to power on/off a slot, QEMU calls `pcie_cap_update_power()` which calls `pci_set_power()` on the downstream device. On hot-unplug, `pcie_unplug_device()` calls `hotplug_handler_unplug()` which calls `object_unparent()` -- this recursively tears down the device's memory regions, IRQ connections, and config space. On hot-plug, a new device is created via `device_add` QMP command, the root port's Presence Detect Status bit is set, and an HP interrupt fires. See [QEMU hotplug infrastructure](https://www.linux-kvm.org/images/d/d7/02x07-Aspen-Michael_Roth-QEMU_Hotplug_infrastructure.pdf).

**State unwinding on removal**: QEMU must: (1) remove all BAR `MemoryRegion` subregions from the PCI address space, (2) disconnect MSI/MSI-X vectors, (3) clear the device's config space from the bus, (4) deassert any pending interrupts, (5) remove the device from the QOM tree. Failure to unwind any of these causes guest OS crashes or simulation hangs.

**gem5**: PCI hotplug is not supported in gem5's standard device models.

### Creative Approach

**Two-phase hotplug with frozen validation.** Phase 1 (pre-validate): before any state change, verify that the device can be cleanly removed (no in-flight DMA, no pending interrupts, no active MSI vectors). Phase 2 (atomic remove): if validation passes, perform the removal in a single `World::remove_device()` call that unwinds all state atomically. If validation fails, return an error and leave the system unchanged. This prevents partial-removal bugs.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **No hotplug** (frozen after startup) | Simplest; matches helm-ng's current "frozen after startup" design rule; no state unwinding bugs | Cannot model PCIe hotplug scenarios; some guest OS features untestable |
| **Full hotplug support** (QEMU-style) | Complete PCIe hotplug; exercises guest PCIEHP driver; supports dynamic topology | Complex state unwinding; must handle partial failure; breaks "frozen after startup" invariant; requires careful borrow management in Rust |
| **Two-phase validated hotplug** | Prevents partial-removal bugs; clean error reporting; atomic state change | Still requires breaking the "frozen after startup" rule; validation may be too conservative (rejecting valid removals) |

### Helm-NG Suitability

**Recommendation: no hotplug in Phase 0-2; two-phase validated hotplug in Phase 3+.** The current helm-ng design rule ("wiring graph frozen after startup") is correct for early phases. Hot-plug requires relaxing this rule, which should be done carefully. When implemented, `Bus::detach(address) -> Result<Box<dyn BusDevice>, DetachError>` should: (1) verify no in-flight DMA via `dma_context.has_pending(device_id)`, (2) deassert all interrupts, (3) remove BAR regions from `MemoryMap` (pushing `RemapCommand::Remove` entries), (4) remove the device from the bus's internal map, (5) fire `HelmEvent::DeviceSignal { asserted: false }` for presence detect. The `World::remove_device()` orchestrates this sequence. The `PciBus::write()` handler for Slot Control register triggers the sequence asynchronously via `EventQueue` (deferred to next tick) to avoid re-entrancy issues.

---

## Q4.10 -- USB xHCI: DMA Ring Polling via EventQueue, Device::advance(), or Doorbell

### Industry Standard

**QEMU**: xHCI in `hcd-xhci.c` is purely doorbell-driven. When the guest writes to a Doorbell Register, `xhci_doorbell_write()` is called, which dispatches to `xhci_process_commands()` (for doorbell 0 / command ring) or kicks the endpoint ring via `xhci_kick_ep()`. Ring processing is synchronous within the doorbell write handler -- TDs are processed, data is DMA'd, and completion events are posted to the event ring, all before the doorbell write returns. No QEMU timers are used for ring polling. See [QEMU hcd-xhci.c](https://github.com/qemu/qemu/blob/master/hw/usb/hcd-xhci.c).

**QEMU timeout handling**: If the guest does not ring the doorbell, nothing happens. There is no polling fallback. This matches real hardware behavior -- the xHC only processes rings when doorbells are rung.

**VirtIO comparison**: VirtIO devices are also doorbell-driven (`virtio_queue_notify()`), not timer-polled. The guest writes to the notification address, and the device processes the virtqueue synchronously.

**Real hardware**: xHCI spec mandates doorbell-driven processing. The xHC does not poll transfer rings -- it processes them only when the host driver rings the doorbell. The only timer-driven aspect is command timeout (host driver sets a timer, not the controller).

### Creative Approach

**Doorbell-driven with deferred completion via EventQueue.** The doorbell write triggers TD parsing and DMA initiation, but completion events are posted to the event ring after a configurable delay via `EventQueue::schedule(tick + usb_latency, complete_td)`. This models USB transfer latency (e.g., bulk/interrupt endpoint scheduling intervals) without polling. The delay is zero in `Virtual` timing mode (pure doorbell-driven, like QEMU) and non-zero in `Interval`/`Accurate` modes.

### Pros / Cons

| Option | Pros | Cons |
|--------|------|------|
| **Pure doorbell-driven** (QEMU/VirtIO) | Simplest; matches xHCI spec; no timer overhead; synchronous completion; proven in QEMU | No USB transfer latency modeling; all transfers complete instantly; unrealistic timing for performance analysis |
| **EventQueue timer polling** | Could model periodic interrupt endpoint processing; catches missed doorbells | xHCI spec says no polling; wastes EventQueue entries; incorrect behavior (processing without doorbell); adds latency even when guest is fast |
| **Doorbell + deferred completion** | Architecturally correct (doorbell triggers processing); models USB latency; timing-model-gated (zero delay in Virtual mode) | More complex than pure doorbell; EventQueue callback for every TD completion; must handle re-entrant doorbell during deferred processing |
| **Device::advance() polling** | Allows xHC to do periodic housekeeping | advance() is called every N ticks regardless; overhead even when no USB activity; xHCI does not need periodic processing |

### Helm-NG Suitability

**Recommendation: pure doorbell-driven for Phase 0-2 (QEMU-equivalent), doorbell with deferred EventQueue completion for Phase 3+ timing modes.** The xHCI device model implements `Device::write()` for the doorbell register. On doorbell write, `xhci_kick_ep()` parses TDs from the transfer ring, performs DMA via `dma_context.read()`/`write()`, and posts completion events to the event ring. In `Virtual` mode, this is all synchronous (zero-latency, like QEMU). In `Interval`/`Accurate` modes, the DMA and completion posting are scheduled via `EventQueue` with appropriate USB latency annotations. `Device::advance()` is not needed and should not be used for xHCI -- the xHC has no autonomous periodic behavior (interrupt endpoints are driven by the host driver's scheduling, not the controller's internal timer). The same doorbell-driven pattern applies to VirtIO devices.

---

Sources referenced throughout this analysis:
- [QEMU pcie_host.c](https://github.com/qemu/qemu/blob/master/hw/pci/pcie_host.c)
- [QEMU pci.c](https://github.com/qemu/qemu/blob/master/hw/pci/pci.c)
- [QEMU hcd-xhci.c](https://github.com/qemu/qemu/blob/master/hw/usb/hcd-xhci.c)
- [QEMU Memory API](https://www.qemu.org/docs/master/devel/memory.html)
- [QEMU Load and Store APIs](https://www.qemu.org/docs/master/devel/loads-stores.html)
- [QEMU MSI-X (Xilinx fork)](https://github.com/Xilinx/qemu/blob/master/hw/pci/msix.c)
- [Airbus SecLab QEMU PCI deep dive](https://airbus-seclab.github.io/qemu_blog/pci.html)
- [Airbus SecLab QEMU PCI slave devices](https://airbus-seclab.github.io/qemu_blog/pci_slave.html)
- [gem5 GenericPciHost](https://pages.cs.wisc.edu/~swilson/gem5-docs/classGenericPciHost.html)
- [gem5 PciDevice](https://pages.cs.wisc.edu/~swilson/gem5-docs/classPciDevice.html)
- [Linux PCIe AER HOWTO](https://docs.kernel.org/PCI/pcieaer-howto.html)
- [AER in QEMU (Fujitsu presentation)](http://events17.linuxfoundation.org/sites/events/files/slides/AER%20functionality%20of%20pass-through%20PCI-e%20device%20in%20Qemu.pdf)
- [Doulos TLM-2.0 AT Example](https://www.doulos.com/knowhow/systemc/tlm-20/complete-tlm-20-at-example/)
- [ARM AMBA TLM 2.0 Library](https://documentation-service.arm.com/static/647e101c3071ab482ad10798)
- [TLM-2.0 Language Reference Manual](https://www.accellera.org/images/downloads/standards/systemc/TLM_2_0_LRM.pdf)
- [SystemC AXI Modeling (Medium)](https://medium.com/@techAsthetic/systemc-modeling-of-axi-protocol-lt-and-at-implementations-049ceecaebf2)
- [QEMU hotplug infrastructure](https://www.linux-kvm.org/images/d/d7/02x07-Aspen-Michael_Roth-QEMU_Hotplug_infrastructure.pdf)
- [I2C SystemC-AMS modeling (HAL)](https://hal.science/hal-01335119)
- [QEMU Aspeed SMC XIP discussion](https://lists.gnu.org/archive/html/qemu-arm/2016-07/msg00046.html)
- [Xilinx libsystemctlm-soc](https://github.com/Xilinx/libsystemctlm-soc)

---

