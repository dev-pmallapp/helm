# Adding Platforms

How to add a new machine type to HELM.

## Steps

### 1. Create the Rust Platform Function

In `crates/helm-device/src/platform.rs`, add a builder function:

```rust
pub fn my_platform(uart: Box<dyn CharBackend>) -> Platform {
    let mut platform = Platform::new("my-machine");
    platform.add_device("uart0", 0x1000_0000, Box::new(Pl011::new("uart0", uart)));
    platform.add_device("gic", 0x2000_0000, Box::new(Gic::new("gic", 64)));
    platform
}
```

### 2. Register in FsSession

In `crates/helm-engine/src/fs/session.rs`, add a match arm:

```rust
"my-machine" => helm_device::my_platform(serial_backend),
```

### 3. Add CLI Support

In `crates/helm-cli/src/bin/helm_system_aarch64.rs`, the `-M` flag
already dispatches to `FsSession::new()`, so no changes needed if the
machine name is handled by the session.

### 4. Add Python Platform (Optional)

Create `python/helm/platforms/my_machine.py` with the platform
definition.

### 5. Add Memory Map Documentation

Create `docs-new/reference/memory-map-my-machine.md` with the address
layout.

### 6. Write Tests

Add a test in `crates/helm-device/src/tests/platform.rs` that
constructs the platform and verifies device mapping.

### 7. Add Example Script

Create `examples/fs/my_machine.py` showing how to boot on the new
platform.
