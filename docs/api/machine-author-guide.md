# Machine Author Guide

> For: engineers creating a new machine or SoC platform configuration (e.g., a minimal RISC-V virt board, a Raspberry Pi-like AArch64 board).
>
> Prerequisites: familiarity with the target ISA's memory map, interrupt controller, and boot flow. Basic Python. Read the [Device Author Guide](./device-author-guide.md) first if you are also writing devices.
>
> Cross-references: [`docs/design/helm-engine/LLD-world.md`](../design/helm-engine/LLD-world.md) · [`docs/design/helm-devices/LLD-interrupt-model.md`](../design/helm-devices/LLD-interrupt-model.md) · [`docs/design/helm-engine/HLD.md`](../design/helm-engine/HLD.md) · [`AGENT.md`](../../AGENT.md)

---

## 1. What Is a Machine?

A machine is a Python configuration script that instantiates device objects, maps them into the address space, wires interrupt signals, loads a binary, and calls `elaborate()` to materialize the simulation.

The machine script does not contain device implementation code. It is purely declarative: it answers the questions "which devices exist, where are they, and how are they connected." The Rust device implementations (and the `World` / `HelmEngine` runtime) do everything else.

helm-ng follows a Gem5-inspired two-phase config model:

```
Phase 1 — Python (no side effects):
    Instantiate device objects, set parameters, declare connections

Phase 2 — Rust (atomic, ordered):
    elaborate() → alloc → init → [attrs set] → finalize → all_finalized → ready to run
```

Nothing is allocated in Rust during phase 1. `elaborate()` triggers everything atomically.

---

## 2. Minimal Machine Example

A minimal RISC-V virt board: single CPU, 512 MiB RAM, UART, PLIC.

```python
from helm_ng import Simulation, Cpu, Memory, Uart16550, Plic, Clint, Isa, TimingModel

# ── Instantiate components (Phase 1 — no allocation yet) ──────────────────────
cpu  = Cpu(isa=Isa.RiscV, timing=TimingModel.Virtual)
mem  = Memory(size="512MiB", base_addr=0x8000_0000)
uart = Uart16550(clock_hz=1_843_200)  # No base_addr — that's a platform concern
plic = Plic(num_sources=64, num_contexts=2)
clint = Clint(num_harts=1)

# ── Build the simulation ───────────────────────────────────────────────────────
sim = Simulation(components=[cpu, mem, uart, plic, clint])

# ── Memory map (Phase 1 — declares intent, not allocation) ────────────────────
sim.map_device(uart,  base=0x1000_0000)
sim.map_device(plic,  base=0x0c00_0000)
sim.map_device(clint, base=0x0200_0000)

# ── Interrupt routing ─────────────────────────────────────────────────────────
# Device → PLIC: UART IRQ output → PLIC source 10
sim.wire_interrupt(uart.irq_out, plic.input(10))

# PLIC → CPU external interrupt (context 0 = hart 0 M-mode)
sim.wire_interrupt(plic.cpu_out(context=0), cpu.external_irq)

# CLINT timer and software interrupts → CPU
sim.wire_interrupt(clint.timer_out(hart=0),    cpu.timer_irq)
sim.wire_interrupt(clint.software_out(hart=0), cpu.software_irq)

# ── Materialize (Phase 2 — all Rust allocation happens here) ──────────────────
sim.elaborate()

# ── Load and run ──────────────────────────────────────────────────────────────
sim.load_elf("/path/to/hello.elf")
sim.run(n_instructions=1_000_000_000)
print(sim.stats())
```

---

## 3. Memory Map Design

### 3.1 Address Layout Principles

The memory map answers two questions: where does RAM live, and where are the MMIO devices?

Conventions for a RISC-V virt board (matching QEMU virt for software compatibility):

