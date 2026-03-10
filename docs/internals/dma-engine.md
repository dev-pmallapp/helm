# DMA Engine

Scatter-gather DMA with bus-beat fragmentation.

## DmaChannel

Each channel tracks:
- `src_addr` / `dst_addr` — source and destination.
- `length` — total bytes.
- `direction` — `MemToDevice`, `DeviceToMem`, or `MemToMem`.
- `status` — `Idle`, `Running`, `Complete`, or `Error`.
- `stall_per_beat` / `beat_size` — timing parameters.
- `bytes_transferred` — progress counter.

## Transfer Model

Transfers are broken into beats of `beat_size` bytes (default 8 for a
64-bit bus). Each beat produces a `Transaction` that flows through the
bus hierarchy, accumulating stall cycles from each bridge level.

`total_beats() = (length + beat_size - 1) / beat_size`

## DmaEngine

Manages multiple `DmaChannel`s. Devices emit `DeviceEvent::DmaComplete`
when a channel finishes.
