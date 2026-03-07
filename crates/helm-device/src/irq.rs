//! Interrupt request (IRQ) system.
//!
//! Two layers:
//! - **IrqLine / IrqController**: simple line tracking (backward compat)
//! - **IrqRouter / InterruptController**: routable, serializable IRQ delivery

use crate::device::{Device, DeviceId};

// ── Basic IRQ types (backward compatible) ───────────────────────────────────

/// State of an interrupt line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqState {
    Low,
    High,
}

/// A single interrupt line that a device can assert or de-assert.
#[derive(Debug, Clone)]
pub struct IrqLine {
    pub id: u32,
    pub state: IrqState,
    pub name: String,
}

impl IrqLine {
    pub fn new(id: u32, name: impl Into<String>) -> Self {
        Self {
            id,
            state: IrqState::Low,
            name: name.into(),
        }
    }

    pub fn assert(&mut self) {
        self.state = IrqState::High;
    }

    pub fn deassert(&mut self) {
        self.state = IrqState::Low;
    }

    pub fn is_asserted(&self) -> bool {
        self.state == IrqState::High
    }
}

/// Simple interrupt controller that tracks pending IRQs.
pub struct IrqController {
    lines: Vec<IrqLine>,
}

impl IrqController {
    pub fn new(num_lines: u32) -> Self {
        let lines = (0..num_lines)
            .map(|i| IrqLine::new(i, format!("irq{i}")))
            .collect();
        Self { lines }
    }

    pub fn set(&mut self, id: u32, state: IrqState) {
        if let Some(line) = self.lines.get_mut(id as usize) {
            line.state = state;
        }
    }

    pub fn assert(&mut self, id: u32) {
        self.set(id, IrqState::High);
    }

    pub fn deassert(&mut self, id: u32) {
        self.set(id, IrqState::Low);
    }

    /// Returns the IDs of all currently asserted interrupt lines.
    pub fn pending(&self) -> Vec<u32> {
        self.lines
            .iter()
            .filter(|l| l.is_asserted())
            .map(|l| l.id)
            .collect()
    }

    pub fn has_pending(&self) -> bool {
        self.lines.iter().any(|l| l.is_asserted())
    }
}

impl Default for IrqController {
    fn default() -> Self {
        Self::new(64)
    }
}

// ── IRQ routing (new) ───────────────────────────────────────────────────────

/// A route connecting a device's IRQ output to an interrupt controller input.
#[derive(Debug, Clone)]
pub struct IrqRoute {
    /// The device that fires the interrupt.
    pub source_device: DeviceId,
    /// Which output line on the source device.
    pub source_line: u32,
    /// Index into the router's controller list.
    pub dest_controller: usize,
    /// Which IRQ number on the destination controller.
    pub dest_irq: u32,
}

/// Trait for interrupt controllers (GIC, PIC, PLIC, etc.).
///
/// An interrupt controller is also a [`Device`] (it has MMIO registers).
pub trait InterruptController: Device {
    /// Inject an interrupt level change.
    fn inject(&mut self, irq: u32, level: bool);

    /// Check if any interrupt is pending for the given CPU.
    fn pending_for_cpu(&self, cpu_id: u32) -> bool;

    /// Acknowledge the highest-priority pending interrupt for a CPU.
    /// Returns the IRQ number if one was pending.
    fn ack(&mut self, cpu_id: u32) -> Option<u32>;
}

/// Routes device IRQ events to interrupt controllers.
///
/// Wire-up is declarative: routes are configured at platform creation time,
/// then the engine calls `deliver()` when a device emits an `Irq` event.
pub struct IrqRouter {
    routes: Vec<IrqRoute>,
    controllers: Vec<Box<dyn InterruptController>>,
}

impl IrqRouter {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            controllers: Vec::new(),
        }
    }

    /// Register an interrupt controller. Returns its index.
    pub fn add_controller(&mut self, ctrl: Box<dyn InterruptController>) -> usize {
        let idx = self.controllers.len();
        self.controllers.push(ctrl);
        idx
    }

    /// Add a routing entry.
    pub fn add_route(&mut self, route: IrqRoute) {
        self.routes.push(route);
    }

    /// Deliver an IRQ event from a device to the appropriate controller(s).
    pub fn deliver(&mut self, source_device: DeviceId, source_line: u32, level: bool) {
        let matches: Vec<(usize, u32)> = self
            .routes
            .iter()
            .filter(|r| r.source_device == source_device && r.source_line == source_line)
            .map(|r| (r.dest_controller, r.dest_irq))
            .collect();

        for (ctrl_idx, irq) in matches {
            if let Some(ctrl) = self.controllers.get_mut(ctrl_idx) {
                ctrl.inject(irq, level);
            }
        }
    }

    /// Check if any controller has a pending interrupt for the given CPU.
    pub fn has_pending(&self, cpu_id: u32) -> bool {
        self.controllers.iter().any(|c| c.pending_for_cpu(cpu_id))
    }

    /// Acknowledge the highest-priority pending interrupt across all controllers.
    pub fn ack(&mut self, cpu_id: u32) -> Option<u32> {
        for ctrl in &mut self.controllers {
            if let Some(irq) = ctrl.ack(cpu_id) {
                return Some(irq);
            }
        }
        None
    }

    /// Access a controller by index.
    pub fn controller(&self, idx: usize) -> Option<&dyn InterruptController> {
        self.controllers.get(idx).map(|c| c.as_ref())
    }

    /// Mutably access a controller by index.
    pub fn controller_mut(&mut self, idx: usize) -> Option<&mut Box<dyn InterruptController>> {
        self.controllers.get_mut(idx)
    }

    /// All configured routes.
    pub fn routes(&self) -> &[IrqRoute] {
        &self.routes
    }
}

impl Default for IrqRouter {
    fn default() -> Self {
        Self::new()
    }
}