| Address Range | Purpose |
|---------------|---------|
| `0x0000_0000 – 0x0001_FFFF` | Boot ROM (32 KiB) |
| `0x0200_0000 – 0x0200_FFFF` | CLINT (timer, software IRQ) |
| `0x0c00_0000 – 0x0FFF_FFFF` | PLIC (64 MiB) |
| `0x1000_0000 – 0x1000_0007` | UART0 (8 bytes) |
| `0x1000_1000 – 0x1000_1FFF` | VirtIO disk 0 (4 KiB) |
| `0x8000_0000 – ...` | DRAM (starts at 2 GiB) |

### 3.2 Device Placement Rules

- Device regions must not overlap. `MemoryMap` will panic at `elaborate()` if they do.
- Region sizes must be powers of two for alignment compatibility. Verify with the device's `region_size()`.
- Reserve gaps between devices for future additions. A 4 KiB minimum gap prevents accidental adjacency aliasing.
- Do not place devices in the DRAM range. DRAM starts at `0x8000_0000` on most RISC-V boards.

### 3.3 Example: Declaring a Reserved Region

```python
# Explicitly reserve a region to document intent and prevent accidental overlap
sim.map_reserved(base=0x0100_0000, size=0x0100_0000, name="pci-config-space")
```

---

## 4. Interrupt Routing

### 4.1 The Routing Model

Every interrupt connection is a directed edge from an `InterruptPin` on one device to an `InterruptSink` on another:

```
Device.irq_out  →  [InterruptWire]  →  InterruptSink.on_assert(wire_id)
```

The `wire_id` is opaque to the pin but meaningful to the sink. For PLIC, `wire_id` is the source number.

### 4.2 Device to Interrupt Controller

```python
# UART IRQ output → PLIC source 10
sim.wire_interrupt(uart.irq_out, plic.input(10))

# VirtIO disk IRQ output → PLIC source 8
sim.wire_interrupt(virtio_disk.irq_out, plic.input(8))

# GPIO bank IRQ → PLIC source 12
sim.wire_interrupt(gpio.irq_out, plic.input(12))
```

`plic.input(N)` returns an `InterruptSinkBinding(sink=plic_rust_sink, wire_id=N)`. The PLIC device receives `on_assert(WireId(N))` and sets its pending bit for source N.

### 4.3 Interrupt Controller to CPU

```python
# PLIC → CPU M-mode external interrupt (context 0 = hart 0, M-mode)
sim.wire_interrupt(plic.cpu_out(context=0), cpu.external_irq)

# CLINT → CPU timer interrupt
sim.wire_interrupt(clint.timer_out(hart=0), cpu.timer_irq)

# CLINT → CPU software interrupt (IPI)
sim.wire_interrupt(clint.software_out(hart=0), cpu.software_irq)
```

### 4.4 Full RISC-V virt Platform Wiring Function

For complex boards, factor the wiring into a function:

```python
def wire_platform(sim, cpu, plic, clint, uart, disk):
    """Wire all interrupts for the virt RISC-V board."""
    # Peripheral → PLIC
    sim.wire_interrupt(uart.irq_out,  plic.input(10))
    sim.wire_interrupt(disk.irq_out,  plic.input(8))

    # PLIC → CPU (context 0 = hart 0, M-mode in single-hart SE-mode setups)
    sim.wire_interrupt(plic.cpu_out(context=0), cpu.external_irq)

    # CLINT → CPU
    sim.wire_interrupt(clint.timer_out(hart=0),    cpu.timer_irq)
    sim.wire_interrupt(clint.software_out(hart=0), cpu.software_irq)
```

---

## 5. Boot Sequence

### 5.1 Loading an ELF Binary

```python
sim.elaborate()  # must come before load_elf

# Load ELF: sets entry point, maps PT_LOAD segments into RAM
sim.load_elf("/path/to/binary.elf")

# Alternatively: load raw binary at a specific address
sim.load_binary("/path/to/firmware.bin", load_addr=0x8000_0000)
```

`load_elf()` performs a functional memory write (no timing, no cache side effects) to map each `PT_LOAD` segment into the RAM region. The ELF entry point becomes the initial PC.

