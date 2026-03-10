# FS Session

The `FsSession` struct orchestrates full-system simulation.

## Ownership

```rust
pub struct FsSession {
    cpu: Aarch64Cpu,          // AArch64 executor with TLB
    mem: AddressSpace,         // Guest RAM + MMIO
    irq_signal: IrqSignal,     // Shared with GIC
    timing: Box<dyn TimingModel>,
    backend: ExecBackend,      // JIT or interpreter
    compiled_cache: Vec<...>,  // Threaded block cache
    jit_engine: Option<JitEngine>,
    jit_cache: Vec<...>,       // JIT block cache (64K entries)
    symbols: SymbolTable,
    halted: bool,
}
```

## Run Loop

1. Check if CPU is halted (WFI); if so, check IRQ and return.
2. Sync registers to the TCG array (`regs_to_array`).
3. Translate the block at current PC:
   - Check JIT cache hit → execute native code.
   - Check compiled cache hit → execute threaded bytecode.
   - Miss → translate block with `A64TcgEmitter`, cache it.
4. Handle exit code:
   - `EndOfBlock` / `Chain` → update PC, continue.
   - `Syscall` → route SVC exception.
   - `Wfi` → set halted.
   - `Exception` → call `take_exception`.
   - `ExceptionReturn` → call ERET logic.
5. Sync registers back (`array_to_regs`).
6. Check timers, IRQs.
7. Increment instruction and cycle counters.

## Fallback

When `A64TcgEmitter::translate_insn()` returns `Unhandled`, the
session falls back to `Aarch64Cpu::step()` for that instruction.

## Session API

| Method | Description |
|--------|-------------|
| `new(kernel, opts)` | Load kernel, build platform |
| `run(max_insns)` | Execute up to N instructions |
| `run_until_symbol(sym)` | Run to named kernel symbol |
| `run_until_pc(target)` | Run to specific PC |
| `pc()` / `xn(n)` / `regs()` | Register inspection |
| `read_memory(addr, size)` | Physical memory read |
| `read_virtual(va, size)` | Virtual memory read (MMU) |
| `stats()` | Instruction/cycle/IRQ counters |
