# Execution Pipeline

How guest instructions flow from fetch through execution in HELM.

## Dual Execution Paths

The same `.decode` file drives two code-generation backends:

```text
.decode file
    │
    ├─► TCG backend  ─► TcgOp chain ─► interp / JIT    (SE/FE path)
    │
    └─► Static backend ─► MicroOp vec ─► pipeline model (APE/CAE path)
```

### SE/FE Path (TCG)

Used for fast functional emulation and SE-mode execution:

1. **Fetch** — read 4 bytes from `AddressSpace` at the current PC.
2. **Decode** — the `A64TcgEmitter` dispatches through generated
   `decode_aarch64_*_dispatch()` functions (auto-generated from
   `.decode` files at build time via `helm-decode`).
3. **Translate** — the emitter produces a `TcgBlock` (sequence of
   `TcgOp`s covering one basic block).
4. **Execute** — three backends are available:
   - **Interpreter** (`TcgInterp`) — match-based dispatch over `TcgOp`
     variants; simplest and most debuggable.
   - **Threaded interpreter** — flat bytecode + function-pointer
     dispatch; avoids per-op match overhead.
   - **JIT** (`JitEngine`) — Cranelift compiles `TcgOp` sequences to
     native x86-64 or AArch64 machine code.

### Direct Executor Path

For the simplest SE use-case, `Aarch64Cpu::step()` in `helm-isa`
decodes and executes each instruction directly on `Aarch64Regs` +
`AddressSpace`, bypassing TCG entirely. This is the most
straightforward path and is used by the `helm` CLI binary.

### APE/CAE Path (MicroOp Pipeline)

Used for microarchitectural simulation:

1. **Decode** — `Aarch64Decoder` converts the instruction word into a
   `Vec<MicroOp>` with opcode classification (IntAlu, Load, FpMul, …).
2. **Rename** — `RenameUnit` maps architectural registers to physical
   registers (RAT + free list).
3. **Dispatch** — `ReorderBuffer::allocate()` reserves a ROB entry;
   `Scheduler::insert()` queues the uop.
4. **Issue** — `Scheduler::select()` picks ready uops (up to `width`
   per cycle) for execution.
5. **Execute** — the `TimingModel` provides per-class latencies.
6. **Complete** — `rob.complete(idx)` marks the entry done.
7. **Commit** — head of ROB retires in program order; sends
   `SimEvent::InsnCommit` to the stats collector.

## Block Caching

Both the TCG interpreter and JIT maintain block caches keyed by guest
PC to avoid re-translation:

- **Threaded cache** — `Vec<Option<BlockCacheEntry>>` in `FsSession`,
  direct-mapped by `(pc >> 2) & mask`.
- **JIT cache** — `Vec<Option<JitCacheEntry>>` with 64K entries,
  stores compiled `JitBlock` function pointers.

Cache invalidation occurs on TLB flush or self-modifying code
detection.

## Fallback Handling

When the TCG emitter encounters an instruction it cannot translate
(returns `TranslateAction::Unhandled`), the FS session falls back to
the direct executor (`Aarch64Cpu::step()`) for that instruction before
resuming TCG translation at the next PC.
