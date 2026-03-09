# Adding Devices

How to add a new device model to HELM.

## Steps

### 1. Create the Device Module

Add a new file in `crates/helm-device/src/` (or under `arm/` for
ARM-specific peripherals):

```rust
pub struct MyDevice {
    region: MemRegion,
    // device state
}

impl Device for MyDevice {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            self.handle_write(txn.offset, txn.size, txn.data_u64())?;
        } else {
            let val = self.handle_read(txn.offset, txn.size)?;
            txn.set_data_u64(val);
        }
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] { &[self.region.clone()] }
    fn name(&self) -> &str { "my-device" }
    fn reset(&mut self) -> HelmResult<()> { /* ... */ Ok(()) }
}
```

### 2. Wire into a Platform

In `crates/helm-device/src/platform.rs`:

```rust
platform.add_device("my-device", 0x1234_0000, Box::new(MyDevice::new()));
```

### 3. Add DTB Node (Optional)

If the device needs a device tree entry, add it to the FDT builder.

### 4. Add IRQ Route (Optional)

```rust
platform.add_irq_route(IrqRoute { src_device: "my-device", src_line: 0,
                                    dst_irq: 42 });
```

### 5. Write Tests

Add tests in `crates/helm-device/src/tests/`:
- Register read/write behaviour.
- Reset state.
- IRQ assertion.

### 6. Run Pre-Commit

```bash
make pre-commit
```
