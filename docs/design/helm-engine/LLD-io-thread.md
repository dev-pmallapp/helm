# helm-engine — LLD: IO Thread

> **Module:** `helm_engine::io_thread`
> **Cross-references:** [`LLD-world.md`](./LLD-world.md) · [`LLD-scheduler.md`](./LLD-scheduler.md) · [`../helm-devices/LLD-device-trait.md`](../helm-devices/LLD-device-trait.md) · [`../research/higan-accuracy.md`](../../research/higan-accuracy.md)

---

## Overview

helm-ng uses a **three-layer IO model** that combines:

1. **higan's cooperative scheduling** — for device model timing and state within the simulation world
2. **QEMU's IOThread pattern** — for actual host-side IO (disk images, network, host serial)
3. **An async channel bridge** — connecting the two layers cleanly

This document specifies Layer 2 (the IO bridge) and Layer 3 (the IO backend thread).

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Layer 1: Simulation World  (single thread, cooperative)        │
│                                                                 │
│  HelmEngine ──► EventQueue ──► Device::write() (VirtIODisk)    │
│                     ▲               │                           │
│                     │               │ io_thread.submit(req)     │
│                     │               ▼                           │
│              drain_completions()  IoRequest sent (non-blocking) │
└─────────────────────────┬───────────────────────────────────────┘
                          │  async_channel<IoCompletion>
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  Layer 2: IO Bridge  (thread-safe channel)                      │
│                                                                 │
│  IoRequest  → sender   ──────────────────► receiver → process  │
│  IoCompletion ← receiver ◄────────────────  sender ← complete  │
└─────────────────────────┬───────────────────────────────────────┘
                          │  tokio async runtime
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  Layer 3: IO Backend  (dedicated OS thread, async)              │
│                                                                 │
│  tokio::fs  (disk image reads/writes)                           │
│  tokio::net (VirtIO network, host networking)                   │
│  io_uring   (Linux high-performance path, optional)             │
└─────────────────────────────────────────────────────────────────┘
```

---

## Key Design Decisions

### Why not pure cooperative threading for IO?

higan's cooperative model works for console emulation because all "memory" is already in RAM (ROM is loaded at startup). Real IO — reading a disk image, receiving a network packet — blocks at the OS level. A cooperative yield cannot wait for an OS file read; it would deadlock.

### Why not pure async (tokio) throughout?

The simulation world requires **deterministic, reproducible timing**. Tokio's scheduler is non-deterministic — the order of event wakeups depends on OS scheduling. Running device models in a tokio task would break determinism and make checkpointing extremely hard. The simulation world must remain single-threaded and event-driven.

### The bridge pattern

QEMU solves this identically with `aio_bh_schedule()` — IO completion callbacks are marshalled from IOThreads into the main event loop as "bottom halves." helm-ng's `drain_completions()` is the Rust equivalent: the simulation thread polls the completion channel at EventQueue boundaries.

---

## Types

```rust
// helm-engine/src/io_thread.rs

/// A request submitted by a simulated device to the host IO backend.
pub enum IoRequest {
    DiskRead {
        image_id: u32,          // which disk image
        offset:   u64,          // byte offset into image
        len:      usize,        // bytes to read
        tag:      u64,          // opaque tag returned in IoCompletion
    },
    DiskWrite {
        image_id: u32,
        offset:   u64,
        data:     Vec<u8>,
        tag:      u64,
    },
    NetSend {
        iface_id: u32,
        frame:    Vec<u8>,
        tag:      u64,
    },
}

/// Delivered back to the simulation thread when host IO completes.
pub struct IoCompletion {
    pub tag:    u64,            // matches the tag from IoRequest
    pub result: IoResult,
}

pub enum IoResult {
    DiskReadOk(Vec<u8>),
    DiskWriteOk,
    NetSendOk,
    Error(IoError),
}

#[derive(Debug)]
pub enum IoError {
    EndOfImage,
    ImageNotFound(u32),
    HostIoError(std::io::Error),
}
```

---

## `IoThread` — The Bridge

```rust
pub struct IoThread {
    req_tx:   async_channel::Sender<IoRequest>,
    comp_rx:  async_channel::Receiver<IoCompletion>,
    handle:   std::thread::JoinHandle<()>,
}

