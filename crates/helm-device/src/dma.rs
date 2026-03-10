//! DMA engine — scatter-gather transfers that traverse the bus hierarchy.
//!
//! Inspired by gem5's DmaDevice: each beat of a transfer produces a
//! [`Transaction`] that flows through buses, accumulating stall cycles.

use helm_core::types::Addr;
use serde::{Deserialize, Serialize};

/// Direction of a DMA transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmaDirection {
    MemToDevice,
    DeviceToMem,
    MemToMem,
}

/// Status of a DMA channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmaStatus {
    Idle,
    Running,
    Complete,
    Error,
}

/// A single DMA channel with source, destination, and timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmaChannel {
    pub src_addr: Addr,
    pub dst_addr: Addr,
    pub length: u64,
    pub direction: DmaDirection,
    pub status: DmaStatus,
    /// Cycles per bus beat (bus-width transfer unit).
    pub stall_per_beat: u64,
    /// Bus beat size in bytes (typically bus width: 8 for 64-bit bus).
    pub beat_size: u64,
    /// Bytes transferred so far.
    pub bytes_transferred: u64,
}

impl DmaChannel {
    pub fn new(src: Addr, dst: Addr, length: u64, direction: DmaDirection) -> Self {
        Self {
            src_addr: src,
            dst_addr: dst,
            length,
            direction,
            status: DmaStatus::Idle,
            stall_per_beat: 1,
            beat_size: 8,
            bytes_transferred: 0,
        }
    }

    /// Total number of bus beats for this transfer.
    pub fn total_beats(&self) -> u64 {
        (self.length + self.beat_size - 1) / self.beat_size
    }

    /// Estimated total stall cycles for the complete transfer.
    pub fn estimated_cycles(&self) -> u64 {
        self.total_beats() * self.stall_per_beat
    }

    /// Whether the transfer is complete.
    pub fn is_complete(&self) -> bool {
        self.status == DmaStatus::Complete
    }

    /// Reset channel to idle.
    pub fn reset(&mut self) {
        self.status = DmaStatus::Idle;
        self.bytes_transferred = 0;
    }
}

/// Multi-channel DMA engine.
///
/// Each channel is independently programmable via MMIO registers.
/// The engine's `tick()` advances active channels by one beat per call,
/// emitting `DmaComplete` events when transfers finish.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmaEngine {
    channels: Vec<DmaChannel>,
}

impl DmaEngine {
    pub fn new(num_channels: usize) -> Self {
        Self {
            channels: (0..num_channels)
                .map(|_| DmaChannel {
                    src_addr: 0,
                    dst_addr: 0,
                    length: 0,
                    direction: DmaDirection::MemToMem,
                    status: DmaStatus::Idle,
                    stall_per_beat: 1,
                    beat_size: 8,
                    bytes_transferred: 0,
                })
                .collect(),
        }
    }

    /// Start a transfer on the given channel.
    pub fn start(&mut self, channel: usize, desc: DmaChannel) {
        if let Some(ch) = self.channels.get_mut(channel) {
            *ch = desc;
            ch.status = DmaStatus::Running;
        }
    }

    /// Advance all running channels by one beat. Returns indices of
    /// channels that completed during this tick.
    pub fn tick(&mut self) -> Vec<u32> {
        let mut completed = Vec::new();
        for (i, ch) in self.channels.iter_mut().enumerate() {
            if ch.status != DmaStatus::Running {
                continue;
            }
            ch.bytes_transferred = (ch.bytes_transferred + ch.beat_size).min(ch.length);
            if ch.bytes_transferred >= ch.length {
                ch.status = DmaStatus::Complete;
                completed.push(i as u32);
            }
        }
        completed
    }

    /// Get channel state.
    pub fn channel(&self, idx: usize) -> Option<&DmaChannel> {
        self.channels.get(idx)
    }

    /// Mutably access a channel.
    pub fn channel_mut(&mut self, idx: usize) -> Option<&mut DmaChannel> {
        self.channels.get_mut(idx)
    }

    /// Number of channels.
    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Reset all channels.
    pub fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.reset();
        }
    }
}
