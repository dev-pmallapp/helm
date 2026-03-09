# FsOpts

Configuration fields for `FsSession::new()`.

## Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `machine` | `String` | `"virt"` | Machine type: `virt`, `realview-pb`, `rpi3` |
| `append` | `String` | `""` | Kernel command line |
| `memory_size` | `String` | `"256M"` | Physical RAM size |
| `dtb` | `Option<String>` | `None` | External DTB path |
| `sysmap` | `Option<String>` | `None` | System.map for symbol resolution |
| `serial` | `String` | `"stdio"` | UART0 backend: `stdio`, `null` |
| `timing` | `String` | `"fe"` | Timing model: `fe`, `ite` |
| `backend` | `String` | `"jit"` | Execution backend: `jit`, `interp` |
| `max_insns` | `u64` | `u64::MAX` | Maximum instructions |

## Memory Size Format

Accepts standard size suffixes: `K`, `M`, `G` (e.g. `"256M"`, `"1G"`).
Parsed by `helm_device::parse_ram_size()`.

## Serial Backends

| Value | Behaviour |
|-------|-----------|
| `"stdio"` | UART I/O to host terminal |
| `"null"` | Discard output, no input |

## Timing Models

| Value | Model | Description |
|-------|-------|-------------|
| `"fe"` | `FeModel` | IPC=1, fastest |
| `"ite"` | `IteModelDetailed` | Per-class latencies |

## Execution Backends

| Value | Description |
|-------|-------------|
| `"jit"` | Cranelift JIT compilation (fastest) |
| `"interp"` | TCG interpreter (most debuggable) |