### 5.2 Setting the Reset Vector Explicitly

For firmware images that are not ELF, or when the reset vector differs from the ELF entry:

```python
sim.set_pc(cpu, 0x8000_0000)       # set initial PC
sim.set_register(cpu, "x10", 0)    # a0 = hart ID (boot protocol)
sim.set_register(cpu, "x11", 0x8020_0000)  # a1 = FDT address (Device Tree)
```

### 5.3 Device Tree / Firmware Data

For SE (syscall emulation) mode with user-space ELF binaries, the boot sequence is simpler: `load_elf()` is sufficient. The kernel is not booted; syscalls are intercepted and forwarded to the host.

For FS (full system) mode (Phase 3), the boot sequence must also load the Device Tree Blob:

```python
sim.load_binary("/path/to/board.dtb", load_addr=0x8020_0000)
sim.set_register(cpu, "x11", 0x8020_0000)  # a1 = DTB address per RISC-V boot protocol
```

### 5.4 Memory Initialization

RAM is zero-initialized at `elaborate()`. If the platform requires specific initialization (e.g., a UEFI variable store region pre-populated with defaults), use `sim.mem_write()`:

```python
# Initialize 4 bytes at physical address 0x8fff_fff0 to a magic value
sim.mem_write(0x8fff_fff0, bytes([0xDE, 0xAD, 0xBE, 0xEF]))
```

---

## 6. Multi-Core Setup

### 6.1 Instantiating Multiple CPUs

Each CPU is a separate `Cpu` object. Name them explicitly for identification in stats and traces:

```python
cpu0 = Cpu(isa=Isa.RiscV, timing=TimingModel.Interval, name="cpu0")
cpu1 = Cpu(isa=Isa.RiscV, timing=TimingModel.Interval, name="cpu1")
cpu2 = Cpu(isa=Isa.RiscV, timing=TimingModel.Interval, name="cpu2")
cpu3 = Cpu(isa=Isa.RiscV, timing=TimingModel.Interval, name="cpu3")

plic  = Plic(num_sources=64, num_contexts=8)   # 4 harts × 2 contexts (M + S)
clint = Clint(num_harts=4)

sim = Simulation(components=[cpu0, cpu1, cpu2, cpu3, mem, plic, clint, uart])
```

### 6.2 Per-Hart Interrupt Wiring

Each hart needs its own PLIC and CLINT connections. Context numbering follows RISC-V PLIC spec: context 2N = hart N M-mode, context 2N+1 = hart N S-mode:

```python
for hart_id, cpu in enumerate([cpu0, cpu1, cpu2, cpu3]):
    m_ctx = hart_id * 2       # M-mode context
    s_ctx = hart_id * 2 + 1   # S-mode context

    # PLIC → CPU external interrupt (M-mode and S-mode separately)
    sim.wire_interrupt(plic.cpu_out(context=m_ctx), cpu.external_irq_m)
    sim.wire_interrupt(plic.cpu_out(context=s_ctx), cpu.external_irq_s)

    # CLINT → CPU per-hart timer and IPI
    sim.wire_interrupt(clint.timer_out(hart=hart_id),    cpu.timer_irq)
    sim.wire_interrupt(clint.software_out(hart=hart_id), cpu.software_irq)
```

### 6.3 Hart ID Initialization

The RISC-V boot protocol requires each hart to know its ID in register `a0` at boot:

```python
for hart_id, cpu in enumerate([cpu0, cpu1, cpu2, cpu3]):
    sim.set_register(cpu, "x10", hart_id)  # a0 = hart ID
```

---

## 7. Attaching Peripherals

### 7.1 VirtIO Disk

```python
from helm_ng import VirtioDisk

disk = VirtioDisk(disk_image="/path/to/rootfs.img")
sim.map_device(disk, base=0x1000_1000)     # 4 KiB MMIO region
sim.wire_interrupt(disk.irq_out, plic.input(8))
```

### 7.2 GPIO Controller

