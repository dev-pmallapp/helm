# SIMICS Research: HAPs, Timing, Events, Processor Model, DML, Port Objects

> Source: Deep research into Intel SIMICS API (SIMICS 6/7 + legacy 3.x).
> Used to inform helm-ng `HelmEventBus`, timing model, `HelmEngine`, and device port design.

---

## 1. SIMICS HAP System (Hardware Action Points)

### How SIMICS Does It

HAPs are a **named, string-keyed callback registry**. A HAP fires synchronously — all callbacks for that HAP name run in the simulation thread before control returns to the caller. No priority ordering: callbacks fire FIFO within a HAP name.

```c
// Subscribe globally (fires for any object that triggers this HAP)
hap_handle_t SIM_hap_add_callback(
    const char *hap_name,   // "Core_Exception", "Core_Control_Register_Write", etc.
    obj_hap_func_t func,    // callback — cast required, signature is HAP-specific
    lang_void *user_data    // passed back to every callback invocation
);

// Subscribe to a specific object only
hap_handle_t SIM_hap_add_callback_obj(
    const char *hap_name, conf_object_t *obj, int flags,
    obj_hap_func_t func, lang_void *user_data
);

// Subscribe with index filter (e.g., only exception 14 = page fault)
hap_handle_t SIM_hap_add_callback_index(
    const char *hap_name, obj_hap_func_t func,
    lang_void *user_data, integer_t index
);

// Cancel
void SIM_hap_delete_callback_id(const char *hap_name, hap_handle_t handle);

// Fire from C
void SIM_c_hap_occurred(hap_type_t hap, conf_object_t *obj, integer_t index, ...);

// Define a custom HAP type (once, in init_local)
hap_type_t SIM_hap_add_type(
    const char *name,        // "My_Custom_Event"
    const char *params,      // "ii" — two integer params
    const char *param_names, // "address,size"
    const char *index,       // NULL or index param name
    const char *desc,
    int         old_count    // 0 for new HAPs
);
```

**Standard HAP callback signatures:**

| HAP Name | Callback signature |
|----------|--------------------|
| `Core_Exception` | `(void *ud, conf_object_t *cpu, int64 exception_number)` |
| `Core_Control_Register_Write` | `(void *ud, conf_object_t *cpu, int64 reg_num, int64 value)` |
| `Core_External_Interrupt` | `(void *ud, conf_object_t *cpu, int64 interrupt_number)` |
| `Core_Breakpoint` | `(void *ud, conf_object_t *trigger_obj, int64 bp_number, int64 bp_address)` |
| `Core_Magic_Instruction` | `(void *ud, conf_object_t *cpu, int64 magic_value)` |
| `Core_Simulation_Stopped` | `(void *ud, conf_object_t *obj, int64 exception, const char *error_str)` |

**HAP callbacks from Python:**
```python
handle = SIM_hap_add_callback("Core_Exception",
    lambda ud, cpu, exc: print(f"Exception {exc} on {cpu.name}"), None)
SIM_hap_delete_callback_id("Core_Exception", handle)
```

**Key rules:**
- Callbacks are NOT saved in checkpoints — re-register in `init_local()` on every load
- `SIM_break_simulation()` may be called from inside a HAP callback to halt simulation
- Index-filtered variants avoid unnecessary callback invocations for high-frequency HAPs

### ✅ Helm Design Choice: `HelmEventBus`

SIMICS HAPs → `HelmEventBus` in `helm-devices/src/bus/event_bus` crate. Key adaptations for Rust:

