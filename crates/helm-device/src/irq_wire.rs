//! IRQ wires — typed, connectable interrupt lines.
//!
//! [`IrqWire`] is a point-to-point interrupt wire that connects a device's
//! output to an [`IrqSink`] (typically a GIC or other interrupt controller).
//! Unlike [`IrqLine`](crate::irq::IrqLine), wires support runtime
//! connect/disconnect for hot-plug scenarios.

use std::sync::Arc;

/// Receives interrupt level changes.
///
/// Implement this on interrupt controllers (GIC, PIC, PLIC) to receive
/// interrupt assertions/de-assertions from devices via [`IrqWire`].
pub trait IrqSink: Send + Sync {
    /// Set the interrupt level on a specific input line.
    ///
    /// - `line`: the IRQ number on this sink (e.g. SPI number on a GIC)
    /// - `level`: `true` = asserted, `false` = de-asserted
    fn set_level(&self, line: u32, level: bool);
}

/// A point-to-point interrupt wire connecting a device output to an [`IrqSink`].
///
/// When no sink is connected, `set_level` is silently dropped.
pub struct IrqWire {
    sink: Option<Arc<dyn IrqSink>>,
    /// Which line on the sink this wire drives.
    line: u32,
}

impl IrqWire {
    /// Create an unconnected wire targeting `line` on a future sink.
    pub fn new(line: u32) -> Self {
        Self { sink: None, line }
    }

    /// Connect this wire to a sink.
    pub fn connect(&mut self, sink: Arc<dyn IrqSink>) {
        self.sink = Some(sink);
    }

    /// Disconnect from the current sink.
    pub fn disconnect(&mut self) {
        self.sink = None;
    }

    /// Whether a sink is connected.
    pub fn is_connected(&self) -> bool {
        self.sink.is_some()
    }

    /// Assert or de-assert the interrupt.
    ///
    /// If no sink is connected, the call is silently dropped.
    pub fn set_level(&self, level: bool) {
        if let Some(sink) = &self.sink {
            sink.set_level(self.line, level);
        }
    }

    /// The line number this wire targets on the sink.
    pub fn line(&self) -> u32 {
        self.line
    }
}

impl std::fmt::Debug for IrqWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrqWire")
            .field("line", &self.line)
            .field("connected", &self.sink.is_some())
            .finish()
    }
}
