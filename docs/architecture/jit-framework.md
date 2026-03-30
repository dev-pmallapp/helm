# JIT Framework

How helm-ng translates guest instructions to host machine code for
accelerated execution.

## Architecture

The JIT framework lives in `helm-jit` and is designed as a pluggable
backend system. The engine (`helm-engine`) integrates JIT execution
via optional feature flags, keeping the interpreter as the default
and always-available path.

```text
                    ┌──────────────┐
                    │ helm-engine  │
                    │  run_jit()   │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │  JitBackend  │  (trait)
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
       ┌──────▼─────┐     │     ┌──────▼──────┐
       │   dynasm    │     │     │  (future)   │
       │  backend    │     │     │  Cranelift  │
       └─────────────┘     │     └─────────────┘
                           │
                    ┌──────▼───────┐
                    │  JitCache    │
                    │  (4096-entry │
                    │  direct-map) │
                    └──────────────┘
```

## JitBackend Trait

```rust
pub trait JitBackend: Send {
    fn compile(&mut self, pc: u64, memory: &[u8]) -> Option<CompiledBlock>;
}
```

A backend receives the starting PC and a slice of guest memory,
decodes a basic block of instructions, and returns a `CompiledBlock`
containing executable host machine code.

## CompiledBlock

```rust
pub struct CompiledBlock {
    // Executable memory (mmap'd with PROT_EXEC)
    // Entry point function pointer
    // Block length in guest instructions
}
```

The compiled block operates on a `[u64; 48]` register array passed
as the first argument (`rdi` on x86-64). This array maps directly to
guest registers (X0–X30, SP, PC, NZCV flags, etc.).

## JitCache

`JitCache` is a 4096-entry direct-mapped cache keyed by guest PC:

```text
index = (pc >> 2) & 0xFFF
```

On a cache miss, the backend is invoked to compile a new block.
On a cache hit, the compiled block's entry point is called directly.

Cache invalidation happens on:
- Self-modifying code detection
- TLB flush (FS mode)
- Explicit `flush()` call

## Dynasm Backend

The default backend (`helm-jit::dynasm`, enabled by
`backend-dynasm` feature) uses `dynasm-rs` to generate x86-64 machine
code at runtime.

The compilation pipeline:

```text
1. Decode AArch64 instructions from guest memory
2. For each instruction:
   a. Emit x86-64 code that reads guest regs from [rdi + offset]
   b. Perform the operation using host instructions
   c. Write results back to [rdi + offset]
3. Emit block epilogue (update PC, return)
4. Finalize: mmap executable page, return CompiledBlock
```

### Register Mapping

Guest registers are not mapped to host registers (unlike QEMU's TCG).
Instead, all guest state lives in the register array, and compiled code
loads/stores from it explicitly. This simplifies the backend at the
cost of some performance — a reasonable tradeoff for a research
simulator.

## Feature Flags

| Flag | Crate | Effect |
|------|-------|--------|
| `backend-dynasm` | `helm-jit` | Enable dynasm-rs code generator (default) |
| `jit-dynasm` | `helm-engine` | Wire JIT into `set_jit()` and `run_jit()` methods |

Both flags must be enabled for JIT execution. When disabled, only the
interpreter path is available.

## Comparison

| Aspect | QEMU | gem5 | helm-ng |
|--------|------|------|---------|
| JIT approach | TCG → custom backend per host | No JIT (interpreter only) | Pluggable `JitBackend` trait |
| Register allocation | TCG globals mapped to host regs | N/A | Explicit load/store from array |
| Block cache | Hash table | N/A | 4096-entry direct-mapped |
| Code generator | Hand-written per host arch | N/A | dynasm-rs (x86-64) |
| Backend swapping | Not supported | N/A | Implement `JitBackend` trait |