```python
gpio = Gpio(num_pins=32)
sim.map_device(gpio, base=0x1001_0000)
sim.wire_interrupt(gpio.irq_out, plic.input(12))
```

### 7.3 Clock Frequencies

Devices that model a clock-driven process (UART baud rate generator, SPI clock divider, I2C bit-bang timer) require a `clock_hz` parameter. This is a constant that does not change after `elaborate()`. Use the platform's actual reference clock frequency:

```python
uart = Uart16550(clock_hz=1_843_200)    # 1.8432 MHz for standard baud rates
spi  = SpiController(clock_hz=50_000_000, max_speed_hz=10_000_000)
```

### 7.4 Sysroot for SE Mode

In SE (syscall emulation) mode, the simulator intercepts Linux syscalls and forwards them to the host OS. If the target binary uses a different C library path than the host, specify the sysroot:

```python
sim = Simulation(
    components=[cpu, mem, uart],
    se_config=SeConfig(
        sysroot="/opt/riscv-sysroot",
        argv=["/bin/my_app", "--flag"],
        envp={"LD_LIBRARY_PATH": "/opt/riscv-sysroot/lib"},
    ),
)
```

---

## 8. Observability Hooks

### 8.1 Subscribing to HelmEventBus

The `HelmEventBus` delivers synchronous notifications for all significant simulation events. All subscriptions installed in Python callbacks are called before the event function returns.

```python
sim.elaborate()

# Subscribe to CPU exceptions
def on_exception(event):
    print(f"Exception: vector={event.vector:#x} pc={event.pc:#x} tval={event.tval:#x}")

sim.event_bus.subscribe("Exception", on_exception)

# Subscribe to CSR writes
def on_csr_write(event):
    print(f"CSR write: csr={event.csr:#x} {event.old:#x} -> {event.new:#x}")

sim.event_bus.subscribe("CsrWrite", on_csr_write)

# Subscribe to all memory writes in a range (for debugging)
def on_mem_write(event):
    if 0x8000_0000 <= event.addr < 0x8000_1000:
        print(f"DRAM write @ {event.addr:#x} = {event.val:#x} size={event.size}")

sim.event_bus.subscribe("MemWrite", on_mem_write)
```

### 8.2 Available Event Types

| Event name | Fields | Use case |
|------------|--------|----------|
| `Exception` | `cpu`, `vector`, `tval`, `pc` | Trap analysis, debug |
| `CsrWrite` | `cpu`, `csr`, `old`, `new` | Privilege mode tracking |
| `ExternalIrq` | `cpu`, `irq_num` | IRQ delivery tracing |
| `Breakpoint` | `cpu`, `addr`, `bp_id` | GDB, custom breakpoints |
| `MagicInsn` | `cpu`, `pc`, `value` | Simulation control (Gem5-style magic instructions) |
| `SimulationStop` | `reason` | Run loop completion |
| `MemWrite` | `addr`, `size`, `val`, `cycle` | Memory access tracing |
| `SyscallEnter` | `nr`, `args` | SE mode syscall tracing |
| `SyscallReturn` | `nr`, `ret` | SE mode syscall tracing |
| `DeviceSignal` | `device`, `port`, `asserted` | Device interrupt observation |

### 8.3 Unsubscribing

```python
handle = sim.event_bus.subscribe("Exception", on_exception)
# ... run for a while ...
handle.unsubscribe()  # removes the subscription; handle drop also works
```

### 8.4 Using TraceLogger

`TraceLogger` is a built-in `HelmEventBus` subscriber that writes events to a ring buffer file:

```python
sim = Simulation(
    components=[cpu, mem, uart],
    trace_log="/tmp/trace.log",
    trace_events=["Exception", "SyscallEnter", "SyscallReturn"],
)
```

---

## 9. Python Config DSL Reference

### 9.1 `Simulation`

