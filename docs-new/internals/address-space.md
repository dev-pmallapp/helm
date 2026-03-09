# Address Space

`helm-memory::address_space::AddressSpace` provides the guest memory model.

## Structure

- `regions: Vec<MemRegion>` — mapped memory regions.
- `io: Option<Box<dyn IoHandler>>` — fallback for unmapped addresses.

Each `MemRegion` has: `base`, `size`, `data` (`Vec<u8>`), and `rwx` permission flags.

## Operations

| Method | Description |
|--------|-------------|
| `map(base, size, rwx)` | Map a new region |
| `read(addr, buf)` | Read bytes; falls back to IoHandler |
| `write(addr, buf)` | Write bytes; falls back to IoHandler |
| `read_phys(addr, buf)` | Read from RAM only (no I/O fallback) |
| `set_io_handler(handler)` | Set the MMIO fallback |
| `write_to(addr, data)` | Bulk write (ELF segment loading) |

## I/O Handler

The `IoHandler` trait provides MMIO dispatch for FS mode:

```rust
pub trait IoHandler {
    fn io_read(&mut self, addr: Addr, size: usize) -> Option<u64>;
    fn io_write(&mut self, addr: Addr, size: usize, value: u64) -> bool;
}
```

When a read/write misses all RAM regions, the address space calls
the I/O handler. In FS mode this routes to the device bus.

## Usage

In SE mode, regions are created by the ELF loader for code, data,
stack, heap, and guard pages. In FS mode, a large RAM region is
mapped at the platform's DRAM base address.
