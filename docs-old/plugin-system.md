# Plugin System

HELM's instrumentation framework lives in the `helm-plugin` crate. Plugins
observe simulation events without modifying the engine core, enabling trace
collection, profiling, cache simulation, and custom analysis.

The design is modelled after QEMU's TCG plugin API (`qemu-plugin.h`).

---

## Crate Structure

```
crates/helm-plugin/src/
├── lib.rs
├── api/                      Stable public API
│   ├── plugin.rs             HelmPlugin trait
│   ├── component.rs          HelmComponent trait (object-model integration)
│   ├── metadata.rs           PluginMetadata, version checking
│   ├── loader.rs             ComponentRegistry (type-name → factory)
│   └── dynamic.rs            Dynamic .so loading (feature = "dynamic")
├── runtime/                  Engine-facing infrastructure
│   ├── registry.rs           PluginRegistry — callback storage / dispatch
│   ├── callback.rs           Callback type definitions
│   ├── scoreboard.rs         Per-vCPU lock-free scoreboard helper
│   ├── bridge.rs             HelmPlugin → HelmComponent adapter
│   └── info.rs               InsnInfo, MemInfo structs
└── builtins/                 Built-in plugins (feature = "builtins", default)
    ├── trace/
    │   ├── insn_count.rs     Instruction counter
    │   ├── hotblocks.rs      Top-N hot basic blocks by PC frequency
    │   ├── execlog.rs        Per-instruction execution trace log
    │   ├── howvec.rs         Histogram of how often each PC executes
    │   └── syscall_trace.rs  Log every syscall number and arguments
    └── memory/
        └── cache_sim.rs      Software cache simulator with hit-rate reporting
```

---

## Plugin Trait

```rust
pub trait HelmPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn install(&mut self, registry: &mut PluginRegistry);
    fn report(&self) -> String { String::new() }
}
```

Implement `install()` to register callbacks. All callbacks are fired by the
engine at the appropriate simulation points.

---

## Available Callbacks

| Callback | When fired | Signature |
|----------|-----------|-----------|
| `on_vcpu_init` | vCPU thread starts | `fn(vcpu_idx: usize)` |
| `on_insn_exec` | each instruction committed | `fn(vcpu: usize, insn: &InsnInfo)` |
| `on_mem_access` | each memory read/write | `fn(vcpu: usize, addr: Addr, size: usize, is_write: bool)` |
| `on_syscall` | syscall entry | `fn(nr: u64, args: &[u64])` |
| `on_syscall_ret` | syscall return | `fn(nr: u64, ret: u64)` |

Callbacks are stored in `PluginRegistry` as `Box<dyn Fn(...)>` and fired
in registration order. They do not block the simulation thread — any heavy
work should be deferred via channels.

---

## Using a Plugin

```rust
use helm_plugin::{PluginRegistry, HelmPlugin};

let mut registry = PluginRegistry::new();

// Register a built-in plugin
registry.install(helm_plugin::builtins::trace::InsnCount::new());

// Or register raw callbacks
let count = Arc::new(AtomicU64::new(0));
let c = count.clone();
registry.on_insn_exec(Box::new(move |_vcpu, _insn| {
    c.fetch_add(1, Ordering::Relaxed);
}));

// Pass to simulation
run_aarch64_se_with_plugins(binary, args, env, max_cycles, Some(&registry));
println!("Instructions: {}", count.load(Ordering::Relaxed));
```

---

## Built-in Plugins

### `insn_count`

Counts committed instructions. Zero overhead — single atomic increment per
instruction.

```rust
use helm_plugin::builtins::trace::InsnCount;
let mut p = InsnCount::new();
// after simulation:
println!("{}", p.report()); // "instructions: 1234567"
```

### `hotblocks`

Tracks the N most frequently executed basic-block PCs.

```rust
use helm_plugin::builtins::trace::HotBlocks;
let mut p = HotBlocks::new(20); // top 20 blocks
// report() returns a sorted table of PC → count
```

### `execlog`

Logs every instruction to a file or string buffer (for debugging). High
overhead — use for short traces only.

### `cache_sim`

Software-simulated set-associative cache. Reports hit rate, MPKI, and miss
counts per level after simulation.

```rust
use helm_plugin::builtins::memory::CacheSim;
let mut p = CacheSim::new(32 * 1024, 8, 64); // 32 KB, 8-way, 64B lines
```

---

## Dynamic Plugin Loading

Enable the `dynamic` feature:

```toml
helm-plugin = { workspace = true, features = ["dynamic"] }
```

Dynamic plugins are shared libraries that export `helm_plugin_install()`:

```c
// myplugin.c  (or Rust with #[no_mangle])
void helm_plugin_install(HelmPluginCtx *ctx) {
    helm_plugin_register_vcpu_tb_trans(ctx, on_tb_trans, NULL);
}
```

```rust
use helm_plugin::api::loader::ComponentRegistry;
let mut reg = ComponentRegistry::new();
reg.load_dynamic("/path/to/libmyplugin.so")?;
```

---

## Scoreboard

`Scoreboard` is a per-vCPU associative store for plugin state that avoids
cross-thread data races. Each plugin gets its own slot keyed by plugin name.

```rust
let mut sb = Scoreboard::new();
sb.insert("my_plugin", vcpu_idx, MyState::default());
let state = sb.get_mut("my_plugin", vcpu_idx)?;
state.count += 1;
```