```python
Simulation(
    components: list,           # all device/cpu/memory objects
    se_config: SeConfig = None, # syscall emulation config (SE mode only)
    trace_log: str = None,      # path to trace log file
    trace_events: list = None,  # event names to trace (default: all)
    name: str = "sim",          # simulation name for stats output
)
```

Methods:

```python
sim.map_device(device, base: int)          # map MMIO device at base address
sim.map_reserved(base: int, size: int, name: str)  # reserve an address range
sim.wire_interrupt(pin, sink_binding)      # connect interrupt pin to sink
sim.elaborate()                            # materialize (must call before load/run)
sim.load_elf(path: str)                    # load ELF into RAM, set entry PC
sim.load_binary(path: str, load_addr: int) # load raw binary at address
sim.mem_write(addr: int, data: bytes)      # functional write to RAM
sim.set_pc(cpu, addr: int)                 # set CPU's initial PC
sim.set_register(cpu, name: str, val: int) # set a CPU register by name
sim.run(n_instructions: int = None, time_ns: int = None)  # run simulation
sim.stats() -> dict                        # return performance counters
```

### 9.2 `Cpu`

```python
Cpu(
    isa: Isa,                         # Isa.RiscV | Isa.AArch64 | Isa.AArch32
    timing: TimingModel,              # TimingModel.Virtual | .Interval | .Accurate
    name: str = "cpu0",               # dot-path name in object tree
)
```

Attributes (accessible via `cpu.external_irq`, etc.):

| Attribute | Type | Description |
|-----------|------|-------------|
| `external_irq` | `InterruptPin` | M-mode external interrupt input |
| `external_irq_m` | `InterruptPin` | Explicit M-mode external IRQ (multi-mode boards) |
| `external_irq_s` | `InterruptPin` | S-mode external IRQ |
| `timer_irq` | `InterruptPin` | Timer interrupt input |
| `software_irq` | `InterruptPin` | Software (IPI) interrupt input |

### 9.3 `Memory`

```python
Memory(
    size: str | int,    # "512MiB", "1GiB", or integer bytes
    base_addr: int,     # physical base address (RAM placement is machine-level)
    name: str = "ram",
)
```

RAM is zero-initialized at `elaborate()`. The `base_addr` for `Memory` is an exception to the "device does not know its address" rule — RAM placement is a fundamental architectural constant, not an MMIO routing concern.

### 9.4 `Uart16550`

```python
Uart16550(
    clock_hz: int = 1_843_200,  # reference clock (determines baud rate divisor values)
    fifo_depth: int = 16,       # 1 (FIFO disabled), 16, 32, or 64
    name: str = "uart0",
)
```

Pins: `uart.irq_out` — interrupt output.

### 9.5 `Plic`

```python
Plic(
    num_sources: int,           # number of interrupt sources (1–1023)
    num_contexts: int = 2,      # number of hart contexts (num_harts × modes_per_hart)
    name: str = "plic",
)
```

Methods:
```python
plic.input(source_number: int) -> InterruptSinkBinding  # source binding for wire_interrupt
plic.cpu_out(context: int) -> InterruptPin              # CPU output pin for context N
```

### 9.6 `Clint`

```python
Clint(
    num_harts: int = 1,   # number of harts to generate timer/swi outputs for
    name: str = "clint",
)
```

Methods:
```python
clint.timer_out(hart: int) -> InterruptPin     # timer interrupt output for hart N
clint.software_out(hart: int) -> InterruptPin  # software interrupt output for hart N
```

### 9.7 `Isa` Enum

```python
Isa.RiscV     # RISC-V RV64GC (Phase 0 target)
Isa.AArch64   # ARM AArch64 (Phase 2)
Isa.AArch32   # ARM AArch32 (Phase 3 stub)
```

### 9.8 `TimingModel` Enum

```python
TimingModel.Virtual   # event-driven clock, >100 MIPS, correctness only
TimingModel.Interval  # Sniper-style interval simulation, <15% MAPE, >10 MIPS
TimingModel.Accurate  # cycle-accurate OoO pipeline, <10% IPC error, >200 KIPS
```
