# FAQ

## General

**Q: What ISAs does HELM support?**
A: AArch64 (ARMv8-A) is fully implemented. RISC-V and x86-64 have
stub frontends with the `IsaFrontend` trait ready for implementation.

**Q: Can I run dynamically-linked binaries in SE mode?**
A: Not yet. SE mode requires statically-linked AArch64 ELF binaries.
The ELF loader does not handle `PT_INTERP` or shared libraries.

**Q: What Linux syscalls are supported?**
A: Approximately 50 syscalls including read, write, openat, close,
mmap, brk, exit, ioctl, fcntl, clock_gettime, uname, and getrandom.
See the `helm-syscall` crate for the full list.

## FS Mode

**Q: My kernel hangs during boot. How do I debug?**
A: Use `--serial stdio` to see early boot messages. Add
`earlycon=pl011,0x09000000` to the kernel command line for early
console output. Use `--backend interp` for deterministic debugging.
Check `--sysmap` with a `System.map` file to see which symbol the
PC corresponds to.

**Q: Which kernel versions work?**
A: Linux 5.x and 6.x ARM64 kernels have been tested. The kernel must
be built as a standalone `Image` (not `zImage` for 32-bit). Compressed
images (gzip) are automatically decompressed.

**Q: How do I add a block device?**
A: Use `--drive file=rootfs.img,format=raw` to attach a VirtIO block
device. Add `root=/dev/vda` to the kernel command line.

## Performance

**Q: How fast is HELM?**
A: In FE mode with JIT compilation, HELM achieves 10–100 MIPS
depending on the workload. The interpreter is slower (1–10 MIPS).
ITE mode adds per-instruction timing overhead. CAE mode is the
slowest at 0.1–1 MIPS.

**Q: How do I profile HELM itself?**
A: Build with `cargo build --profile profiling` to get release
optimisations with debug symbols. Use `perf record` or your preferred
profiler.

## Plugins

**Q: Can I write my own plugin?**
A: Yes. Implement the `HelmPlugin` trait in Rust, or extend
`PluginBase` in Python. Register callbacks in `install()` for
instruction, memory, syscall, or fault events.

**Q: Can plugins be loaded at runtime?**
A: Yes. Both `SeSession` and `FsSession` support hot-loading plugins
between simulation phases via `add_plugin()`.
