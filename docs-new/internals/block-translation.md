# Block Translation

How guest basic blocks are translated and cached.

## translate_block_fs

In `FsSession`, block translation proceeds:

1. Create a `TcgContext` for accumulating ops.
2. Loop: fetch instruction at PC, call `A64TcgEmitter::translate_insn()`.
3. If `Continue` — advance PC by 4, continue.
4. If `EndBlock` — stop, return completed `TcgBlock`.
5. If `Unhandled` — stop, return partial block (fallback to step).

## Block Cache

Two caches, both direct-mapped by `(pc >> 2) & mask`:

| Cache | Entry Type | Size | Used By |
|-------|-----------|------|---------|
| Compiled cache | `BlockCacheEntry` (threaded bytecode) | 64K entries | Threaded interpreter |
| JIT cache | `JitCacheEntry` (native fn pointer) | 64K entries | JIT backend |

On hit: verify `entry.pc == current_pc` (collision detection).
On miss: translate + compile + insert.

## TranslateAction

| Variant | Meaning |
|---------|---------|
| `Continue` | Instruction translated; more to follow |
| `EndBlock` | Block-ending instruction (branch, SVC, WFI) |
| `Unhandled` | Emitter cannot translate; fall back |

## SE-Mode Translation

In SE mode, `helm-translate` provides a separate translation cache:
- `Translator` drives the ISA frontend to produce `TranslatedBlock`s.
- `TranslationCache` stores blocks keyed by guest PC.
- Used by the `MicroOp` pipeline path (APE/CAE).
