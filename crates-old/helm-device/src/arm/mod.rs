//! ARM-specific device models.
//!
//! Covers both generic ARM IP (PL011, SP804, GIC, etc.) and
//! SoC-specific peripherals (BCM2837 for RPi-3).

// ── Generic ARM IP ──────────────────────────────────────────────────────────
pub mod gic;
pub mod pl011;
pub mod pl031;
pub mod pl061;
pub mod sp804;
pub mod sp805;
pub mod sysregs;

// ── BCM2837 (Raspberry Pi 3) ────────────────────────────────────────────────
pub mod bcm_gpio;
pub mod bcm_mailbox;
pub mod bcm_mini_uart;
pub mod bcm_sys_timer;
