# SE Session

The `SeSession` struct orchestrates syscall-emulation mode.

## Ownership

```rust
pub struct SeSession {
    cpu: Aarch64Cpu,
    mem: AddressSpace,
    syscall: Aarch64SyscallHandler,
    sched: Scheduler,          // Thread scheduler (futex, clone)
    backend: ExecBackend,
    plugin_reg: PluginRegistry,
    comp_reg: ComponentRegistry,
    adapters: Vec<PluginComponentAdapter>,
    timing: Box<dyn TimingModel>,
    symbols: SymbolTable,
}
```

## Run Loop

1. Execute one instruction via the selected backend:
   - `Interpretive` → `Aarch64Cpu::step()`.
   - `Tcg` → translate block + interpret/JIT.
2. Check for syscall (`HelmError::Syscall`):
   - Dispatch to `Aarch64SyscallHandler::handle()`.
   - Handle scheduling actions (futex, clone, thread exit).
3. Fire plugin callbacks (instruction, memory, syscall).
4. Accumulate timing: `virtual_cycles += timing.instruction_latency_for_class(class)`.
5. Check stop conditions: instruction limit, breakpoint PC, exit.

## Plugin Hot-Loading

`add_plugin(name, args)` during a paused session:

1. Look up the plugin type in `ComponentRegistry`.
2. Create a `PluginComponentAdapter`.
3. Call `adapter.install(&mut plugin_reg, &args)`.
4. The plugin's callbacks become active on the next `run()`.

## StopReason

| Variant | Meaning |
|---------|---------|
| `InsnLimit` | Reached instruction budget |
| `Breakpoint { pc }` | Hit requested PC |
| `Exited { code }` | Guest called exit/exit_group |
| `Error(msg)` | Unrecoverable error |

## Thread Scheduler

`Scheduler` supports multi-threaded SE workloads:
- `futex_wait` / `futex_wake` — futex emulation.
- `clone` — thread creation with separate register state.
- Round-robin scheduling with configurable quantum.
