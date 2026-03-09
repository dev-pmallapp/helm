# DTB Generation

Flattened Device Tree building and patching.

## Strategy

DTB handling is inferred from CLI arguments (like QEMU and gem5):

| DtbPolicy | When |
|-----------|------|
| `Generate` | `-kernel` without `--dtb` |
| `Patch` | `--dtb` provided + extra devices |
| `Passthrough` | `--dtb` provided, no extras |
| `None` | `-bios` or drive-only boot |

## FdtBuilder

Constructs a DTB binary from scratch:

1. Write the FDT header (40 bytes).
2. Write the memory reservation map.
3. Write the structure block (nodes and properties).
4. Write the strings block.

## DtbConfig

Controls DTB generation:

```rust
pub struct DtbConfig {
    pub ram_base: u64,
    pub ram_size: u64,
    pub num_cpus: u32,
    pub bootargs: String,
    pub initrd_start: Option<u64>,
    pub initrd_end: Option<u64>,
    // ...
}
```

## RuntimeDtb

Combines a platform description with a DTB config to produce the
final DTB blob. Supports:
- Fresh generation from platform devices.
- Patching an existing DTB with additional device nodes.
- CLI overlay of boot arguments, memory size, CPU count.

## FDT Binary Layout

```text
┌──────────────────────────┐  offset 0
│  fdt_header (40 bytes)   │
├──────────────────────────┤  off_mem_rsvmap
│  memory reservation map  │
├──────────────────────────┤  off_dt_struct
│  structure block         │
├──────────────────────────┤  off_dt_strings
│  strings block           │
└──────────────────────────┘  totalsize
```
