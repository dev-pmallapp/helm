//! SP804 Dual Timer -- ARM DDI0271.
//!
//! Two independent 32-bit countdown timers with interrupt generation.
//! Each timer occupies 0x20 bytes of register space. Timer 1 is at
//! offset 0x00, Timer 2 is at offset 0x20.
//!
//! # Per-timer register map
//!
//! | Offset | Name       | R/W | Description                     |
//! |--------|------------|-----|---------------------------------|
//! | 0x00   | TimerLoad  | RW  | Load value (reload on wrap)     |
//! | 0x04   | TimerValue | R   | Current countdown value         |
//! | 0x08   | TimerControl| RW | Control register                |
//! | 0x0C   | TimerIntClr| W   | Interrupt clear (write-only)    |
//! | 0x10   | TimerRIS   | R   | Raw interrupt status            |
//! | 0x14   | TimerMIS   | R   | Masked interrupt status         |
//! | 0x18   | TimerBGLoad| RW  | Background load value           |

use helm_devices::{Device, InterruptPin, TickableDevice};

// ── Timer register offsets within each 0x20-byte block ──────────────────────

const TIMER_LOAD: u64 = 0x00;
const TIMER_VALUE: u64 = 0x04;
const TIMER_CONTROL: u64 = 0x08;
const TIMER_INTCLR: u64 = 0x0C;
const TIMER_RIS: u64 = 0x10;
const TIMER_MIS: u64 = 0x14;
const TIMER_BGLOAD: u64 = 0x18;

// ── Control register bits ───────────────────────────────────────────────────

const CTRL_ENABLE: u32 = 1 << 7;
const CTRL_PERIODIC: u32 = 1 << 6;
const CTRL_INTEN: u32 = 1 << 5;
#[allow(dead_code)]
const CTRL_PRESCALE_MASK: u32 = 3 << 2;
#[allow(dead_code)]
const CTRL_32BIT: u32 = 1 << 1;
const CTRL_ONESHOT: u32 = 1 << 0;

// ── Timer unit ──────────────────────────────────────────────────────────────

/// State for a single timer unit within the SP804.
#[derive(Debug, Clone)]
struct TimerUnit {
    load: u32,
    value: u32,
    control: u32,
    bg_load: u32,
    raw_irq: bool,
}

impl Default for TimerUnit {
    fn default() -> Self {
        Self {
            load: 0,
            value: 0xFFFF_FFFF,
            control: 0x20, // interrupt enabled by default
            bg_load: 0,
            raw_irq: false,
        }
    }
}

impl TimerUnit {
    fn enabled(&self) -> bool {
        self.control & CTRL_ENABLE != 0
    }

    fn periodic(&self) -> bool {
        self.control & CTRL_PERIODIC != 0
    }

    fn oneshot(&self) -> bool {
        self.control & CTRL_ONESHOT != 0
    }

    fn irq_enabled(&self) -> bool {
        self.control & CTRL_INTEN != 0
    }

    fn prescale_shift(&self) -> u32 {
        match (self.control >> 2) & 3 {
            1 => 4, // divide by 16
            2 => 8, // divide by 256
            _ => 0, // no prescale
        }
    }

    /// Tick the timer by `cycles`. Returns `true` if the timer fired
    /// (value reached zero).
    fn tick(&mut self, cycles: u64) -> bool {
        if !self.enabled() {
            return false;
        }

        let shift = self.prescale_shift();
        let decrements = (cycles >> shift) as u32;
        if decrements == 0 {
            return false;
        }

        let mut fired = false;

        if self.value <= decrements {
            // Timer reached zero
            self.raw_irq = true;
            fired = true;

            if self.periodic() {
                // Reload from load register
                let reload = if self.bg_load != 0 {
                    self.load = self.bg_load;
                    self.bg_load
                } else {
                    self.load
                };
                let remaining = decrements - self.value;
                if reload > 0 {
                    self.value = reload.wrapping_sub(remaining % reload);
                } else {
                    self.value = 0;
                }
            } else if self.oneshot() {
                // Stop after firing
                self.value = 0;
                self.control &= !CTRL_ENABLE;
            } else {
                // Free-running: wrap around
                self.value = 0xFFFF_FFFF_u32.wrapping_sub(decrements - self.value - 1);
            }
        } else {
            self.value -= decrements;
        }

        fired
    }

