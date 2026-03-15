# Higan / Ares Emulation Architecture Research

> Source: higan-emu/emulation-articles + ares-emulator/ares source code
> Relevance: higan's accuracy philosophy and techniques directly inform helm-ng's `Accurate` timing model

---

## What is Higan?

Higan (now continued as **ares**) is a multi-system emulator by Near (byuu) with an uncompromising focus on **cycle-accurate emulation** of game console hardware. It emulates 24+ systems (SNES, GBA, Mega Drive, PC Engine, etc.) at sub-instruction timing granularity.

It is cited as a helm-ng inspiration for its **accuracy philosophy and scheduler architecture** — specifically how it synchronizes multiple hardware components running at different clock frequencies.

---

## 1. Two Scheduler Designs

### Relative Scheduler (bsnes — for tightly coupled pairs)

Each pair of communicating components holds one `int64` counter. When component A steps N clocks: `counter -= N * freq_B`. When B steps N clocks: `counter += N * freq_A`. Positive = A ahead; negative = B ahead.

```cpp
// SNES CPU ↔ SMP synchronization
int64 cpu_smp = 0;
// CPU steps N clocks:  cpu_smp -= N * SMP_FREQ  (24,576,000)
// SMP steps N clocks:  cpu_smp += N * CPU_FREQ  (21,477,272)
```

**Key insight:** step by a multiple of the *other* component's frequency — no common denominator needed. O(1) per pair, O(N²) total — works for SNES (4 components, 3 pairs), too slow for N>6.

### Absolute Scheduler (higan/ares — for N:N multi-component)

Each thread holds a 64-bit monotonic clock. `Second = 2^63 - 1`. Each component's `Scalar = Second / Frequency`. Stepping N clocks: `clock += N * Scalar`.

```cpp
// ares thread.hpp
enum : u64 { Second = (u64)-1 >> 1 };  // 2^63 - 1

auto Thread::setFrequency(double frequency) -> void {
    _frequency = frequency + 0.5;
    _scalar    = Second / _frequency;
}

auto Thread::step(u32 clocks) -> void {
    _clock += _scalar * clocks;  // absolute clock advance
}
```

All components share the same 64-bit timestamp space. A component is "ahead" if its clock is larger. Normalize by subtracting minimum clock periodically to prevent overflow. `uniqueID` breaks ties deterministically.

**Precision:** ~10× attosecond precision. Works for any N:N component graph.

---

## 2. Cooperative Threading — The Key to Cycle Accuracy

### The Problem Without It

A cycle-accurate CPU emulator requires nested state machines. A single `LDA abs,Y` instruction with bus hold delays requires 3 levels of `switch/case` with explicit returns between each sub-cycle. Code explosion is exponential with instruction count × sub-cycle depth.

### The Solution: Coroutines as State Machines

Each hardware component runs as its own **cooperative coroutine** (libco — ~12 assembly instructions for context switch). The call stack IS the state machine:

```cpp
// Without cooperative threading: explosion of state machine code
// With cooperative threading: natural sequential code

uint8_t CPU::readMemory(uint16_t address) {
    wait(2);                      // advance clock 2 cycles
    uint8_t data = memory[address];
    wait(4);                      // advance clock 4 more cycles
    return data;
}

void CPU::wait(uint clockCycles) {
    apuCounter += clockCycles;
    while(apuCounter > 0) yield(apu);  // switch to APU if it's behind
}
```

The CPU stack frame holds all intermediate state. `yield()` freezes the CPU's stack and resumes the APU exactly where it left off.

### Ares Thread::synchronize() — The Catch-Up Loop

```cpp
// Critical: while-loop, not if-statement
template<typename... P>
auto Thread::synchronize(Thread& thread, P&&... p) -> void {
    while(thread.clock() < clock() && thread.handle()) {
        if(scheduler.synchronizing()) break;
        co_switch(thread.handle());
    }
    if constexpr(sizeof...(p) > 0) synchronize(std::forward<P>(p)...);
}
```

**The while-not-if rule:** Switching to another thread doesn't guarantee it catches up before switching back. The loop ensures the target runs until it's no longer behind.

---

## 3. JIT (Just-In-Time) Synchronization — The Critical Optimization

Naive: synchronize ALL components after EVERY clock step → millions of context switches/second.

**JIT insight:** Only synchronize component B when component A actually accesses shared state with B.

```cpp
uint8_t CPU::readMemory(uint16_t address) {
    wait(2);
    if(address >= 0x2140 && address <= 0x2143) {
        // Only sync APU when accessing shared APU registers
        while(apuCounter > 0) yield(apu);
    }
    uint8_t data = memory[address];
    wait(4);
    return data;
}

void CPU::wait(uint clockCycles) {
    apuCounter += clockCycles;  // just accumulate — no yield
}
```

Result: synchronizations drop from millions/second to thousands/second. CPU can run hundreds of instructions ahead of APU as long as it doesn't touch shared registers.

---

## 4. State Serialization

Three methods (in order of portability vs. determinism):

| Method | Technique | Use Case | Portability |
|--------|-----------|----------|-------------|
| **Fast sync** | Walk each thread to `main()` entry; ignore stack | Save states (may miss a cycle) | Fully portable |
| **Strict sync** | Same + loop until no further thread moved | Manual save states (correct) | Fully portable |
| **Hibernation** | Copy entire coroutine stack to buffer | Run-ahead, rewind | Non-portable (absolute pointers) |