```rust
/// Typed HAP equivalent — no string dispatch in the hot path
#[derive(Debug, Clone)]
pub enum HelmEvent {
    // CPU events
    Exception       { cpu: ObjectRef, vector: u32, tval: u64, pc: u64 },
    CsrWrite        { cpu: ObjectRef, csr: u16, old: u64, new: u64 },
    ExternalIrq     { cpu: ObjectRef, irq_num: u32 },
    // Debug events
    Breakpoint      { cpu: ObjectRef, addr: u64, bp_id: u32 },
    Watchpoint      { cpu: ObjectRef, addr: u64, size: usize, write: bool },
    MagicInsn       { cpu: ObjectRef, pc: u64, value: u64 },
    // System events
    SimulationStop  { reason: StopReason },
    ModeChange      { from: ExecMode, to: ExecMode },
    // Memory events
    MemWrite        { addr: u64, size: usize, val: u64, cycle: u64 },
    // Syscall events (SE mode)
    SyscallEnter    { nr: u64, args: [u64; 6] },
    SyscallReturn   { nr: u64, ret: u64 },
    // Device events
    DeviceSignal    { device: ObjectRef, port: String, asserted: bool },
    // Extensible
    Custom          { name: &'static str, data: Arc<dyn Any + Send + Sync> },
}

/// Opaque handle — cancel on drop or explicit cancel()
pub struct EventHandle(u64);

pub struct HelmEventBus {
    subscribers: RwLock<HashMap<HelmEventKind, Vec<(u64, Box<dyn Fn(&HelmEvent) + Send + Sync>)>>>,
    next_id: AtomicU64,
}

impl HelmEventBus {
    /// Subscribe to all events of a kind (global, like SIM_hap_add_callback)
    pub fn subscribe<F>(&self, kind: HelmEventKind, f: F) -> EventHandle
    where F: Fn(&HelmEvent) + Send + Sync + 'static;

    /// Subscribe with filter predicate (like SIM_hap_add_callback_index)
    pub fn subscribe_filtered<F, P>(&self, kind: HelmEventKind, pred: P, f: F) -> EventHandle
    where P: Fn(&HelmEvent) -> bool + Send + Sync + 'static,
          F: Fn(&HelmEvent) + Send + Sync + 'static;

    /// Fire synchronously — all subscribers for this kind called before return
    pub fn fire(&self, event: HelmEvent);

    /// Cancel subscription
    pub fn cancel(&self, handle: EventHandle);
}

/// TraceLogger is a HelmEventBus subscriber — not a separate system
/// Python callbacks subscribe via PyO3 lambda wrapping
```

**HAP checkpoint rule applied to helm-ng:**
`HelmEventBus` subscriptions are NOT saved to checkpoints. Components re-subscribe in their `init()` method on every load (initial + restore). This matches SIMICS behavior exactly.

---

## 2. SIMICS Timing Model and Temporal Decoupling

### How SIMICS Does It

SIMICS models time in two units:
- `cycles_t` — processor cycle count (integer, `int64_t`)
- `simtime_t` — virtual seconds (double, `SIM_time(obj)`)

The conversion: `SIM_time(obj) = SIM_cycle_count(obj) / obj->freq_hz`

**Temporal decoupling** is the key speed technique:

Instead of running one instruction globally then synchronizing all components, SIMICS gives each vCPU a **quantum** (500K–1M instructions typically) and lets it run independently. Devices are event-driven — they don't run in lockstep with the CPU. A device's timer event fires when the CPU's simulated clock reaches the event's scheduled time.

```c
// CPU implements the "execute" interface
typedef struct execute_interface {
    void (*run)(conf_object_t *obj);   // run until quantum exhausted or stopped
    void (*stop)(conf_object_t *obj);  // stop running
} execute_interface_t;

// Scheduler calls run() on each registered execute object in round-robin
// Each run() call exhausts its time quantum before returning

// Query current time
cycles_t SIM_cycle_count(conf_object_t *obj);
double   SIM_time(conf_object_t *obj);

// Control simulation
void SIM_continue(int64 steps);        // run N steps (0 = forever)
void SIM_break_simulation(const char *msg);  // halt (from callback or script)
```

**The quantum discipline:**
- Each vCPU runs its `execute` quantum without checking global state
- At quantum boundary: check for posted events, advance simulation clock
- Devices post events into the queue; they fire when the CPU's clock reaches them
- No locking needed between vCPUs during a quantum — each is independent
- **Hypersimulation**: if CPU is idle (waiting for interrupt), skip ahead to the next event's scheduled time without simulating individual cycles

### ✅ Helm Design Choice: `Execute` Trait + Temporal Decoupling

