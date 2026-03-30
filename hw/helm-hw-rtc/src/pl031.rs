//! PL031 Real-Time Clock -- ARM PrimeCell RTC (PL031).
//!
//! A simple 32-bit counter that increments once per second (in
//! simulation ticks). Supports an alarm match register that generates
//! an interrupt when the counter matches the alarm value.
//!
//! # Register map
//!
//! | Offset | Name   | R/W | Description                    |
//! |--------|--------|-----|--------------------------------|
//! | 0x000  | RTCDR  | R   | Data register (current count)  |
//! | 0x004  | RTCMR  | RW  | Match register (alarm value)   |
//! | 0x008  | RTCLR  | RW  | Load register (set counter)    |
//! | 0x00C  | RTCCR  | RW  | Control register (bit0=enable) |
//! | 0x010  | RTCIMSC| RW  | Interrupt mask set/clear       |
//! | 0x014  | RTCRIS | R   | Raw interrupt status           |
//! | 0x018  | RTCMIS | R   | Masked interrupt status        |
//! | 0x01C  | RTCICR | W   | Interrupt clear register       |
//! | 0xFE0  | PeriphID0-3 | R | Peripheral identification   |
//! | 0xFF0  | CellID0-3   | R | PrimeCell identification    |

use helm_devices::{Device, InterruptPin, TickableDevice};

// ── Register offsets ────────────────────────────────────────────────────────

const RTCDR: u64 = 0x000;
const RTCMR: u64 = 0x004;
const RTCLR: u64 = 0x008;
const RTCCR: u64 = 0x00C;
const RTCIMSC: u64 = 0x010;
const RTCRIS: u64 = 0x014;
const RTCMIS: u64 = 0x018;
const RTCICR: u64 = 0x01C;

// Identification registers
const PERIPH_ID0: u64 = 0xFE0;
const PERIPH_ID1: u64 = 0xFE4;
const PERIPH_ID2: u64 = 0xFE8;
const PERIPH_ID3: u64 = 0xFEC;
const CELL_ID0: u64 = 0xFF0;
const CELL_ID1: u64 = 0xFF4;
const CELL_ID2: u64 = 0xFF8;
const CELL_ID3: u64 = 0xFFC;

// ── Pl031 ───────────────────────────────────────────────────────────────────

/// PL031 Real-Time Clock device.
///
/// The counter increments once per [`tick()`](Pl031::tick) call. The
/// simulation engine should call `tick()` at the rate corresponding to
/// one second of simulated time.
pub struct Pl031 {
    /// Current counter value (seconds since epoch).
    counter: u32,
    /// Match (alarm) register.
    match_reg: u32,
    /// Load register (written to set counter).
    load: u32,
    /// Control register: bit 0 = RTC enable.
    control: u32,
    /// Interrupt mask.
    imsc: u32,
    /// Raw interrupt status (bit 0 = alarm matched).
    ris: u32,
    /// Interrupt output pin.
    pub irq_out: InterruptPin,
}

impl Pl031 {
    /// Create a new PL031 RTC with the counter starting at `initial_time`.
    pub fn new(initial_time: u32) -> Self {
        Self {
            counter: initial_time,
            match_reg: 0,
            load: initial_time,
            control: 1, // enabled by default
            imsc: 0,
            ris: 0,
            irq_out: InterruptPin::new(),
        }
    }

    /// Advance the RTC by one second. Call this from the engine's event
    /// scheduler at 1 Hz simulated frequency.
    pub fn tick(&mut self) {
        if self.control & 1 == 0 {
            return; // disabled
        }
        self.counter = self.counter.wrapping_add(1);

        // Check alarm match
        if self.counter == self.match_reg {
            self.ris |= 1;
            self.update_irq();
        }
    }

    fn update_irq(&mut self) {
        if (self.ris & self.imsc) != 0 {
            self.irq_out.assert();
        } else {
            self.irq_out.deassert();
        }
    }
}

impl TickableDevice for Pl031 {
    fn tick(&mut self, cycles: u64) {
        for _ in 0..cycles {
            Pl031::tick(self);
        }
    }
}

