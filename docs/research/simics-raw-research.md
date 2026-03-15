# Intel SIMICS Simulator API Design — Research Report

**Purpose:** Inform the design of a Rust simulator inspired by SIMICS.
**SIMICS versions referenced:** Primarily SIMICS 6/7 (public docs), with some legacy 3.x references where they remain accurate.
**Sources:** Intel TSFFS documentation, SFU Simics Programming Guide, intel/device-modeling-language GitHub, intel/simulator-bindings (Rust).

---

## Table of Contents

1. [HAP System (Hardware Action Points)](#1-hap-system)
2. [Timing Model and Temporal Decoupling](#2-timing-model-and-temporal-decoupling)
3. [Event Posting System](#3-event-posting-system)
4. [Processor (CPU) Model Integration](#4-processor-cpu-model-integration)
5. [DML Device Modeling — Key Patterns](#5-dml-device-modeling)
6. [Port Objects and the Signal Interface](#6-port-objects-and-the-signal-interface)
7. [SIMICS Object Model Foundation](#7-simics-object-model-foundation)
8. [Python API Layer](#8-python-api-layer)
9. [Checkpointing Considerations](#9-checkpointing-considerations)
10. [Rust Design Implications](#10-rust-design-implications)

---

## 1. HAP System

HAPs (Hardware Action Points) are SIMICS's callback/event-bus mechanism. A HAP is a named occurrence in the simulation — either target-hardware-related (processor exception, interrupt, magic instruction) or simulator-internal (simulation stopped, object created). Multiple callbacks can be registered per HAP name; all fire when the HAP occurs.

### 1.1 Architecture

HAPs form a named callback registry, not a typed event bus. The name is a string (`"Core_Exception"`), and the callback signature is HAP-type-specific. SIMICS does not enforce callback type at compile time in C — the caller casts to `obj_hap_func_t`.

HAP callbacks fire synchronously in the simulation thread. They must not advance simulated time and must return `void`. `SIM_break_simulation()` may be called from within a HAP callback to halt the simulation.

### 1.2 Registration API

```c
// Subscribe globally (fires for any object that triggers this HAP)
hap_handle_t SIM_hap_add_callback(
    const char       *hap_name,   // e.g., "Core_Exception"
    obj_hap_func_t    func,       // callback function pointer (cast required)
    lang_void        *user_data   // arbitrary user context, passed back to callback
);

// Subscribe restricted to a specific object
hap_handle_t SIM_hap_add_callback_obj(
    const char       *hap_name,
    conf_object_t    *obj,        // only fire when THIS object triggers the HAP
    int               flags,      // 0 or Sim_HAP_Flag_Obj_Hierarchy
    obj_hap_func_t    func,
    lang_void        *user_data
);

// Subscribe restricted to a single index value (e.g., register number, exception number)
hap_handle_t SIM_hap_add_callback_index(
    const char       *hap_name,
    obj_hap_func_t    func,
    lang_void        *user_data,
    integer_t         index       // e.g., register number for Core_Control_Register_Write
);

// Subscribe for a range of index values
hap_handle_t SIM_hap_add_callback_range(
    const char       *hap_name,
    obj_hap_func_t    func,
    lang_void        *user_data,
    integer_t         start,
    integer_t         end
);
```

`hap_handle_t` is an opaque integer handle used for later deletion.

### 1.3 Unsubscription API

```c
// Remove by matching (hap_name, func, user_data) — must match exactly as registered
void SIM_hap_delete_callback(
    const char       *hap_name,
    obj_hap_func_t    func,
    lang_void        *user_data
);

// Remove by handle (preferred; unambiguous)
void SIM_hap_delete_callback_id(
    const char       *hap_name,
    hap_handle_t      handle
);

// Object-scoped variants
void SIM_hap_delete_callback_obj(const char *hap, conf_object_t *obj, obj_hap_func_t func, lang_void *user_data);
void SIM_hap_delete_callback_obj_id(const char *hap, conf_object_t *obj, hap_handle_t handle);
```

### 1.4 Firing a HAP from C

```c
// Fire HAP from C code — variadic, HAP-type-specific arguments follow
void SIM_c_hap_occurred(
    hap_type_t   hap,    // value returned from SIM_hap_add_type() or SIM_hap_get_number()
    conf_object_t *obj,  // the object associated with this occurrence
    integer_t     index, // HAP index (0 if not indexed)
    ...                  // additional HAP-specific parameters (must match params string)
);

// Check if any callbacks are registered before paying the overhead cost
int SIM_hap_is_active(hap_type_t hap);  // returns 1 if callbacks exist, 0 otherwise
```

Usage pattern (from public SIMICS docs):
```c
static hap_type_t hap_handle;

static void some_func(conf_object_t *obj, int v1, int v2) {
    if (some_condition) {
        if (SIM_hap_is_active(hap_handle))
            SIM_c_hap_occurred(hap_handle, obj, 0, v1, v2);
    }
}
```

### 1.5 Defining Custom HAP Types

```c
hap_type_t SIM_hap_add_type(
    const char *hap,        // Name string, e.g., "My_Custom_Event"
    const char *params,     // Type string: "i" = integer, "s" = string, "o" = object, "II" = two ints
    const char *param_desc, // Space-separated parameter name list (matches params order)
    const char *index,      // Index parameter description, or NULL if not indexed
    const char *desc,       // Human-readable documentation string
    int         old_hap_obj // Compatibility flag; pass 0
);
```

The `params` string encoding (characters only; first two args `lang_void*` and `conf_object_t*` are implicit and not listed):

| Char | C Type |
|------|--------|
| `i` | `integer_t` (signed 64-bit) |
| `I` | `integer_t` |
| `s` | `char *` |
| `o` | `conf_object_t *` |
| `f` | `double` |

Python example:
```python
def some_func(obj, v1, v2):
    if some_condition:
        if SIM_hap_is_active(hap_handle):
            SIM_hap_occurred(hap_handle, obj, 0, [v1, v2])
```

### 1.6 Standard HAP Callback Signatures

All callbacks have the form `void callback(lang_void *user_data, conf_object_t *obj, ...)` in C. The first two parameters are always the same; the remainder are HAP-specific.

#### `Core_Exception`
Fired when a CPU takes an exception/interrupt.
```c
void cb(lang_void *user_data, conf_object_t *cpu, integer_t exception_number);
```
- `cpu`: the processor taking the exception
- `exception_number`: architecture-specific exception/interrupt number
- Index variant available: `SIM_hap_add_callback_index("Core_Exception", cb, data, exception_number)` fires only for one exception type

Python: `def cb(user_data, cpu, exception_number): ...`

#### `Core_Control_Register_Write`
Fired when a control/special register is written.
```c
void cb(lang_void *user_data, conf_object_t *cpu, integer_t reg, integer_t val);
```
- `reg`: register number (architecture-specific index)
- `val`: new value written to the register
- Indexed by register number; use `SIM_hap_add_callback_index` to filter

Python example (from SIMICS docs):
```python
def ctrl_write_pil(user_arg, cpu, reg, val):
    print("[%s] Write to %%pil: 0x%x" % (cpu.name, val))
pil_reg_no = SIM_get_register_number(conf.cpu0, "pil")
SIM_hap_add_callback_index("Core_Control_Register_Write", ctrl_write_pil, None, pil_reg_no)
```

#### `Core_External_Interrupt`
Fired when a processor receives an external interrupt (SPARC-centric but general concept).
```c
void cb(lang_void *user_data, conf_object_t *cpu, integer_t source_mid);
```
- `cpu`: the receiving processor
- `source_mid`: Module ID of the sending device or CPU
- Fires even if interrupts are masked; does not fire if interrupt logic is busy

#### `Core_Breakpoint`
Fired when a breakpoint is hit. Indexed by breakpoint ID.
```c
void cb(lang_void *user_data, conf_object_t *trigger_obj,
        integer_t breakpoint_id, integer_t access_type, generic_address_t address);
```
- `trigger_obj`: typically the memory space object
- `breakpoint_id`: the ID returned when the breakpoint was created
- Hap handlers execute **before** the triggering access is performed (for memory bps)
- Use `SIM_hap_add_callback_index("Core_Breakpoint", cb, data, bp_id)` to filter

#### `Core_Magic_Instruction`
Fired when the CPU executes the architecture-specific "magic instruction" (a special NOP encoding).
```c
void cb(lang_void *user_data, conf_object_t *cpu, integer_t magic_inst_num);
```
- `magic_inst_num`: the immediate value encoded in the magic instruction (MAGIC(n) → n)
- Index variant filters to a specific magic number
- Primary mechanism for simulated software to signal the simulator (e.g., fuzzing harness start/stop)

Python example:
```python
def magic_hap_callback(user_arg, cpu, magic_inst_num):
    if magic_inst_num == 42:
        print("Magic 42 hit!")
SIM_hap_add_callback("Core_Magic_Instruction", magic_hap_callback, None)
```

#### `Core_Simulation_Stopped`
Fired when the simulation pauses for any reason (breakpoint, user Ctrl-C, error, `SIM_break_simulation` call).
```c
void cb(lang_void *user_data, conf_object_t *obj, integer_t exception, char *errstr);
```
- `obj`: typically the `sim` global object
- `exception`: integer error code (0 = normal stop)
- `errstr`: human-readable reason string

C init example (from SIMICS docs):
```c
static hap_handle_t h1, h2;
void init_local(void) {
    h1 = SIM_hap_add_callback("Core_Continuation",
                               (obj_hap_func_t)started, NULL);
    h2 = SIM_hap_add_callback("Core_Simulation_Stopped",
                               (obj_hap_func_t)stopped, NULL);
}
```

### 1.7 HAP Priorities and Ordering

SIMICS does not expose a numbered priority system for HAP callbacks. Callbacks fire in registration order (FIFO) within a given HAP name. There is no pre-emption or priority weighting. If ordering matters, the module that needs to run first must register first.

The `_obj` and `_index` variants do not create separate priority lanes — they simply add a filter that prevents the callback from firing unless the object or index matches.

---

## 2. Timing Model and Temporal Decoupling

### 2.1 Time Representation

SIMICS models time at two resolutions:

| Unit | Type | API |
|------|------|-----|
| Cycles | `cycles_t` (= `int64_t`) | `SIM_cycle_count(obj)` |
| Seconds (virtual) | `double` | `SIM_time(obj)` |
| Picoseconds | `uint64_t` (ps) | Internal; used in some newer APIs |

Cycles and time are related by the clock's frequency. If frequency can change dynamically, "cycles from now" != "time from now" in the naive arithmetic sense. `SIM_event_post_cycle` therefore posts at a time equivalent to N cycles at the *current* frequency, not at cycle number C+N.

```c
cycles_t SIM_cycle_count(conf_object_t *obj);  // cycles elapsed on this clock/CPU
double   SIM_time(conf_object_t *obj);          // virtual seconds elapsed
```

Both take a `conf_object_t *` — typically a CPU or clock object. Each CPU has its own time domain in a multi-processor setup.

### 2.2 Controlling Simulation Flow

```c
// Resume simulation; steps=0 means run until stopped by other means
void SIM_continue(cycles_t steps);

// Stop the simulation from within a callback/event handler
void SIM_break_simulation(const char *msg);  // msg may be NULL
```

`SIM_break_simulation` schedules a stop — the currently-executing instruction completes, then simulation pauses and returns to prompt. It is safe to call from HAP callbacks, event callbacks, or device interface methods.

### 2.3 Temporal Decoupling

**Definition:** Each vCPU (virtual CPU) runs for a fixed time window called the *quantum* before handing control to the next vCPU. During that quantum, the CPU executes independently without synchronizing with other CPUs or devices. Synchronization happens only at quantum boundaries.

**Why it's fast:** Without temporal decoupling, every inter-object interaction (memory access, interrupt, etc.) requires the entire simulation to be at the same instant in time — effectively serializing all execution. With temporal decoupling:
- Binary translation (JIT) can run for long stretches without interruption
- Hypersimulation (skipping idle cycles) can fast-forward entire quanta at once
- Cache effects improve for long JIT code runs

Speedup: 10–100x real-time is achievable; hypersimulating idle processors has produced >100x with months of virtual time advancing overnight.

**Quantum:** Set via the `time_quantum` attribute on the cell object (or `cpu-switch-time` command). Optimal values are typically 500k–1M instructions. A larger quantum = more speedup but more temporal inaccuracy.

**Round-robin scheduling:** vCPUs within a cell execute one quantum each in round-robin order. After all vCPUs finish their current quantum, the cycle repeats.

**Variant 2 (device interaction within a quantum):** Devices execute "within the time slice of a processor" when the processor accesses them. A device can post events on a per-processor event queue, allowing multiple interrupts to appear at different points within one quantum rather than all at quantum boundaries. This is the SIMICS default and makes long quanta compatible with standard OS software.

**Device synchronization:** Devices are event-driven and do not have their own "run" loop. They respond to:
1. Direct interface calls from the CPU (memory-mapped I/O reads/writes)
2. Their own posted timer events (which fire at the scheduled time, potentially during a CPU's quantum)

Devices are inherently temporally decoupled — they don't need to advance time; they only respond to events.

### 2.4 The `execute` Interface

The `execute` interface is what the SIMICS scheduler calls on each scheduled object (typically CPUs) to run it forward. It is implemented by CPU models, not by devices.

```c
typedef struct execute_interface {
    // Run this object forward; the scheduler calls this once per quantum
    // The object runs until:
    //   (a) its quantum expires (scheduler will call stop())
    //   (b) SIM_break_simulation() is called
    //   (c) a breakpoint is hit
    void (*run)(conf_object_t *obj);

    // Called by scheduler to request the object stop at next stable point
    void (*stop)(conf_object_t *obj);
} execute_interface_t;
```

All objects in a cell implementing `execute` are kept within a virtual time window defined by `time_quantum`. The scheduler drives them in round-robin.

Hypersimulation is implemented inside `run()`: when the CPU detects it is idle (e.g., spinning in a WFI/HLT loop), it can advance its cycle counter to the next posted event or end-of-quantum without executing individual instructions.

---

## 3. Event Posting System

Events schedule a callback to fire at a future simulated time. They are the primary mechanism for device models to implement timed behavior (TX delays, timeouts, periodic interrupts).

### 3.1 Event Class Registration

Before posting events, register an event class once at class init time:

```c
event_class_t *SIM_register_event(
    const char          *name,       // Human-readable; shown in `peq` (event queue inspector)
    conf_class_t        *cls,        // Simics class this event is associated with
    event_class_flag_t   flags,      // See below
    void (*callback)(conf_object_t *obj, void *user_data),  // Fires when event expires
    attr_value_t (*get_value)(conf_object_t *obj, void *user_data),  // For checkpointing; NULL ok
    void *(*set_value)(conf_object_t *obj, attr_value_t val),        // For checkpointing; NULL ok
    char *(*describe)(conf_object_t *obj, void *user_data)           // For peq display; NULL ok
);
```

`event_class_flag_t` values:
- `Sim_EC_No_Exception` (0) — normal event, no special requirements
- `Sim_EC_Machine_Sync` — must fire synchronized across all processors; cannot use step-based posting

### 3.2 Posting Events

```c
// Post event to fire N cycles from now on the given clock
void SIM_event_post_cycle(
    conf_object_t  *clock,    // Clock or processor driving this event's timeline
    event_class_t  *evclass,  // Event class from SIM_register_event()
    conf_object_t  *obj,      // Owner object (must match cls if cls was specified at registration)
    cycles_t        cycles,   // Cycles from now (relative, based on current frequency)
    void           *user_data // Passed to callback unchanged
);

// Post event to fire N seconds from now
void SIM_event_post_time(
    conf_object_t  *clock,
    event_class_t  *evclass,
    conf_object_t  *obj,
    double          seconds,  // Seconds from now (relative)
    void           *user_data
);

// Post event to fire N steps from now (cannot use with Sim_EC_Machine_Sync)
void SIM_event_post_step(
    conf_object_t  *clock,
    event_class_t  *evclass,
    conf_object_t  *obj,
    pc_step_t       steps,
    void           *user_data
);
```

**Key distinction:** `SIM_event_post_cycle` converts the cycle count to an absolute time using the *current* frequency. If the CPU's frequency changes after posting, the event still fires at the originally computed time — it does not re-scale. (Simics processor models do re-scale their queued events on frequency change, but user-posted events may not.)

### 3.3 Cancellation

```c
// Cancel time-based events (cycle or second-based)
void SIM_event_cancel_time(
    conf_object_t  *clock,
    event_class_t  *evclass,
    conf_object_t  *obj,
    int  (*pred)(lang_void *data, lang_void *match_data),  // NULL = cancel all matching
    lang_void      *match_data
);

// Cancel step-based events
void SIM_event_cancel_step(
    conf_object_t  *clock,
    event_class_t  *evclass,
    conf_object_t  *obj,
    int  (*pred)(lang_void *data, lang_void *match_data),
    lang_void      *match_data
);
```

If `pred` is NULL, all pending events matching `(clock, evclass, obj)` are cancelled. If `pred` is provided, only events where `pred(user_data, match_data)` returns non-zero are cancelled.

**Rule:** Before destroying a device object, all its pending events must be cancelled.

### 3.4 Inspecting the Event Queue

```c
// Returns the absolute cycle count when the next matching event fires; -1 if none
cycles_t SIM_event_find_next_cycle(
    conf_object_t  *clock,
    event_class_t  *evclass,
    conf_object_t  *obj,
    int  (*pred)(lang_void *data, lang_void *match_data),
    lang_void      *match_data
);

// Returns time in seconds for next matching event; -1.0 if none
double SIM_event_find_next_time(
    conf_object_t  *clock,
    event_class_t  *evclass,
    conf_object_t  *obj,
    int  (*pred)(lang_void *data, lang_void *match_data),
    lang_void      *match_data
);
```

### 3.5 Device Model Pattern — UART TX Delay

A UART transmit delay is a canonical event usage example:

```c
// In init_local():
static event_class_t *tx_done_event;
tx_done_event = SIM_register_event(
    "UART TX Complete",
    uart_class,
    Sim_EC_No_Exception,
    tx_done_callback,   // fires when byte transmission completes
    NULL, NULL,         // no checkpointing needed for simple timers
    NULL
);

// When software writes a byte to the TX register:
exception_type_t uart_write(conf_object_t *obj, generic_transaction_t *mop, map_info_t info) {
    uart_device_t *uart = (uart_device_t *)obj;
    uint8_t data = SIM_get_mem_op_value_le(mop);
    uart->tx_buffer = data;
    // Schedule TX complete after N cycles (baud rate derived)
    cycles_t delay = uart->cpu_freq / uart->baud_rate;
    SIM_event_post_cycle(uart->clock, tx_done_event, obj, delay, NULL);
    return Sim_PE_No_Exception;
}

// The callback:
static void tx_done_callback(conf_object_t *obj, void *user_data) {
    uart_device_t *uart = (uart_device_t *)obj;
    // Mark TX complete, raise interrupt
    uart->status |= TX_EMPTY;
    if (uart->irq_enable & TX_IRQ)
        uart->irq_dev.signal.signal_raise(uart->irq_port);
}
```

### 3.6 C++ API

The C++ Device API wraps events into typed classes:
- `simics::TimeEvent` — second-based
- `simics::CycleEvent` — cycle-based
- `simics::StepEvent` — step-based

The callback is a virtual method override rather than a function pointer.

---

## 4. Processor (CPU) Model Integration

### 4.1 Core Interfaces a CPU Must Implement

SIMICS uses interface dispatch for everything. A full CPU model implements many interfaces. The minimum set for integration:

| Interface | Purpose |
|-----------|---------|
| `execute` | Scheduler calls `run()` to advance the CPU; CPU calls `stop()` when halting |
| `cycle` | Expose cycle counter, frequency, and event posting on this CPU's timeline |
| `processor_info_v2` | Generic processor operations: disassemble, logical-to-physical address, etc. |
| `int_register` | Read/write integer registers by index |
| `exception` | Trigger exceptions programmatically |

Optional but commonly expected:
- `step` — step-granular control
- `context` — software context tracking
- `disassemble` — instruction disassembly

### 4.2 `execute` Interface

```c
typedef struct execute_interface {
    void (*run)(conf_object_t *obj);   // Run until quantum exhausted or stopped
    void (*stop)(conf_object_t *obj);  // Request stop at next stable point
} execute_interface_t;
```

**Dispatcher mechanism:** The SIMICS scheduler maintains a list of all objects in a cell implementing the `execute` interface. At each scheduling cycle, it calls `run()` on each object in round-robin order. The `run()` implementation is the CPU's instruction execution loop — it runs until the scheduler calls `stop()` or until `SIM_break_simulation()` has been triggered.

The scheduler enforces time window discipline: when a CPU has advanced its virtual time beyond the allowed window (`time_quantum`), SIMICS calls `stop()`. The CPU must then complete its current instruction and return from `run()`.

### 4.3 `processor_info_v2` Interface

```c
typedef struct processor_info_v2_interface {
    // Disassemble instruction at logical address; returns string + byte count
    tuple_int_string_t (*disassemble)(conf_object_t *cpu,
                                       generic_address_t addr,
                                       attr_value_t instruction_data,
                                       int sub_operation);

    // Translate logical address to physical
    physical_address_t (*logical_to_physical)(conf_object_t *cpu,
                                               logical_address_t addr,
                                               access_t access_type);

    // Get the program counter
    logical_address_t  (*get_program_counter)(conf_object_t *cpu);

    // Architecture string ("x86-64", "arm", "riscv", etc.)
    const char        *(*architecture)(conf_object_t *cpu);

    // Endianness
    int                (*get_endian)(conf_object_t *cpu);  // returns Sim_Endian_Big/Little

    // Physical memory space this CPU is attached to
    conf_object_t     *(*get_physical_memory)(conf_object_t *cpu);
} processor_info_v2_interface_t;
```

This interface is what standard SIMICS commands (`stepi`, `disassemble`, `print-stack`, etc.) use to operate generically across all CPU architectures.

### 4.4 `cycle` Interface

```c
typedef struct cycle_interface {
    cycles_t (*get_cycle_count)(conf_object_t *obj);   // Current cycle count
    double   (*get_time)(conf_object_t *obj);           // Current time in seconds
    uint32_t (*get_frequency)(conf_object_t *obj);      // Frequency in Hz

    // Post an event on this CPU's timeline (wrapper around SIM_event_post_cycle)
    void     (*post_cycle)(conf_object_t *obj, event_class_t *evclass,
                           conf_object_t *poster, cycles_t cycles, void *data);
    void     (*post_time)(conf_object_t *obj, event_class_t *evclass,
                          conf_object_t *poster, double seconds, void *data);

    // Cancel and find events
    void     (*cancel)(conf_object_t *obj, event_class_t *evclass,
                       conf_object_t *poster, int (*pred)(void*, void*), void *match);
    cycles_t (*find_next_cycle)(conf_object_t *obj, event_class_t *evclass,
                                conf_object_t *poster, int (*pred)(void*, void*), void *match);
} cycle_interface_t;
```

The `cycle` interface on a CPU object mirrors the global `SIM_event_post_cycle` API but is accessed through the object's interface pointer — allowing other objects to post events on a specific CPU's timeline without knowing the global API.

### 4.5 Memory Operation API

Memory operations in SIMICS flow through `generic_transaction_t`, a struct carrying the address, size, type (read/write/execute), initiating CPU, and data.

```c
// Get the value from a memory operation (endian-converted to CPU native)
uint64_t SIM_get_mem_op_value_cpu(generic_transaction_t *mop);
// Set the return value for a read operation
void     SIM_set_mem_op_value_cpu(generic_transaction_t *mop, uint64_t val);

// Endian-explicit variants
uint64_t SIM_get_mem_op_value_le(generic_transaction_t *mop);  // little-endian
uint64_t SIM_get_mem_op_value_be(generic_transaction_t *mop);  // big-endian

// Query the transaction
int               SIM_mem_op_is_read(generic_transaction_t *mop);
int               SIM_mem_op_is_write(generic_transaction_t *mop);
physical_address_t SIM_get_mem_op_physical_address(generic_transaction_t *mop);
size_t            SIM_get_mem_op_size(generic_transaction_t *mop);
conf_object_t    *SIM_get_mem_op_initiator(generic_transaction_t *mop);
```

### 4.6 Multi-CPU Synchronization

Each CPU has its own time domain. The SIMICS scheduler enforces that all CPUs within a cell stay within a time window of `time_quantum` of each other. Specifically:

- A CPU that has run its full quantum must wait for all other CPUs to finish their quanta before it runs again.
- This is not lockstep — CPUs can be at different times within the allowed window.
- For `Sim_EC_Machine_Sync` events, SIMICS ensures they fire at a point where all CPUs are at the same virtual time (quantum boundary).

The `SIM_continue()` function enters the scheduler loop. The scheduler drives all CPUs in round-robin until `SIM_break_simulation()` is called or all CPUs have no more work.

### 4.7 Simulator Translation Cache (STC)

SIMICS maintains an STC (Simulator Translation Cache) that bypasses the full memory space dispatch for "harmless" addresses (addresses where access causes no side effects). When an address is cached in the STC:
- Memory reads/writes go directly to host memory
- Device memory is never in the STC (device accesses always go through `io_memory.operation()`)

`run_simple_uncached` is a term from SIMICS internals referring to the path where a memory access falls outside the STC and must be dispatched through the full memory space hierarchy, potentially reaching a device's `io_memory` interface.

---

## 5. DML Device Modeling

DML (Device Modeling Language) is SIMICS's domain-specific language for functional device models. The DML compiler (`dmlc`) generates C code with SIMICS API calls. DML 1.4 is the current version.

### 5.1 File Structure

```dml
dml 1.4;
device my_uart;

import "simics/devs/signal.dml";   // For signal interface
// ... declarations follow
```

### 5.2 `bank` / `register` / `field` Hierarchy

Banks are memory-mapped register groups. Each bank implements the `io_memory` (or newer `transaction`) interface and handles reads/writes to its address range.

```dml
bank regs {
    param register_size = 4;  // Default register width in bytes

    // Register at offset 0x00, 4 bytes
    register STATUS @ 0x00 {
        // Fields subdivide bits
        field TX_EMPTY  @ [0]   is (read, write);
        field RX_FULL   @ [1]   is (read, write);
        field ERROR     @ [2]   is (read, write) {
            // Override the write method
            method write(uint64 val) {
                // Clear-on-write-1 semantic
                this.val &= ~val;
            }
        }
    }

    // Register at offset 0x04
    register CONTROL @ 0x04 {
        field TX_EN  @ [0]  is (read, write);
        field RX_EN  @ [1]  is (read, write);
        field IRQ_EN @ [2]  is (read, write);
    }

    // Array of registers: 4 entries, 8 bytes each, starting at 0x10
    register DATA[i < 4] size 8 @ 0x10 + i * 8 is (read, write) {}
}
```

Templates applied with `is`:
- `read` — readable field
- `write` — writable field
- `read_unimpl` — reads return 0, logs "unimplemented"
- `write_unimpl` — writes are ignored, logs "unimplemented"
- `reserved` — reads 0, writes ignored (no log)
- `read_only` — write attempts log a warning

### 5.3 `implement` Keyword — Interface Implementations

`implement` declares that the device (or a port) provides an interface. DML generates the C registration and stub dispatch.

```dml
// Device receives signal inputs on a named port
port RESET_IN {
    implement signal {
        method signal_raise() {
            log info: "Reset asserted";
            call $reset();
        }
        method signal_lower() {
            log info: "Reset deasserted";
        }
    }
}
```

When compiled for SIMICS, the DML compiler generates:
```c
// (generated)
static signal_interface_t reset_in_signal_iface = {
    .signal_raise = reset_in_signal_raise,
    .signal_lower = reset_in_signal_lower,
};
SIM_register_port_interface(cls, "signal", &reset_in_signal_iface, "RESET_IN", NULL);
```

### 5.4 `connect` Keyword — Typed Object References

`connect` declares a reference to another SIMICS object, exposing a configuration attribute and declaring which interfaces the connected object must implement.

```dml
// Outgoing interrupt line — connects to interrupt controller input port
connect irq_dev {
    param documentation = "Interrupt controller input";
    param configuration = "required";  // Must be set before simulation starts
    interface signal;
}

// Usage within device methods:
method raise_irq() {
    irq_dev.signal.signal_raise();
}

method lower_irq() {
    irq_dev.signal.signal_lower();
}
```

SIMICS setup scripts wire this connection:
```python
# Python setup script
my_uart.irq_dev = (interrupt_controller, "IRQ[3]")
# or for simple (non-port) objects:
my_uart.irq_dev = interrupt_controller
```

### 5.5 `event` Declaration — Device-Internal Timers

```dml
// Declare an event
event tx_complete_event {
    // The method called when the event fires
    method event(void *data) {
        log info: "TX complete";
        regs.STATUS.TX_EMPTY.val = 1;
        if (regs.CONTROL.IRQ_EN.val)
            irq_dev.signal.signal_raise();
    }
}

// Post the event from within a register write handler
bank regs {
    register TX_DATA @ 0x08 {
        method write(uint64 val) {
            local double delay = 1.0 / cast(baud_rate.val, double);
            after (delay s): tx_complete_event.event(NULL);
        }
    }
}
```

### 5.6 `attribute` — Exposing State

Attributes are SIMICS-visible named values on the object — used for configuration, inspection, and checkpointing. Standard templates handle the common cases:

```dml
// Simple integer attribute
attribute baud_rate is (uint64_attr) {
    param documentation = "UART baud rate in bps";
    param init_val = 115200;
}

// Custom attribute with get/set
attribute my_state is (pseudo_attr) {  // pseudo = not saved in checkpoint
    param type = "i";
    method get() -> (attr_value_t) {
        return SIM_make_attr_int64(some_computed_value());
    }
    method set(attr_value_t val) {
        // ...
    }
}
```

`saved` variables are automatically checkpointed without being user-visible as attributes:
```dml
saved uint32 internal_state;  // Saved in checkpoint; not an attribute
```

### 5.7 `method` — Behavior

DML methods are synchronous subroutines. They can have parameters and return values:

```dml
method reset() {
    regs.STATUS.val   = 0;
    regs.CONTROL.val  = 0;
    log info: "UART reset";
}

method read_status() -> (uint64) {
    return regs.STATUS.val;
}
```

Overriding `read`/`write` on a register or field:
```dml
register STATUS @ 0x00 {
    method read() -> (uint64) {
        // Clear interrupt-pending bit on read (read-to-clear)
        local uint64 v = this.val;
        this.val &= ~INTR_PENDING;
        return v;
    }
}
```

### 5.8 `after` Statement — Scheduling Future Actions

The `after` statement is syntactic sugar for declaring and immediately posting an event:

```dml
// Schedule a call to my_method() 100 microseconds from now
after (100e-6 s): my_method();

// Schedule after N cycles
after (baud_cycles): tx_complete_event.event(NULL);
```

Equivalent to explicitly registering an event and calling `SIM_event_post_time`.

### 5.9 DML 1.4 vs DML 1.2 Key Differences

| Feature | DML 1.2 | DML 1.4 |
|---------|---------|---------|
| Dollar prefix for object refs | `$irq_dev.signal.signal_raise()` | `irq_dev.signal.signal_raise()` |
| `saved` keyword | Not available; use `attribute` | `saved` for checkpoint-only state |
| `is` template application | Less uniform | Uniform `is (template)` syntax |
| `after` with method params | Limited | Supported (bug fixed in recent releases) |
| Error handling | C-style | Better typed |

---

## 6. Port Objects and the Signal Interface

### 6.1 What a Port Object Is

A port object is a sub-object of a SIMICS device object, introduced in SIMICS 6. It is a first-class SIMICS configuration object (`conf_object_t`) that:
- Lives as a child of the parent device (named `parent_name.port_name`)
- Can implement one or more interfaces independently from the parent
- Enables a device to expose the *same* interface *multiple times* (one per port)

Example: A device with 8 GPIO pins might expose 8 `signal` interface instances, one per port named `gpio[0]` through `gpio[7]`.

Without port objects (pre-SIMICS 6), a device could only implement each interface once on the top-level object.

### 6.2 `SIM_register_port_interface`

```c
void SIM_register_port_interface(
    conf_class_t *cls,        // The device class
    const char   *iface_name, // Interface name string, e.g., "signal"
    const void   *iface,      // Pointer to the filled interface struct
    const char   *port_name,  // Port name string, e.g., "IRQ[0]" or "RESET_IN"
    const char   *desc        // Documentation string (may be NULL)
);
```

DML generates these calls automatically when you write:
```dml
port IRQ_IN {
    implement signal { ... }
}
```

The generated C is approximately:
```c
static signal_interface_t irq_in_iface = {
    .signal_raise = my_device_irq_in_signal_raise,
    .signal_lower = my_device_irq_in_signal_lower,
};
SIM_register_port_interface(my_class, "signal", &irq_in_iface, "IRQ_IN", "Interrupt input");
```

To retrieve a port interface from another object:
```c
void *SIM_get_port_interface(conf_object_t *obj, const char *iface_name, const char *port_name);
```

### 6.3 The `signal` Interface Struct

```c
// From simics/devs/signal.h
typedef struct signal_interface {
    void (*signal_raise)(conf_object_t *obj);  // Assert the signal (edge or level)
    void (*signal_lower)(conf_object_t *obj);  // Deassert the signal
} signal_interface_t;

#define SIGNAL_INTERFACE "signal"
```

`obj` in both functions is the port object (or device object if not using port objects) that implements the interface — i.e., the *receiver* of the signal, not the sender.

The bidirectional I2C case shows two `signal` interface instances (one per direction):
```
i2c_bus.port.SCL  -- implements signal (master drives SCL line)
i2c_bus.port.SDA  -- implements signal (master drives SDA line)
```

### 6.4 Complete Interrupt Output Pattern

This is the canonical pattern for a device that raises an interrupt:

**Device model (DML 1.4):**
```dml
dml 1.4;
device my_timer;

import "simics/devs/signal.dml";

// Output interrupt connection — caller wires this to the interrupt controller
connect irq {
    param documentation = "Interrupt output — connect to interrupt controller";
    param configuration = "required";
    interface signal;
}

// Track IRQ state (saved for checkpointing)
saved bool irq_raised = false;

method assert_irq() {
    if (!irq_raised) {
        irq_raised = true;
        irq.signal.signal_raise();
    }
}

method deassert_irq() {
    if (irq_raised) {
        irq_raised = false;
        irq.signal.signal_lower();
    }
}

bank regs {
    register STATUS @ 0x00 is (read, write) {
        method write(uint64 val) {
            this.val = val;
            // Clear interrupt if all bits cleared
            if (val == 0)
                call $deassert_irq();
        }
    }
}

event timer_tick {
    method event(void *data) {
        regs.STATUS.val |= 0x1;  // Set interrupt pending bit
        call $assert_irq();
        // Reschedule
        after (1e-3 s): timer_tick.event(NULL);
    }
}
```

**Interrupt controller (DML 1.4) — the receiver:**
```dml
dml 1.4;
device my_irq_controller;

// 8 interrupt input ports
port IRQ[i < 8] {
    implement signal {
        method signal_raise() {
            log info: "IRQ %d raised", i;
            // Set pending bit in internal register
            pending_irqs |= (1 << i);
            call $update_cpu_interrupt();
        }
        method signal_lower() {
            log info: "IRQ %d lowered", i;
            pending_irqs &= ~(1 << i);
            call $update_cpu_interrupt();
        }
    }
}

saved uint8 pending_irqs = 0;
```

**SIMICS setup (Python):**
```python
# Wire timer IRQ output to interrupt controller input port 3
my_timer.irq = (my_irq_controller, "IRQ[3]")
```

### 6.5 Interrupt Controller to CPU Connection

The interrupt controller signals the CPU through the CPU's own interrupt interface (architecture-specific). The `signal` pattern terminates at the interrupt controller; the controller then drives the CPU's interrupt pin through a separate interface (e.g., `simple_interrupt` or the CPU's `exception` interface).

---

## 7. SIMICS Object Model Foundation

### 7.1 Core Types

| Type | Description |
|------|-------------|
| `conf_object_t` | Base opaque struct for every SIMICS object. All device instances, CPUs, memory spaces, etc. are `conf_object_t *`. Device structs embed it as the first field. |
| `conf_class_t` | Opaque handle for a registered SIMICS class. Created by `SIM_register_class()`. |
| `integer_t` | `int64_t` — SIMICS's signed 64-bit integer type used throughout the HAP and register APIs |
| `cycles_t` | `int64_t` — cycle count |
| `pc_step_t` | `int64_t` — instruction step count |
| `simtime_t` | `double` — virtual seconds |
| `generic_address_t` | 64-bit address (physical or logical depending on context) |

### 7.2 Class Registration

```c
typedef struct class_info {
    conf_object_t *(*alloc_object)(void *data);          // Allocate device struct
    void           (*delete_instance)(conf_object_t *);  // Free device struct
    void          *(*init_object)(conf_object_t *obj,    // Optional: post-alloc init
                                   conf_object_t *parent,
                                   attr_value_t *args);
    const char    *description;     // Long documentation
    const char    *short_desc;      // One-line description
    class_kind_t   kind;            // Sim_Class_Kind_Vanilla (normal), _Session, _Pseudo, etc.
} class_info_t;

conf_class_t *SIM_register_class(const char *name, const class_info_t *class_info);
```

`alloc_object` embeds the `conf_object_t` as the first field:
```c
typedef struct {
    conf_object_t  obj;      // MUST be first — SIMICS requires this layout
    uint32_t       status;
    uint32_t       control;
    // ... device state
} my_uart_t;

static conf_object_t *alloc_object(void *data) {
    my_uart_t *dev = MM_ZALLOC(1, my_uart_t);
    return &dev->obj;
}
```

### 7.3 Interface Registration

```c
// Register a named interface on a class
void SIM_register_interface(conf_class_t *cls, const char *name, const void *iface);

// Retrieve an interface from an object at runtime
void *SIM_get_interface(const conf_object_t *obj, const char *name);

// Port interface variants (SIMICS 6+)
void  SIM_register_port_interface(conf_class_t *cls, const char *iface_name,
                                   const void *iface, const char *port_name, const char *desc);
void *SIM_get_port_interface(const conf_object_t *obj, const char *iface_name,
                              const char *port_name);
```

### 7.4 `io_memory` Interface (Memory-Mapped I/O)

```c
typedef struct io_memory_interface {
    // Called when device is mapped into a memory space
    int (*map)(conf_object_t *obj, addr_space_t memory_or_io, map_info_t map_info);

    // Called for every read/write that reaches this device
    exception_type_t (*operation)(conf_object_t *obj,
                                   generic_transaction_t *mem_op,
                                   map_info_t map_info);
} io_memory_interface_t;
```

`map_info_t` carries the base address, size, function number (for multi-bank devices), and byte-swap settings. The `operation` method handles both reads and writes — check `SIM_mem_op_is_read(mem_op)` to distinguish.

Return value `exception_type_t`:
- `Sim_PE_No_Exception` — success
- `Sim_PE_IO_Not_Taken` — address not handled (causes bus error)
- `Sim_PE_IO_Error` — device error

---

## 8. Python API Layer

The entire `SIM_*` C API is accessible from Python in a SIMICS session. SIMICS wraps C functions into Python via its internal module.

**Subscribing to HAPs from Python:**
```python
# Global subscription
handle = SIM_hap_add_callback("Core_Exception",
                               lambda user_data, cpu, exc_no: print(f"Exception {exc_no}"),
                               None)

# Object-specific
handle = SIM_hap_add_callback_obj("Core_Control_Register_Write",
                                   conf.cpu0, 0,
                                   my_callback, None)

# Remove
SIM_hap_delete_callback_id("Core_Exception", handle)
```

**Controlling simulation:**
```python
SIM_continue(0)              # Run indefinitely
SIM_break_simulation("done") # Stop from a callback
```

**Accessing objects:**
```python
cpu = conf.cpu0               # Access object by name
cycles = SIM_cycle_count(cpu) # Query time
t = SIM_time(cpu)
```

**Python cannot implement new interfaces** — interface structs require C function pointers. Python can only *call* interfaces on existing objects.

**Module entry point:** Every C module has an `init_local()` function that SIMICS calls when the module is loaded. This is where class registration, interface registration, and HAP subscriptions happen.

---

## 9. Checkpointing Considerations

SIMICS's save/restore (checkpoint) system requires all simulation state to be serializable.

**For HAP callbacks:** The list of registered HAP callbacks is NOT saved in checkpoints. Modules must re-register their callbacks in `init_local()` each time they are loaded. This is by design — `init_local()` runs on every load (both initial load and checkpoint restore).

**For events:** Events in the queue ARE saved, but only if the event class was registered with `get_value`/`set_value` callbacks in `SIM_register_event()`. If those are NULL, events are cancelled on checkpoint save and not restored. For timer events that must survive checkpointing (e.g., a pending UART TX), implement the serialization callbacks.

**For device state:** In DML, all `saved` variables and `attribute` values (that are not `pseudo`) are automatically serialized into the checkpoint.

**IRQ state:** The SIMICS Model Development Checklist recommends tracking interrupt assertion state in a `saved` attribute:
```dml
saved bool irq_raised = false;
```
This ensures the interrupt line state is correctly restored after a checkpoint.

---

## 10. Rust Design Implications

### 10.1 HAP System → Rust Event Bus

The HAP system maps naturally to a typed event bus in Rust. Key design decisions:

- **Type erasure at registration vs. typed dispatch:** SIMICS uses `void *` and runtime casting. Rust can use trait objects (`dyn Fn`) or an enum-based event type. For a SIMICS-inspired design, a `HapBus<HapType>` with `HapType` as an enum variant carrying typed payloads is cleanest.
- **`hap_handle_t` → `HapHandle`:** A newtype wrapping a u64 or index for cancellation. Consider a `WeakRef`-style handle that auto-removes on drop.
- **Index filtering:** SIMICS's `_index_` variants suggest a filter predicate on registration.
- **No priority ordering:** Match SIMICS's FIFO behavior within a HAP name.

```rust
// Suggested Rust pattern
pub enum HapEvent {
    Exception { cpu: ObjectRef, exception_number: i64 },
    ControlRegisterWrite { cpu: ObjectRef, reg: i64, val: i64 },
    SimulationStopped { exception: i64, reason: String },
    // ...
}

pub struct HapHandle(u64);  // Returned on registration; used for cancellation

impl HapBus {
    pub fn subscribe<F>(&self, f: F) -> HapHandle
    where F: Fn(&HapEvent) + Send + 'static { ... }

    pub fn subscribe_filtered<F>(&self, filter: impl Fn(&HapEvent) -> bool, f: F) -> HapHandle
    where F: Fn(&HapEvent) + Send + 'static { ... }

    pub fn fire(&self, event: HapEvent) { ... }
    pub fn cancel(&self, handle: HapHandle) { ... }
}
```

### 10.2 Temporal Decoupling → Rust Scheduler

- Each vCPU is a `struct CpuCore` that implements an `Execute` trait with `run(&mut self, budget: Cycles)`.
- The scheduler round-robins across all registered `Execute` implementors.
- Quantum: a configurable `time_quantum: Duration` on the `Cell` struct.
- Temporal decoupling means no `Mutex` needed between vCPUs during a quantum — each runs independently. Sync points only at quantum boundaries.
- Hypersimulation: `run()` returns early if `next_event_at > current_time + quantum`; advance clock to `min(next_event_at, quantum_end)`.

```rust
pub trait Execute {
    fn run(&mut self, budget: Cycles) -> StopReason;
    fn stop(&mut self);
}

pub enum StopReason {
    QuantumExhausted,
    BreakpointHit,
    SimulationStopped,
}
```

### 10.3 Event System → Priority Queue + Trait

```rust
pub struct EventClass {
    name: String,
    flags: EventClassFlags,
    callback: Box<dyn Fn(&mut dyn SimObject, *mut c_void)>,
}

// The event queue is a BinaryHeap sorted by (fire_time, sequence_number)
pub struct EventQueue {
    queue: BinaryHeap<PendingEvent>,
}

pub struct PendingEvent {
    fire_at: VirtualTime,
    seq: u64,
    class: Arc<EventClass>,
    owner: ObjectRef,
    user_data: *mut c_void,
}
```

Rust ownership challenge: `user_data` as `*mut c_void` — consider `Box<dyn Any + Send>` instead for safe ownership.

### 10.4 Interface System → Traits

SIMICS interfaces (`io_memory_interface_t`, `signal_interface_t`, etc.) map directly to Rust traits:

```rust
pub trait IoMemory {
    fn map(&mut self, space: AddrSpace, info: MapInfo) -> i32;
    fn operation(&mut self, txn: &mut MemTransaction, info: MapInfo) -> ExceptionType;
}

pub trait Signal {
    fn signal_raise(&mut self);
    fn signal_lower(&mut self);
}
```

Port objects map to component fields on the device struct implementing different trait instances.

### 10.5 DML Hierarchy → Rust Structs + Macros

The `bank / register / field` hierarchy is a strong candidate for Rust proc-macros:

```rust
#[register_bank(base = 0x0, register_size = 4)]
struct UartRegisters {
    #[register(offset = 0x00)]
    status: StatusRegister,
    #[register(offset = 0x04)]
    control: ControlRegister,
}

#[register]
struct StatusRegister {
    #[field(bits = 0..=0, access = ReadWrite)]
    tx_empty: bool,
    #[field(bits = 1..=1, access = ReadWrite)]
    rx_full: bool,
}
```

### 10.6 Object Model → `Arc<dyn SimObject>` + Type Map

SIMICS's `conf_object_t` + interface dispatch maps to a trait object registry:

```rust
pub trait SimObject: Send + Sync {
    fn name(&self) -> &str;
    fn class_name(&self) -> &str;
    // Interface lookup by name (returns Box<dyn Any>)
    fn get_interface(&self, name: &str) -> Option<&dyn Any>;
}
```

Consider `anymap` or a `HashMap<TypeId, Box<dyn Any>>` for interface storage.

### 10.7 Checkpointing → serde

SIMICS's checkpoint system maps to `serde::Serialize + serde::Deserialize`. DML's `saved` variables become `#[serde(skip)]` for non-checkpointed state and regular fields for checkpointed state. Event queue checkpointing requires serialization of pending events — straightforward with serde if `user_data` is typed.

---

## Sources

- [Simics HAPs — SFU Programming Guide](https://www2.cs.sfu.ca/CourseCentral/886/fedorova/Tools/Simics-old/simics-3.0.23/doc/simics-programming-guide/topic32.html)
- [Simics API Functions Reference — SFU](https://www2.cs.sfu.ca/CourseCentral/886/fedorova/Tools/Simics-old/simics-3.0.23/doc/simics-reference-manual-public-all/topic8.html)
- [Simics Core_External_Interrupt — SFU Reference](https://www2.cs.sfu.ca/CourseCentral/886/fedorova/Tools/Simics-old/simics-3.0.23/doc/simics-reference-manual-public-all/topic575.html)
- [Intel TSFFS: Temporal Decoupling Notes](https://www.intel.com/content/www/us/en/developer/articles/technical/additional-notes-about-temporal-decoupling.html)
- [Jakob Engblom: Some Notes on Temporal Decoupling](https://jakob.engbloms.se/archives/3467)
- [Intel TSFFS: Processor Model Integration Guide](https://intel.github.io/tsffs/simics/processor-model-integration-guide/index.html)
- [Intel TSFFS: Programming with DML](https://intel.github.io/tsffs/simics/model-builder-user-guide/programming-with-dml.html)
- [Intel TSFFS: Example Models](https://intel.github.io/tsffs/simics/model-builder-user-guide/example-models.html)
- [Intel TSFFS: Connecting to External World](https://intel.github.io/tsffs/simics/model-builder-user-guide/external-world.html)
- [Intel TSFFS: Memory Spaces](https://intel.github.io/tsffs/simics/model-builder-user-guide/memory-space.html)
- [Intel TSFFS: C++ Device API v2](https://intel.github.io/tsffs/simics/cc-device-api/index.html)
- [Intel TSFFS: Scripting with Python](https://intel.github.io/tsffs/simics/simics-user-guide/userguide-scripting-python.html)
- [Intel TSFFS: Modeling I2C Devices](https://intel.github.io/tsffs/simics/model-builder-user-guide/modeling-i2c-devices.html)
- [Intel TSFFS: SystemC Execution](https://intel.github.io/tsffs/simics/systemc-library/execution-of-systemc-models-in-the-simics-simulator.html)
- [Intel TSFFS: Model Development Checklist](https://intel.github.io/simulator-bindings/simics/model-development-checklist/index.html)
- [intel/device-modeling-language GitHub](https://github.com/intel/device-modeling-language)
- [DML 1.4 Introduction Wiki](https://github-wiki-see.page/m/intel/device-modeling-language/wiki/1.-Introduction)
- [DML Non-Simics Simulators Wiki](https://github-wiki-see.page/m/intel/device-modeling-language/wiki/Supporting-non-Simics-simulators)
- [intel/simulator-bindings (Rust)](https://github.com/intel/simulator-bindings)
- [simics crate on crates.io](https://crates.io/crates/simics)
- [Simics Interfaces — SFU Programming Guide](https://www2.cs.sfu.ca/CourseCentral/886/fedorova/Tools/Simics-old/simics-3.0.23/doc/simics-programming-guide/topic29.html)
- [Simics Classes — SFU Programming Guide](http://www.cs.sfu.ca/~fedorova/Tech/simics-guides-3.0.26/simics-programming-guide/topic26.html)
- [Simics Cheatsheet — a-mr/simics-cheatsheet GitHub](https://github.com/a-mr/simics-cheatsheet)
- [Simics Python Scripting — Stony Brook](https://compas.cs.stonybrook.edu/~mabbasidinan/simics-user-guide-unix/topic37.html)
- [Simics Event Post Step (Rust bindings)](https://intel.github.io/simulator-bindings/crates/simics/api/base/event/fn._event_post_step.html)