```rust
/// CPU models implement Execute — called by the scheduler
pub trait Execute: Send {
    /// Run for up to `budget` cycles. Return reason for stopping early (or QuantumDone).
    fn run(&mut self, budget: Cycles, world: &mut World) -> StopReason;

    /// Stop running (called by scheduler when simulation is halted)
    fn stop(&mut self);

    /// Current cycle count for this hart
    fn cycle_count(&self) -> Cycles;

    /// Hypersimulation: can we skip to `target` without simulating?
    /// Returns true if the CPU is idle until at least `target` cycles.
    fn can_skip_to(&self, target: Cycles) -> bool;
}

pub enum StopReason {
    QuantumExhausted,
    BreakpointHit { addr: u64 },
    SimulationHalted(String),
    IllegalInstruction { pc: u64, raw: u32 },
}

/// Scheduler — round-robins Execute implementors with temporal decoupling
pub struct Scheduler {
    executors: Vec<Box<dyn Execute>>,
    quantum:   Cycles,         // typical: 100_000–1_000_000
}

impl Scheduler {
    pub fn run_until_halt(&mut self, world: &mut World) -> StopReason {
        loop {
            for cpu in &mut self.executors {
                if cpu.can_skip_to(world.event_queue.peek_next()) {
                    // Hypersimulation: skip idle CPU forward
                    world.drain_events_until(world.event_queue.peek_next());
                } else {
                    let reason = cpu.run(self.quantum, world);
                    if let StopReason::SimulationHalted(_) = reason { return reason; }
                }
                world.drain_events_until(cpu.cycle_count());
            }
        }
    }
}
```

**Temporal decoupling in `HelmEngine<T>`:**
`HelmEngine<T>` implements `Execute`. Its `run()` executes instructions until the budget is exhausted or a stop condition is hit. The `TimingModel` generic parameter tracks simulated time — `Virtual` advances a virtual clock, `Interval` applies timing analytically, `Accurate` models cycle-by-cycle.

---

## 3. SIMICS Event Posting System

### How SIMICS Does It

```c
// Register an event class (once, in init_local)
event_class_t *SIM_register_event(
    const char *name,
    conf_class_t *cls,
    event_class_flag_t flags,
    void (*callback)(conf_object_t *obj, void *data),
    attr_value_t (*get_value)(conf_object_t *obj, void *data),  // for checkpointing
    void (*set_value)(conf_object_t *obj, attr_value_t val),    // for checkpointing
    char *(*describe)(conf_object_t *obj, void *data),
    void (*destroy)(conf_object_t *obj, void *data)
);

// Post event relative to current time
void SIM_event_post_cycle(
    conf_object_t *clock,      // the clock object (usually the CPU)
    event_class_t *event_cls,
    conf_object_t *obj,        // will be passed to callback as first arg
    cycles_t       cycles,     // fire after this many cycles from now
    void          *data        // arbitrary user data passed to callback
);

void SIM_event_post_time(conf_object_t *clock, event_class_t *cls,
                          conf_object_t *obj, double seconds, void *data);

// Cancel
void SIM_event_cancel_time(conf_object_t *clock, event_class_t *cls,
                             conf_object_t *obj, int (*pred)(lang_void*, lang_void*),
                             lang_void *match_data);

// Query
cycles_t SIM_event_find_next_cycle(conf_object_t *clock, event_class_t *cls,
                                    conf_object_t *obj, int (*pred)(lang_void*, lang_void*),
                                    lang_void *match_data);
```

**UART TX delay example (DML):**
```dml
event tx_complete is uint64_time_event {
    method event(uint64 data) {
        tx_done = true;
        call $assert_irq();
    }
}
// Post: fire in 1ms simulated time
tx_complete.post(1e-3, 0);
```

**Checkpoint behavior:** Events are saved ONLY if `get_value`/`set_value` callbacks are provided. Without them, events are cancelled on checkpoint save and not restored. Stateless one-shot events (e.g., a watchdog timer) that fire rarely can omit these. Frequent recurrent events (UART poll, periodic timer) MUST implement serialization.

### ✅ Helm Design Choice: `EventQueue` with Typed Events

