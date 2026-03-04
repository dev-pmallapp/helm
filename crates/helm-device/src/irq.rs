//! Interrupt request (IRQ) system.

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
