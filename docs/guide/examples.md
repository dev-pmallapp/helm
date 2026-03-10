# Examples Walkthrough

Guide to the scripts in the `examples/` directory.

## SE Mode

### `examples/se/run_binary.py`

Basic SE-mode execution of an AArch64 binary using the Python API.
Demonstrates `SeSession` construction, running with an instruction
budget, and inspecting results.

## FS Mode

### `examples/fs/virt.py`

Boot a Linux kernel on the ARM `virt` machine. Shows `FsSession`
creation with kernel path, machine type, kernel command line, and
memory size.

### `examples/fs/rpi3.py`

Boot a Linux kernel on the Raspberry Pi 3 (BCM2837) platform.

## Debug Scripts

### `examples/debug/benchmark.py`

Compare execution speed across different timing models and backends.

### `examples/debug/boot_progress.py`

Monitor FS-mode boot progress by running in increments and printing
PC, exception level, and instruction count.

### `examples/debug/compare_backends.py`

Run the same workload with interpreter vs JIT backends and verify
register state matches.

### `examples/debug/dump_sysregs.py`

Read and print system register values during FS-mode execution.

### `examples/debug/read_memory.py`

Read physical and virtual memory from a running FS session.

## Running Examples

```bash
# SE mode
PYTHONPATH=python python3 examples/se/run_binary.py

# FS mode (requires kernel image)
PYTHONPATH=python python3 examples/fs/virt.py

# Via embedded Python
cargo run --release --bin helm-arm -- examples/se/run_binary.py
cargo run --release --bin helm-system-aarch64 -- examples/fs/virt.py
```