```rust
/// Event registered once — equivalent to SIMICS event_class_t
pub struct EventClass {
    pub name:     &'static str,
    pub callback: Box<dyn Fn(&mut World, EventData) + Send + Sync>,
    /// If Some — event survives checkpoint/restore
    pub serialize: Option<EventSerialize>,
}

pub struct EventSerialize {
    pub save:    Box<dyn Fn(&EventData) -> AttrValue + Send + Sync>,
    pub restore: Box<dyn Fn(AttrValue) -> EventData + Send + Sync>,
}

/// Opaque handle (cancel by id)
pub struct EventId(u64);

/// The queue — min-heap by (fire_at, sequence)
pub struct EventQueue {
    queue:   BinaryHeap<PendingEvent>,
    next_id: u64,
    now:     Cycles,
}

impl EventQueue {
    /// Post event N cycles from now
    pub fn post_cycles(&mut self, class: &EventClass, after: Cycles,
                        data: EventData) -> EventId;

    /// Run all events scheduled up to and including `until`
    pub fn drain_until(&mut self, until: Cycles, world: &mut World);

    pub fn cancel(&mut self, id: EventId) -> bool;
    pub fn find_next(&self, class_name: &str) -> Option<Cycles>;
    pub fn current_tick(&self) -> Cycles;
}
```

**Rule applied from SIMICS:** If an `EventClass` has no `serialize`, its pending events are **dropped on checkpoint save**. Components that post recurrent timer events (UART baud clock, periodic interrupt) must provide `serialize` to survive checkpoint/restore.

---

## 4. SIMICS Processor Model Integration

### How SIMICS Does It

A CPU model implements several interfaces, of which the most critical are:

```c
// execute interface — the scheduler calls run() each quantum
typedef struct execute_interface { void (*run)(conf_object_t*); void (*stop)(conf_object_t*); } execute_interface_t;

// processor_info_v2 — metadata and utilities
typedef struct processor_info_v2_interface {
    tuple_int_string_t (*disassemble)(conf_object_t*, generic_address_t, int sub_op);
    physical_block_t   (*logical_to_physical)(conf_object_t*, logical_address_t, access_t);
    generic_address_t  (*get_program_counter)(conf_object_t*);
    int                (*architecture_name)(conf_object_t*, char*, size_t);
    // ...
} processor_info_v2_interface_t;

// int_register — register file access (used by debugger, scripting)
typedef struct int_register_interface {
    int  (*get_number)(conf_object_t *obj, const char *name);
    int  (*read)(conf_object_t *obj, int reg_num);
    void (*write)(conf_object_t *obj, int reg_num, uint64 value);
    // ...
} int_register_interface_t;

// cycle interface — clock queries
typedef struct cycle_interface {
    cycles_t (*get_cycle_count)(conf_object_t *obj);
    double   (*get_time)(conf_object_t *obj);
    // ...
} cycle_interface_t;
```

Memory operations use `generic_transaction_t*` — a struct carrying address, size, read/write flag, data buffer, and endianness. The `io_memory` interface's `operation()` receives these.

### ✅ Helm Design Choice: `Hart` + Named Interfaces

The `Hart` trait provides `Execute` (for scheduling) and exposes named interfaces (for runtime discovery by debugger, Python, GDB):

```rust
pub trait Hart: Execute + SimObject {
    // processor_info_v2 equivalent
    fn disassemble(&self, addr: u64) -> String;
    fn logical_to_physical(&self, vaddr: u64, access: AccessType) -> Option<u64>;
    fn get_pc(&self) -> u64;
    fn isa_name(&self) -> &'static str;   // "riscv64", "aarch64", "aarch32"

    // int_register equivalent — named register access (for GDB/Python)
    fn reg_by_name(&self, name: &str) -> Option<u32>;   // name → index
    fn read_reg(&self, idx: u32) -> u64;
    fn write_reg(&mut self, idx: u32, val: u64);
    fn reg_names(&self) -> &[&'static str];

    // cycle equivalent
    fn cycle_count(&self) -> Cycles;
    fn time_seconds(&self) -> f64;

    // ThreadContext for external-facing cold-path access
    fn thread_context(&mut self) -> &mut dyn ThreadContext;
}
```

The `Hart` registers its interfaces with the `InterfaceRegistry` at `init()` time, so the GDB stub and Python inspection can discover them by name without knowing the concrete type.

---

## 5. DML Device Modeling — Key Patterns

### How SIMICS Does It

DML (Device Modeling Language) compiles to C. Key constructs:

