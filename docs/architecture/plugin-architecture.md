# Plugin Architecture

HELM's instrumentation system — how plugins hook into the simulation
for tracing, analysis, and debugging.

## Design

The plugin system is consolidated in `helm-plugin` with three layers:

1. **API** (`api` module) — stable traits for plugin authors.
2. **Runtime** (`runtime` module) — callback registry, scoreboard,
   and bridge infrastructure.
3. **Built-ins** (`builtins` module) — shipped plugins for common
   use cases.

## Plugin Traits

### HelmPlugin

The primary trait for trace/analysis plugins:

```rust
pub trait HelmPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs);
    fn atexit(&mut self);
}
```

Plugins register callbacks in `install()` and clean up in `atexit()`.

### HelmComponent

A more general trait for pluggable simulation components (devices,
timing models, cache policies):

```rust
pub trait HelmComponent: Send + Sync {
    fn component_type(&self) -> &'static str;
    fn interfaces(&self) -> &[&str];
    fn realize(&mut self) -> HelmResult<()>;
    fn reset(&mut self) -> HelmResult<()>;
    fn tick(&mut self, cycles: u64) -> HelmResult<()>;
}
```

## Callback Registry

`PluginRegistry` stores all registered callbacks:

| Hook | Signature | When Fired |
|------|-----------|------------|
| `vcpu_init` | `(vcpu_idx)` | vCPU creation |
| `vcpu_exit` | `(vcpu_idx)` | vCPU teardown |
| `tb_trans` | `(pc, size)` | Translation block generated |
| `tb_exec` | `(pc, insn_count)` | Block executed |
| `insn_exec` | `(vcpu_idx, insn_info)` | Per-instruction |
| `mem_access` | `(vcpu_idx, addr, size, rw)` | Memory operation (filterable) |
| `syscall` | `(syscall_info)` | Syscall entry |
| `syscall_ret` | `(ret_info)` | Syscall return |
| `fault` | `(fault_info)` | Execution fault |

Memory callbacks support `MemFilter` for address-range filtering.

## ComponentRegistry

`ComponentRegistry` maps fully-qualified type names to factory
functions. `register_builtins()` populates it with all built-in
plugins. The CLI resolves short names (e.g. `"cache"` →
`"plugin.memory.cache"`).

## Hot-Loading

Plugins can be loaded between simulation phases:

```python
s = SeSession("./binary", ["binary"])
s.run(1_000_000)           # Phase 1: no plugins
s.add_plugin(FaultDetect()) # Hot-load
s.run(10_000_000)          # Phase 2: with plugin
```

The `SeSession::add_plugin()` method registers the plugin's callbacks
with the active `PluginRegistry` without restarting the simulation.

## Built-In Plugins

See [plugin-catalog.md](../reference/plugin-catalog.md) for the full
list with parameters and output formats.