impl IoThread {
    /// Spawn the IO backend thread. Call once during World construction.
    pub fn spawn(images: Vec<DiskImage>, ifaces: Vec<NetIface>) -> Self {
        let (req_tx, req_rx)   = async_channel::unbounded::<IoRequest>();
        let (comp_tx, comp_rx) = async_channel::unbounded::<IoCompletion>();

        let handle = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .unwrap()
                .block_on(io_backend_loop(req_rx, comp_tx, images, ifaces));
        });

        IoThread { req_tx, comp_rx, handle }
    }

    /// Submit an IO request from the simulation thread — never blocks.
    /// Called by simulated device models (e.g., VirtIODisk::write()).
    #[inline]
    pub fn submit(&self, req: IoRequest) {
        // try_send never blocks — unbounded channel
        self.req_tx.try_send(req).expect("IO thread gone");
    }

    /// Drain all pending completions. Called by World::drain_io() at
    /// every EventQueue boundary. Never blocks.
    pub fn drain_completions(&self) -> Vec<IoCompletion> {
        let mut out = Vec::new();
        while let Ok(c) = self.comp_rx.try_recv() {
            out.push(c);
        }
        out
    }

    /// Shutdown: drop all senders, then join the thread.
    pub fn shutdown(self) {
        drop(self.req_tx);
        self.handle.join().ok();
    }
}
```

---

## IO Backend Loop (Layer 3)

```rust
async fn io_backend_loop(
    req_rx:  async_channel::Receiver<IoRequest>,
    comp_tx: async_channel::Sender<IoCompletion>,
    images:  Vec<DiskImage>,
    ifaces:  Vec<NetIface>,
) {
    while let Ok(req) = req_rx.recv().await {
        let comp_tx = comp_tx.clone();
        let images  = images.clone();  // Arc<> internally

        // Spawn each IO operation as an independent task — true concurrency
        tokio::spawn(async move {
            let result = match req {
                IoRequest::DiskRead { image_id, offset, len, tag } => {
                    let result = images[image_id as usize].read(offset, len).await;
                    IoCompletion {
                        tag,
                        result: result
                            .map(IoResult::DiskReadOk)
                            .unwrap_or_else(IoResult::Error),
                    }
                }
                IoRequest::DiskWrite { image_id, offset, data, tag } => {
                    let result = images[image_id as usize].write(offset, &data).await;
                    IoCompletion {
                        tag,
                        result: result
                            .map(|_| IoResult::DiskWriteOk)
                            .unwrap_or_else(IoResult::Error),
                    }
                }
                IoRequest::NetSend { iface_id, frame, tag } => {
                    // fire-and-forget for now; errors logged but not returned
                    ifaces[iface_id as usize].send(&frame).await.ok();
                    IoCompletion { tag, result: IoResult::NetSendOk }
                }
            };
            comp_tx.send(result).await.ok();
        });
    }
}
```

Multiple IO operations run concurrently inside the tokio runtime — a slow disk read doesn't block a fast network send.

---

## Integration with `World`

`World` drains IO completions at every EventQueue boundary and routes them to the registered device:

```rust
// helm-engine/src/world.rs

impl World {
    /// Called by Scheduler at every quantum boundary and by advance().
    pub fn drain_io(&mut self) {
        if let Some(io) = &self.io_thread {
            for completion in io.drain_completions() {
                // Route completion to the device that owns this tag
                if let Some(device_id) = self.io_tag_registry.get(&completion.tag) {
                    if let Some(obj) = self.objects.get_mut(*device_id) {
                        obj.as_device_mut()
                           .unwrap()
                           .on_io_complete(completion);
                    }
                }
            }
        }
    }

    /// Advance simulation time + drain IO completions.
    pub fn advance(&mut self, cycles: u64) {
        self.event_queue.drain_until(self.clock.tick + cycles, self);
        self.drain_io();
        self.clock.tick += cycles;
    }
}
```

---

## Device Integration: `VirtIODisk` Example

```rust
pub struct VirtIODisk {
    irq:         InterruptPin,
    io_thread:   Weak<IoThread>,     // shared from World
    next_tag:    u64,
    pending:     HashMap<u64, VirtqDescriptor>,  // tag → in-flight request
}

