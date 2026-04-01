//! GDB target trait — interface between GDB stub and simulation engine.

/// Interface that the simulation engine implements for GDB control.
pub trait GdbTarget {
    fn read_register(&self, reg_num: usize) -> Option<u64>;
    fn write_register(&mut self, reg_num: usize, val: u64) -> bool;
    fn read_memory(&self, addr: u64, len: usize) -> Option<Vec<u8>>;
    fn write_memory(&mut self, addr: u64, data: &[u8]) -> bool;
    fn step(&mut self) -> u64;
    fn continue_exec(&mut self) -> StopReason;
    fn set_breakpoint(&mut self, addr: u64) -> bool;
    fn remove_breakpoint(&mut self, addr: u64) -> bool;
    fn num_registers(&self) -> usize;
    fn read_pc(&self) -> u64;
}

/// Reason the target stopped.
#[derive(Debug, Clone)]
pub enum StopReason {
    Breakpoint(u64),
    Step,
    Exited(i32),
    Signal(u8),
}