    fn read_reg(&self, reg_offset: u64) -> u32 {
        match reg_offset {
            TIMER_LOAD => self.load,
            TIMER_VALUE => self.value,
            TIMER_CONTROL => self.control,
            TIMER_RIS => self.raw_irq as u32,
            TIMER_MIS => {
                if self.irq_enabled() && self.raw_irq {
                    1
                } else {
                    0
                }
            }
            TIMER_BGLOAD => self.bg_load,
            _ => 0,
        }
    }

    fn write_reg(&mut self, reg_offset: u64, val: u32) {
        match reg_offset {
            TIMER_LOAD => {
                self.load = val;
                self.value = val;
            }
            TIMER_CONTROL => {
                self.control = val;
            }
            TIMER_INTCLR => {
                self.raw_irq = false;
            }
            TIMER_BGLOAD => {
                self.bg_load = val;
            }
            // TIMER_VALUE, TIMER_RIS, TIMER_MIS are read-only
            _ => {}
        }
    }
}

// ── Sp804 ───────────────────────────────────────────────────────────────────

/// SP804 Dual Timer device.
///
/// Contains two independent countdown timers. Each can be configured for
/// free-running, periodic, or one-shot mode. The device exposes a single
/// combined interrupt pin.
pub struct Sp804 {
    timers: [TimerUnit; 2],
    /// Combined interrupt output pin (OR of both timer interrupts).
    pub irq_out: InterruptPin,
}

impl Sp804 {
    /// Create a new SP804 dual timer.
    pub fn new() -> Self {
        Self {
            timers: [TimerUnit::default(), TimerUnit::default()],
            irq_out: InterruptPin::new(),
        }
    }

    /// Advance both timers by `cycles` ticks. Call this from the engine's
    /// tick loop or event scheduler.
    pub fn tick(&mut self, cycles: u64) {
        let fired0 = self.timers[0].tick(cycles);
        let fired1 = self.timers[1].tick(cycles);

        // Update combined IRQ
        let any_pending = (self.timers[0].irq_enabled() && self.timers[0].raw_irq)
            || (self.timers[1].irq_enabled() && self.timers[1].raw_irq);

        if any_pending && (fired0 || fired1) {
            self.irq_out.assert();
        } else if !any_pending {
            self.irq_out.deassert();
        }
    }
}

impl TickableDevice for Sp804 {
    fn tick(&mut self, cycles: u64) {
        Sp804::tick(self, cycles);
    }
}

impl Default for Sp804 {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for Sp804 {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let (timer_idx, reg_offset) = if offset < 0x20 {
            (0, offset)
        } else if offset < 0x40 {
            (1, offset - 0x20)
        } else {
            return 0;
        };
        self.timers[timer_idx].read_reg(reg_offset) as u64
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let (timer_idx, reg_offset) = if offset < 0x20 {
            (0, offset)
        } else if offset < 0x40 {
            (1, offset - 0x20)
        } else {
            return;
        };

        self.timers[timer_idx].write_reg(reg_offset, val as u32);