**Bank / Register / Field hierarchy:**
```dml
bank regs {
    // register at offset 0x00, size 4 bytes
    register STATUS @ 0x00 is (read, write) {
        field TX_EMPTY [0]  is (read, write);
        field RX_FULL  [1]  is (read, write);
        method write(uint64 val) {
            this.val = val;
            call $update_irq();
        }
    }
    register CONTROL @ 0x04;
}
```

**Connect (outgoing interface reference):**
```dml
connect irq_dev {
    param documentation = "Interrupt output — wire to interrupt controller";
    param configuration = "required";
    interface signal;      // must implement signal_interface_t
}
// Usage: irq_dev.signal.signal_raise();
```

**Port (incoming interface implementation):**
```dml
port IRQ_IN[i < 8] {
    implement signal {
        method signal_raise() { pending |= (1u << i); call $update_cpu(); }
        method signal_lower() { pending &= ~(1u << i); call $update_cpu(); }
    }
}
saved uint8 pending = 0;
```

**Events (internal timers):**
```dml
event tx_done is uint64_time_event {
    method event(uint64 data) {
        tx_busy = false;
        call $assert_irq();
    }
}
// Post: after (baud_period s): tx_done.event(0);
```

**Saved state (checkpointed):**
```dml
saved bool irq_raised = false;   // auto-checkpointed; survives save/restore
```

**The `after` statement** is syntactic sugar for `SIM_event_post_time` — no manual event class registration needed.

### ✅ Helm Design Choice: Proc-Macro `#[register_bank]`

DML's `bank/register/field` hierarchy → Rust proc-macros:

```rust
/// Declare a memory-mapped register bank.
/// Generates: MmioHandler impl dispatching reads/writes by offset.
/// Generates: AttrDescriptor for each field (for checkpoint/introspection).
#[register_bank(offset_type = u64, register_size = 4)]
struct UartRegs {
    /// STATUS register at offset 0x00
    #[register(offset = 0x00, access = ReadWrite)]
    status: StatusReg,

    /// CONTROL register at offset 0x04
    #[register(offset = 0x04, access = ReadWrite)]
    control: ControlReg,
}

/// Individual register with bit fields
#[register]
struct StatusReg {
    #[field(bits = 0..=0, access = ReadWrite)]
    tx_empty: bool,

    #[field(bits = 1..=1, access = ReadOnly)]
    rx_full: bool,
}
```

The `#[register_bank]` proc-macro generates:
- `MmioHandler` impl — dispatches reads/writes to the correct register by offset
- `#[serde(serialize, deserialize)]` — register state survives checkpoint
- `AttrDescriptor` entries — each register is introspectable from Python
- Write callbacks — each `#[register]` can have a `fn on_write(&mut self, old: u32, new: u32)`

**Connect → `Connect<T>` field:**
```rust
/// Typed outgoing interface reference. Set at elaborate() time.
/// Equivalent to DML `connect irq_dev { interface signal; }`.
pub struct Connect<T: Interface> {
    target: Option<(HelmObjectId, String)>,  // (object, port_name)
    cached: Option<Arc<T>>,
}

impl<T: Interface> Connect<T> {
    pub fn call(&self) -> &T;          // panics if not connected
    pub fn try_call(&self) -> Option<&T>;
    pub fn is_connected(&self) -> bool;
}

// Usage in device
pub struct MyDevice {
    irq: Connect<SignalInterface>,   // DML: connect irq { interface signal; }
}
impl MyDevice {
    fn assert_irq(&self) { self.irq.call().signal_raise(); }
}
```

---

## 6. SIMICS Port Objects and the `signal` Interface

### How SIMICS Does It

The `signal_interface_t` is the canonical SIMICS interrupt/GPIO signal pattern:

```c
typedef struct signal_interface {
    void (*signal_raise)(conf_object_t *obj);
    void (*signal_lower)(conf_object_t *obj);
} signal_interface_t;
```

A device exposes interrupt outputs as named **port objects** — child `conf_object_t` instances under the device. An interrupt controller exposes interrupt inputs the same way.

```c
// Register IRQ output port on the timer device
SIM_register_port_interface(timer_class, "signal", &timer_irq_out_iface, "IRQ_OUT", "...");
// Creates child object: timer.port.IRQ_OUT

// Register 8 IRQ input ports on the interrupt controller
for (int i = 0; i < 8; i++) {
    char port_name[32]; snprintf(port_name, sizeof(port_name), "IRQ[%d]", i);
    SIM_register_port_interface(plic_class, "signal", &plic_irq_in_ifaces[i], port_name, "...");
}
// Creates children: plic.port.IRQ[0] ... plic.port.IRQ[7]
```

