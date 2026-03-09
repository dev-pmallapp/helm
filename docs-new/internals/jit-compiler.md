# JIT Compiler

The Cranelift-based JIT in `helm-tcg` compiles `TcgBlock`s to native
machine code.

## Compilation Pipeline

```text
TcgBlock (Vec<TcgOp>)
    │
    ▼
JitEngine::compile(block)
    │
    ├── Create Cranelift function signature:
    │     fn(regs: *mut u64, mem: *mut u8, sysregs: *mut u64) -> i64
    │
    ├── Lower each TcgOp to Cranelift IR:
    │   ├── Movi  → iconst
    │   ├── Add   → iadd
    │   ├── Load  → call helper_load()
    │   ├── Store → call helper_store()
    │   ├── ReadReg  → load from regs array
    │   ├── WriteReg → store to regs array
    │   ├── BrCond → brif
    │   └── ...
    │
    ├── Cranelift optimisation + register allocation
    │
    └── Emit native code → JitBlock { fn_ptr, size }

JitBlock::execute(regs, mem, sysregs)
    │
    └── Returns exit code:
        0 = EndOfBlock, 1 = Chain, 2 = Syscall,
        3 = Wfi, 4 = Eret, 5 = Exception
```

## Helper Functions

JIT-compiled code calls extern "C" helper functions for operations
that cannot be inlined:

- `helper_load(regs, mem, va, size)` — guest memory read with optional
  VA→PA translation.
- `helper_store(regs, mem, va, val, size)` — guest memory write.
- `helper_read_sysreg(sysregs, idx)` — system register read.
- `helper_write_sysreg(sysregs, idx, val)` — system register write.

## VA→PA Translation Callback

In FS mode, a `TranslateVaFn` callback is registered globally. JIT
helpers call it to translate virtual addresses through the MMU/TLB
before accessing physical memory.

## Block Cache

`FsSession` maintains a direct-mapped JIT cache:

- 64K entries indexed by `(pc >> 2) & 0xFFFF`.
- Each entry stores the guest PC and compiled `JitBlock`.
- Cache miss triggers compilation of a new block.