```cpp
// ares Thread::serialize — hybrid approach
auto Thread::serialize(serializer& s) -> void {
    s(_frequency); s(_scalar); s(_clock);
    if(!scheduler._synchronize) {
        // Hibernation: copy entire stack
        static u8 stack[Thread::Size];
        memory::copy(_handle, stack, Thread::Size);
        s(stack);
    }
}
```

---

## 5. Run-Ahead Technique

Reduces input latency by N frames by pre-running the simulation:

```
1. poll_input()
2. run_frame()          → discard video/audio (frame N: user input applied)
3. save_state = serialize()
4. run_frame() × N      → discard (run-ahead frames)
5. run_frame() → display (displayed frame is N frames ahead)
6. unserialize(save_state)  → restore to state after frame N
```

Overhead: ~40% per run-ahead frame (not 100% — video generation skipped). Requires fast serialization.

---

## 6. Emulator Hierarchy: Three Models

| Model | Description | Weakness |
|-------|-------------|----------|
| Hard-coded | System-specific code per peripheral | Doesn't generalize |
| List | `vector<CartridgePort>`, `vector<ControllerPort>` | Can't represent dynamic topologies |
| **Tree (higan v107+)** | Every component is `Object { children: Vec<Object> }` | Steep UX curve |

The tree model allows Sufami Turbo to add child `CartridgePort` nodes at load time. Micro Machines 2 cartridge (Genesis) can expose additional controller ports.

```cpp
struct Object { string name; vector<shared_ptr<Object>> children; };
```

---

## Mapping Higan → helm-ng Design

| Higan Concept | Higan | helm-ng Analogue | Action Needed |
|---------------|-------|-----------------|---------------|
| Relative scheduler | `int64 counter`, cross-multiply | N/A (single hart for now) | Use if multi-freq device clocks needed |
| Absolute scheduler | `u64 clock`, `scalar = Second/freq` | `global_tick = min(hart_ticks)` | Add freq-normalized scalars for device clocks |
| Cooperative threading | libco coroutines | Not used — explicit pipeline stages | Not needed; explicit state avoids serialization complexity |
| JIT synchronization | Sync only on shared-memory access | `EventQueue::drain_until` at interval boundary | **Fine-grain: drain on device-register access** |
| Catch-up (while loop) | `while(peer.clock < my.clock)` | EventQueue drain loop | Already follows while-pattern |
| Run-ahead | serialize + N frames + unserialize | `can_skip_to/skip_to` hypersimulation | Use for checkpoint-replay testing |
| Tree hierarchy | `Object { children }` | `SimObject` tree | Already aligned |
| Fast serialization | Walk thread to entry point | Trivial — pipeline stage registers explicit | No change needed |

---

## Key Actionable Findings for helm-ng

**1. AccuratePipeline::step() granularity is correct.**
Higan's most accurate mode steps one CPU cycle at a time. helm-ng's `AccuratePipeline` advancing one pipeline cycle per `step()` call is the right granularity.

**2. Device-register JIT synchronization pattern.**
When `step_mem()` detects a device-mapped address, advance that device's simulated clock to `current_cycles` before performing the access. This is higan JIT sync applied to a processor simulator.

**3. The while-not-if rule for catch-up.**
Any place a hart checks a shared device register, drain the EventQueue in a `while` loop until `event_queue.peek_next_tick() > current_cycles` — not just once.

**4. Scalar normalization for multi-freq clocks.**
If modeling a memory controller at 3200 MHz alongside a 3 GHz CPU:
```rust
const SECOND: u64 = u64::MAX >> 1;
let cpu_scalar  = SECOND / 3_000_000_000u64;
let mem_scalar  = SECOND / 3_200_000_000u64;
// Both advance in the same u64 timestamp space. No fractional math.
```

**5. Serialization advantage.**
helm-ng's explicit-state-machine pipeline avoids higan's biggest serialization headache (cooperative thread stacks). Checkpointing `AccuratePipeline` = serialize the five stage registers + cycle count. No hibernation or stack copying needed.

---

## Sources

- [higan-emu/emulation-articles — schedulers](https://github.com/higan-emu/emulation-articles/blob/master/design/schedulers/README.md)
- [higan-emu/emulation-articles — cooperative-threading](https://github.com/higan-emu/emulation-articles/blob/master/design/cooperative-threading/README.md)
- [higan-emu/emulation-articles — cooperative-serialization](https://github.com/higan-emu/emulation-articles/blob/master/design/cooperative-serialization/README.md)
- [higan-emu/emulation-articles — hierarchy](https://github.com/higan-emu/emulation-articles/blob/master/design/hierarchy/README.md)
- [higan-emu/emulation-articles — run-ahead](https://github.com/higan-emu/emulation-articles/blob/master/input/run-ahead/README.md)
- [ares-emulator/ares — scheduler.hpp](https://github.com/ares-emulator/ares/blob/master/ares/ares/scheduler/scheduler.hpp)
- [ares-emulator/ares — thread.hpp](https://github.com/ares-emulator/ares/blob/master/ares/ares/scheduler/thread.hpp)