        // If interrupt was cleared, check combined IRQ state
        if reg_offset == TIMER_INTCLR {
            let any_pending = (self.timers[0].irq_enabled() && self.timers[0].raw_irq)
                || (self.timers[1].irq_enabled() && self.timers[1].raw_irq);
            if !any_pending {
                self.irq_out.deassert();
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x1000 // 4 KiB (standard ARM peripheral page)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_load_and_read_value() {
        let mut timer = Sp804::new();

        // Load timer 0 with value 100
        timer.write(TIMER_LOAD, 4, 100);
        assert_eq!(timer.read(TIMER_VALUE, 4), 100);
    }

    #[test]
    fn timer_countdown() {
        let mut timer = Sp804::new();

        // Load and enable timer 0 (periodic, interrupt enabled)
        timer.write(TIMER_LOAD, 4, 100);
        timer.write(
            TIMER_CONTROL,
            4,
            (CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN) as u64,
        );

        // Tick 50 cycles
        timer.tick(50);
        assert_eq!(timer.read(TIMER_VALUE, 4), 50);

        // No interrupt yet
        assert_eq!(timer.read(TIMER_RIS, 4), 0);
    }

    #[test]
    fn timer_fires_at_zero() {
        let mut timer = Sp804::new();

        timer.write(TIMER_LOAD, 4, 10);
        timer.write(
            TIMER_CONTROL,
            4,
            (CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN) as u64,
        );

        // Tick past zero
        timer.tick(15);

        // Raw interrupt should be set
        assert_eq!(timer.read(TIMER_RIS, 4), 1);
        // Masked interrupt should also be set (inten is on)
        assert_eq!(timer.read(TIMER_MIS, 4), 1);
    }

    #[test]
    fn timer_periodic_reload() {
        let mut timer = Sp804::new();

        timer.write(TIMER_LOAD, 4, 10);
        timer.write(
            TIMER_CONTROL,
            4,
            (CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN) as u64,
        );

        // Tick 10 -- should fire and reload
        timer.tick(10);
        assert_eq!(timer.read(TIMER_RIS, 4), 1);

        // Clear interrupt
        timer.write(TIMER_INTCLR, 4, 1);
        assert_eq!(timer.read(TIMER_RIS, 4), 0);

        // Timer should have reloaded to 10
        let val = timer.read(TIMER_VALUE, 4);
        assert!(val <= 10, "timer should have reloaded, got {val}");
    }

    #[test]
    fn timer_oneshot_stops_after_fire() {
        let mut timer = Sp804::new();

        timer.write(TIMER_LOAD, 4, 5);
        timer.write(
            TIMER_CONTROL,
            4,
            (CTRL_ENABLE | CTRL_ONESHOT | CTRL_INTEN) as u64,
        );

        timer.tick(10);

        // Should have fired
        assert_eq!(timer.read(TIMER_RIS, 4), 1);

        // Value should be 0 and timer disabled
        assert_eq!(timer.read(TIMER_VALUE, 4), 0);

        // Clear interrupt and tick again -- should not fire again
        timer.write(TIMER_INTCLR, 4, 1);
        timer.tick(100);
        assert_eq!(timer.read(TIMER_RIS, 4), 0);
    }

    #[test]
    fn timer2_independent() {
        let mut timer = Sp804::new();

        // Timer 1 at offset 0x00
        timer.write(0x00, 4, 50); // Timer1 Load
        timer.write(0x08, 4, (CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN) as u64);

        // Timer 2 at offset 0x20
        timer.write(0x20, 4, 100); // Timer2 Load
        timer.write(0x28, 4, (CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN) as u64);

        timer.tick(50);

        // Timer 1 should have fired
        assert_eq!(timer.read(0x10, 4), 1); // Timer1 RIS

        // Timer 2 should still be counting
        assert_eq!(timer.read(0x30, 4), 0); // Timer2 RIS
        assert_eq!(timer.read(0x24, 4), 50); // Timer2 Value
    }

    #[test]
    fn disabled_timer_does_not_count() {
        let mut timer = Sp804::new();

        timer.write(TIMER_LOAD, 4, 10);
        // Do NOT set CTRL_ENABLE
        timer.write(TIMER_CONTROL, 4, CTRL_INTEN as u64);

        timer.tick(100);

        // Value should be unchanged (load writes to value too)
        assert_eq!(timer.read(TIMER_VALUE, 4), 10);
        assert_eq!(timer.read(TIMER_RIS, 4), 0);
    }

    #[test]
    fn region_size() {
        let timer = Sp804::new();
        assert_eq!(timer.region_size(), 0x1000);
    }
}