impl Device for VirtIODisk {
    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            QUEUE_NOTIFY => {
                let desc = self.pop_avail_descriptor();
                let tag  = self.next_tag;
                self.next_tag += 1;
                self.pending.insert(tag, desc.clone());

                // Submit real IO to host backend — non-blocking
                if let Some(io) = self.io_thread.upgrade() {
                    io.submit(IoRequest::DiskRead {
                        image_id: 0,
                        offset:   desc.sector * 512,
                        len:      desc.len,
                        tag,
                    });
                }

                // Fire a simulated latency event — device appears busy for
                // the modeled storage latency (from MicroarchProfile)
                // World::drain_io() will deliver the completion when ready.
            }
            _ => {}
        }
    }

    fn on_io_complete(&mut self, completion: IoCompletion) {
        if let Some(desc) = self.pending.remove(&completion.tag) {
            match completion.result {
                IoResult::DiskReadOk(data) => {
                    // Write data into guest memory
                    self.write_to_guest(desc.guest_addr, &data);
                    // Push to virtqueue used ring
                    self.push_used(desc.id, data.len() as u32);
                    // Raise interrupt
                    self.irq.assert();
                }
                IoResult::Error(e) => {
                    log::warn!("Disk IO error: {:?}", e);
                    self.push_used_error(desc.id);
                    self.irq.assert();
                }
                _ => unreachable!()
            }
        }
    }
}
```

---

## Cooperative Scheduling for Device State (Layer 1)

Within the simulation world, device state transitions follow higan's **JIT synchronization** principle:

```rust
// When CPU reads a device register, drain pending events first
impl World {
    pub fn mmio_read(&mut self, addr: u64, size: usize) -> Result<u64, MemFault> {
        // JIT: drain IO completions before device register access
        // (equivalent to higan's while(peer.clock < my.clock) yield(peer))
        self.drain_io();
        self.event_queue.drain_until(self.clock.tick, self);

        self.memory.read(addr, size)
    }
}
```

This ensures that when the guest OS reads the VirtIO used ring (after an interrupt), all pending completions have been processed — the device state is "current" at the moment of access.

---

## Higan Absolute Scheduler for Device Clocks (Future)

If helm-ng models devices running at frequencies different from the CPU (e.g., a memory bus at 3200 MHz alongside a 3 GHz CPU), apply higan's scalar normalization:

```rust
/// Higan-inspired absolute scheduler for multi-frequency components.
pub struct AbsoluteClock {
    clock:  u64,
    scalar: u64,
}

impl AbsoluteClock {
    const SECOND: u64 = u64::MAX >> 1;   // 2^63 - 1

    pub fn new(frequency_hz: u64) -> Self {
        AbsoluteClock {
            clock:  0,
            scalar: Self::SECOND / frequency_hz,
        }
    }

    /// Advance by N cycles of this component's clock.
    #[inline]
    pub fn step(&mut self, cycles: u64) {
        self.clock = self.clock.wrapping_add(self.scalar * cycles);
    }

    /// Is this component behind the reference clock?
    #[inline]
    pub fn behind(&self, reference: u64) -> bool {
        self.clock < reference
    }
}

// Usage: CPU at 3 GHz, memory bus at 3200 MHz — same timestamp space, no fractions
let cpu_clock = AbsoluteClock::new(3_000_000_000);
let mem_clock = AbsoluteClock::new(3_200_000_000);
```

All components advance in the same `u64` timestamp space. No fractional arithmetic. No integer rounding accumulation.

---

## Summary: What Goes Where

| IO Concern | Layer | Mechanism |
|-----------|-------|-----------|
| UART baud timer | Layer 1 (sim) | EventQueue `post_cycles(baud_period)` |
| VirtIO queue notification | Layer 1 (sim) | Device::write() → io_thread.submit() |
| Disk image read (host) | Layer 3 (async) | tokio::fs |
| Network packet receive | Layer 3 (async) | tokio::net + NetReceiver callback |
| Completion delivery | Bridge | drain_completions() at EventQueue boundary |
| Device register read sync | Layer 1 (sim) | JIT drain before mmio_read() |
| Multi-freq device clocks | Layer 1 (sim) | AbsoluteClock (scalar normalization) |
| Simulated IO latency | Layer 1 (sim) | EventQueue post_cycles(latency_cycles) |

---

## Testing

```rust
#[test]
fn test_disk_read_completion_delivered() {
    let mut world = World::new_with_io(vec![DiskImage::from_bytes(FAKE_DISK)]);
    let disk = world.add_device("disk", Box::new(VirtIODisk::new()));
    world.map_device(disk, 0x1000_0000);
    world.elaborate();

    // Trigger a disk read via MMIO
    world.mmio_write(0x1000_0000 + QUEUE_NOTIFY, 4, 0);

    // Advance time — IO backend completes, drain_io delivers completion
    world.advance(100_000);

    // Verify: interrupt was raised
    assert!(world.pending_interrupts().contains(&(disk, "irq_out".into())));
}

#[test]
fn test_concurrent_disk_reads_all_complete() {
    // Submit 4 disk reads simultaneously — all complete independently
    // Verify: 4 completions delivered, 4 interrupts raised, tags match
}

#[test]
fn test_io_thread_shutdown_clean() {
    let thread = IoThread::spawn(vec![], vec![]);
    thread.shutdown();  // must not hang
}
```