impl Device for Pl031 {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let val = match offset {
            RTCDR => self.counter,
            RTCMR => self.match_reg,
            RTCLR => self.load,
            RTCCR => self.control,
            RTCIMSC => self.imsc,
            RTCRIS => self.ris,
            RTCMIS => self.ris & self.imsc,
            // PL031 identification
            PERIPH_ID0 => 0x31,
            PERIPH_ID1 => 0x10,
            PERIPH_ID2 => 0x04,
            PERIPH_ID3 => 0x00,
            CELL_ID0 => 0x0D,
            CELL_ID1 => 0xF0,
            CELL_ID2 => 0x05,
            CELL_ID3 => 0xB1,
            _ => 0,
        };
        val as u64
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        match offset {
            RTCMR => self.match_reg = val32,
            RTCLR => {
                self.load = val32;
                self.counter = val32;
            }
            RTCCR => self.control = val32 & 1,
            RTCIMSC => {
                self.imsc = val32 & 1;
                self.update_irq();
            }
            RTCICR => {
                self.ris &= !val32;
                self.update_irq();
            }
            // RTCDR, RTCRIS, RTCMIS are read-only
            _ => {}
        }
    }

    fn region_size(&self) -> u64 {
        0x1000 // 4 KiB
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_counter_value() {
        let mut rtc = Pl031::new(1_000_000);
        assert_eq!(rtc.read(RTCDR, 4), 1_000_000);
    }

    #[test]
    fn counter_increments_on_tick() {
        let mut rtc = Pl031::new(100);
        rtc.tick();
        assert_eq!(rtc.read(RTCDR, 4), 101);
        rtc.tick();
        assert_eq!(rtc.read(RTCDR, 4), 102);
    }

    #[test]
    fn load_register_sets_counter() {
        let mut rtc = Pl031::new(0);
        rtc.write(RTCLR, 4, 500);
        assert_eq!(rtc.read(RTCDR, 4), 500);
    }

    #[test]
    fn alarm_match_fires_interrupt() {
        let mut rtc = Pl031::new(99);

        // Set alarm at 100
        rtc.write(RTCMR, 4, 100);

        // Enable interrupt mask
        rtc.write(RTCIMSC, 4, 1);

        // Tick to 100
        rtc.tick();

        // Raw interrupt should be set
        assert_eq!(rtc.read(RTCRIS, 4), 1);
        // Masked interrupt should also be set
        assert_eq!(rtc.read(RTCMIS, 4), 1);
    }

    #[test]
    fn alarm_no_interrupt_when_masked() {
        let mut rtc = Pl031::new(99);
        rtc.write(RTCMR, 4, 100);
        // Mask disabled (default)

        rtc.tick();

        // Raw interrupt set
        assert_eq!(rtc.read(RTCRIS, 4), 1);
        // Masked interrupt not set
        assert_eq!(rtc.read(RTCMIS, 4), 0);
    }

    #[test]
    fn interrupt_clear() {
        let mut rtc = Pl031::new(99);
        rtc.write(RTCMR, 4, 100);
        rtc.write(RTCIMSC, 4, 1);
        rtc.tick();

        assert_eq!(rtc.read(RTCRIS, 4), 1);

        // Clear interrupt
        rtc.write(RTCICR, 4, 1);
        assert_eq!(rtc.read(RTCRIS, 4), 0);
        assert_eq!(rtc.read(RTCMIS, 4), 0);
    }

    #[test]
    fn disabled_rtc_does_not_count() {
        let mut rtc = Pl031::new(50);
        rtc.write(RTCCR, 4, 0); // disable

        rtc.tick();
        rtc.tick();

        assert_eq!(rtc.read(RTCDR, 4), 50);
    }

    #[test]
    fn identification_registers() {
        let mut rtc = Pl031::new(0);
        assert_eq!(rtc.read(PERIPH_ID0, 4), 0x31);
        assert_eq!(rtc.read(CELL_ID0, 4), 0x0D);
    }

    #[test]
    fn region_size() {
        let rtc = Pl031::new(0);
        assert_eq!(rtc.region_size(), 0x1000);
    }
}
