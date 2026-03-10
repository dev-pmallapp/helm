//! TcgContext — builder for TCG op sequences within a translated block.

use super::ir::{TcgOp, TcgTemp};

/// Builder that emits TCG ops for a single translated block.
pub struct TcgContext {
    ops: Vec<TcgOp>,
    next_temp: u32,
    next_label: u32,
}

impl TcgContext {
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            next_temp: 0,
            next_label: 0,
        }
    }

    /// Allocate a new temporary register.
    pub fn temp(&mut self) -> TcgTemp {
        let t = TcgTemp(self.next_temp);
        self.next_temp += 1;
        t
    }

    /// Allocate a new label ID.
    pub fn label(&mut self) -> u32 {
        let l = self.next_label;
        self.next_label += 1;
        l
    }

    /// Emit a TCG op.
    pub fn emit(&mut self, op: TcgOp) {
        self.ops.push(op);
    }

    // -- Convenience helpers ------------------------------------------------

    pub fn movi(&mut self, value: u64) -> TcgTemp {
        let dst = self.temp();
        self.emit(TcgOp::Movi { dst, value });
        dst
    }

    pub fn read_reg(&mut self, reg_id: u16) -> TcgTemp {
        let dst = self.temp();
        self.emit(TcgOp::ReadReg { dst, reg_id });
        dst
    }

    pub fn write_reg(&mut self, reg_id: u16, src: TcgTemp) {
        self.emit(TcgOp::WriteReg { reg_id, src });
    }

    pub fn add(&mut self, a: TcgTemp, b: TcgTemp) -> TcgTemp {
        let dst = self.temp();
        self.emit(TcgOp::Add { dst, a, b });
        dst
    }

    pub fn addi(&mut self, a: TcgTemp, imm: i64) -> TcgTemp {
        let dst = self.temp();
        self.emit(TcgOp::Addi { dst, a, imm });
        dst
    }

    pub fn load(&mut self, addr: TcgTemp, size: u8) -> TcgTemp {
        let dst = self.temp();
        self.emit(TcgOp::Load { dst, addr, size });
        dst
    }

    pub fn store(&mut self, addr: TcgTemp, val: TcgTemp, size: u8) {
        self.emit(TcgOp::Store { addr, val, size });
    }

    /// Finalise and return the op list.
    /// Borrow the ops list (for inspection before finish).
    pub fn ops(&self) -> &[TcgOp] {
        &self.ops
    }

    pub fn finish(self) -> Vec<TcgOp> {
        self.ops
    }

    pub fn op_count(&self) -> usize {
        self.ops.len()
    }
}

impl Default for TcgContext {
    fn default() -> Self {
        Self::new()
    }
}