**Python wiring** — set as an attribute on the device, value is `(target_obj, port_name)`:
```python
# Wire timer's IRQ output to PLIC input port 3
my_timer.irq_dev = (my_plic, "IRQ[3]")
```

**The canonical DML full pattern:**

```dml
// Timer device — has IRQ output
connect irq_dev { param configuration = "required"; interface signal; }
saved bool irq_raised = false;
method assert_irq() {
    if (!irq_raised) { irq_raised = true; irq_dev.signal.signal_raise(); }
}

// Interrupt controller — has 8 IRQ inputs as ports
port IRQ[i < 8] {
    implement signal {
        method signal_raise() { pending |= (1u << i); call $update_cpu(); }
        method signal_lower() { pending &= ~(1u << i); call $update_cpu(); }
    }
}
```

### ✅ Helm Design Choice: `SignalInterface` + `Port<T>` + `Connect<T>`

```rust
/// The canonical signal interface — matches SIMICS signal_interface_t exactly
pub struct SignalInterface {
    pub raise: fn(&mut HelmObject),
    pub lower: fn(&mut HelmObject),
}

/// An input port — the device implements this to receive signals
/// Equivalent to DML `port IRQ[i < N] { implement signal { ... } }`
pub struct Port<T: Interface> {
    pub name:  String,         // "IRQ[3]"
    vtable:    T,              // the interface impl
}

/// Device field for outgoing connection — wired at elaborate() time
/// Equivalent to DML `connect irq_dev { interface signal; }`
pub struct Connect<T: Interface> {
    target:   Option<(HelmObjectId, String)>,   // (object id, port name)
    iface:    Option<Arc<T>>,                    // cached after elaborate
}

/// Registration — device registers its ports at init() time
/// (analogous to SIM_register_port_interface)
impl InterfaceRegistry {
    pub fn register_port<T: Interface + 'static>(
        &mut self,
        class: &'static str,
        iface_name: &'static str,
        port_name: &str,
        vtable: T,
    );
}
```

**Platform wiring in Python config** — identical pattern to SIMICS:
```python
# Wire timer's IRQ output port to PLIC input port 33
timer.irq_dev = (plic, "IRQ[33]")
```

This sets `timer`'s `irq_dev` attribute to `AttrValue::Port(plic_id, "IRQ[33]".into())`. At `finalize()`, `Connect<SignalInterface>` resolves this to a cached interface reference.

**Interrupt state checkpointing** — following SIMICS's Model Development Checklist:
```rust
// The IRQ assertion state MUST be a Required attribute
// so it is correctly restored after checkpoint/restore.
// (Equivalent to DML's `saved bool irq_raised = false;`)
#[attr(kind = Required, name = "irq_raised")]
irq_raised: bool,
```

---

## Cross-Cutting SIMICS Design Principles Applied to Helm

### The `signal` pattern is the universal interrupt mechanism

Both directions use the same `signal_interface_t` struct:
- Device **output**: `Connect<SignalInterface>` — `signal_raise()` fires into whatever is connected
- Controller **input**: `Port<SignalInterface>` — receives signals, updates pending bitmap
- Platform config wires them: `dev.irq_out = (ctrl, "IRQ[N]")`
- Neither device nor controller knows about each other's class — fully decoupled

### Named interface registry enables runtime plugin discovery

Any code can ask: `registry.get::<SignalInterface>(obj, "signal")` — works for built-in and plugin devices alike. No trait bound at compile time. This is the critical enabler for `.so` plugin loading.

### `after (delay s): method()` = post to EventQueue

DML's `after` is syntactic sugar that posts to the EventQueue. In Rust, wrap it as:
```rust
// In a device method:
self.event_queue.post_cycles(
    &TX_DONE_EVENT,         // EventClass registered once
    baud_period_cycles,
    EventData::None,
);
```

### HAP subscriptions ≠ checkpoint state

Never checkpoint the HelmEventBus subscription list. Components re-subscribe in `init()`. Events in the EventQueue ARE checkpointed if their `EventClass` has `serialize`.
