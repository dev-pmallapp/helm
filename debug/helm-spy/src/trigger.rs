use std::sync::atomic::{AtomicBool, Ordering};

/// Conditions under which a trigger fires.
pub enum TriggerKind {
    /// Fire once when the global instruction count reaches exactly N.
    AtInsn(u64),
    /// Fire periodically -- every N instructions (checked via insn_count % N == 0).
    EveryN(u64),
    /// Fire when the instruction PC equals the given address (exact match).
    AtPc(u64),
    /// Fire while the instruction PC is inside [start, end).
    PcRange(u64, u64),
}

/// A trigger that tests a condition on each step and fires an action.
/// Hot-loop cost: one `AtomicBool::load(Relaxed)` + one comparison.
pub struct Trigger {
    kind: TriggerKind,
    action: Box<dyn Fn(u64, u64) + Send + Sync>, // (pc, insn_count)
    armed: AtomicBool,
    one_shot: bool,
}

impl Trigger {
    pub fn new(
        kind: TriggerKind,
        one_shot: bool,
        action: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            action: Box::new(action),
            armed: AtomicBool::new(true),
            one_shot,
        }
    }

    /// Check the trigger condition. Returns true if the trigger fired.
    #[inline]
    pub fn check(&self, pc: u64, insn_count: u64) -> bool {
        if !self.armed.load(Ordering::Relaxed) {
            return false;
        }

        let fired = match &self.kind {
            TriggerKind::AtInsn(n) => insn_count == *n,
            TriggerKind::EveryN(n) => *n > 0 && insn_count % n == 0,
            TriggerKind::AtPc(addr) => pc == *addr,
            TriggerKind::PcRange(s, e) => pc >= *s && pc < *e,
        };

        if fired {
            (self.action)(pc, insn_count);
            if self.one_shot {
                self.armed.store(false, Ordering::Relaxed);
            }
        }
        fired
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    pub fn arm(&self) {
        self.armed.store(true, Ordering::Relaxed);
    }

    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    #[test]
    fn trigger_at_insn_fires_at_n() {
        let fired_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fired_count);
        let t = Trigger::new(TriggerKind::AtInsn(100), false, move |_pc, _ic| {
            fc.fetch_add(1, Ordering::Relaxed);
        });

        // Should not fire before N
        assert!(!t.check(0x1000, 50));
        assert!(!t.check(0x1000, 99));
        assert_eq!(fired_count.load(Ordering::Relaxed), 0);

        // Should fire at N
        assert!(t.check(0x1000, 100));
        assert_eq!(fired_count.load(Ordering::Relaxed), 1);

        // Should not fire after N
        assert!(!t.check(0x1000, 101));
    }

    #[test]
    fn trigger_every_n() {
        let fired_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fired_count);
        let t = Trigger::new(TriggerKind::EveryN(10), false, move |_pc, _ic| {
            fc.fetch_add(1, Ordering::Relaxed);
        });

        for i in 0..=30 {
            t.check(0x1000, i);
        }
        // Should fire at 0, 10, 20, 30 = 4 times
        assert_eq!(fired_count.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn trigger_at_pc() {
        let fired_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fired_count);
        let t = Trigger::new(TriggerKind::AtPc(0x4000), false, move |_pc, _ic| {
            fc.fetch_add(1, Ordering::Relaxed);
        });

        assert!(!t.check(0x3FFF, 0));
        assert!(t.check(0x4000, 1));
        assert!(!t.check(0x4001, 2));
        assert!(t.check(0x4000, 3)); // fires again (not one-shot)
        assert_eq!(fired_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn trigger_pc_range() {
        let fired_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fired_count);
        let t = Trigger::new(
            TriggerKind::PcRange(0x1000, 0x2000),
            false,
            move |_pc, _ic| {
                fc.fetch_add(1, Ordering::Relaxed);
            },
        );

        assert!(!t.check(0x0FFF, 0)); // below range
        assert!(t.check(0x1000, 1));   // start (inclusive)
        assert!(t.check(0x1500, 2));   // middle
        assert!(t.check(0x1FFF, 3));   // just before end
        assert!(!t.check(0x2000, 4));  // end (exclusive)
        assert_eq!(fired_count.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn trigger_one_shot_disarms_after_fire() {
        let fired_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fired_count);
        let t = Trigger::new(TriggerKind::AtInsn(10), true, move |_pc, _ic| {
            fc.fetch_add(1, Ordering::Relaxed);
        });

        assert!(t.is_armed());
        assert!(t.check(0x1000, 10));
        assert!(!t.is_armed());
        assert!(!t.check(0x1000, 10)); // disarmed, won't fire
        assert_eq!(fired_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn trigger_disarmed_does_not_fire() {
        let t = Trigger::new(TriggerKind::AtPc(0x1000), false, |_pc, _ic| {});
        t.disarm();
        assert!(!t.check(0x1000, 0));
        t.arm();
        assert!(t.check(0x1000, 0));
    }

    #[test]
    fn trigger_every_n_zero_never_fires() {
        let fired_count = Arc::new(AtomicU64::new(0));
        let fc = Arc::clone(&fired_count);
        let t = Trigger::new(TriggerKind::EveryN(0), false, move |_pc, _ic| {
            fc.fetch_add(1, Ordering::Relaxed);
        });

        for i in 0..100 {
            t.check(0x1000, i);
        }
        assert_eq!(fired_count.load(Ordering::Relaxed), 0);
    }
}
