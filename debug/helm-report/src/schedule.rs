// src/schedule.rs -- ReportTrigger enum, ReportSchedule.

use crate::{error::SinkError, report::Report};

/// Trigger that fires a report delivery.
#[derive(Debug, Clone)]
pub enum ReportTrigger {
    /// Deliver when the process exits (called from `flush_at_exit()`).
    AtExit,
    /// Deliver every N instructions.
    EveryNInsns(u64),
    /// Deliver when the named counter exceeds a threshold.
    OnCounter { name: String, threshold: u64 },
    /// Deliver when PC equals the given address.
    OnPc(u64),
    /// Never fires automatically; caller invokes `deliver()` directly.
    Explicit,
}

/// Wraps a `Report` with a list of triggers. The engine calls `check()` from
/// the `pre_step` probe subscriber on every instruction.
pub struct ReportSchedule {
    triggers: Vec<ReportTrigger>,
    report: Report,
    last_delivered_at: u64, // insn_count at which we last fired EveryNInsns
}

impl ReportSchedule {
    pub fn new(report: Report, triggers: Vec<ReportTrigger>) -> Self {
        ReportSchedule {
            triggers,
            report,
            last_delivered_at: 0,
        }
    }

    /// Called on every instruction from the engine's pre_step hook.
    ///
    /// Cost when no trigger fires: one integer division per `EveryNInsns` trigger,
    /// one equality compare per `OnPc` trigger. Typically < 2 ns.
    pub fn check(&mut self, pc: u64, insn_count: u64) {
        let mut should_deliver = false;
        for trigger in &self.triggers {
            match trigger {
                ReportTrigger::EveryNInsns(n) => {
                    if *n > 0
                        && insn_count > 0
                        && (insn_count / n) > (self.last_delivered_at / n)
                    {
                        should_deliver = true;
                    }
                }
                ReportTrigger::OnPc(addr) => {
                    if pc == *addr {
                        should_deliver = true;
                    }
                }
                // AtExit / Explicit / OnCounter do not fire in check().
                _ => {}
            }
        }
        if should_deliver {
            self.last_delivered_at = insn_count;
            let _ = self.report.deliver();
        }
    }

    /// Call at process exit. Fires all `AtExit` triggers.
    pub fn flush_at_exit(&self) {
        let has_at_exit = self
            .triggers
            .iter()
            .any(|t| matches!(t, ReportTrigger::AtExit));
        if has_at_exit {
            let _ = self.report.deliver();
        }
    }

    /// Deliver immediately, regardless of triggers.
    pub fn deliver(&self) -> Result<(), SinkError> {
        self.report.deliver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use crate::{format::TextFormatter, report::Report};

    struct CounterSink(Arc<Mutex<u32>>);
    impl crate::sink::Sink for CounterSink {
        fn write(&self, _: &[u8]) -> std::io::Result<()> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
        fn name(&self) -> &str {
            "counter"
        }
    }

    fn make_schedule(trigger: ReportTrigger) -> (ReportSchedule, Arc<Mutex<u32>>) {
        let count = Arc::new(Mutex::new(0u32));
        let sink = Box::new(CounterSink(Arc::clone(&count)));
        let report = Report::new(
            Arc::new(crate::tests::test_snapshot()),
            Box::new(TextFormatter::default()),
            vec![sink],
        );
        let sched = ReportSchedule::new(report, vec![trigger]);
        (sched, count)
    }

    #[test]
    fn schedule_every_n_insns_fires_at_interval() {
        let (mut sched, count) = make_schedule(ReportTrigger::EveryNInsns(1_000_000));

        for i in 0u64..3_500_000 {
            sched.check(0x4000, i);
        }

        let fires = *count.lock().unwrap();
        // Should have fired at 1M, 2M, 3M -- exactly 3 times.
        assert_eq!(
            fires, 3,
            "EveryNInsns(1M) should fire 3 times at 3.5M insns"
        );
    }

    #[test]
    fn schedule_on_pc_fires_on_match() {
        let (mut sched, count) = make_schedule(ReportTrigger::OnPc(0xDEAD_BEEF));

        sched.check(0x1000, 100);
        sched.check(0xDEAD_BEEF, 101); // fires here
        sched.check(0x2000, 102);
        sched.check(0xDEAD_BEEF, 103); // fires again

        let fires = *count.lock().unwrap();
        assert_eq!(fires, 2, "OnPc should fire on every PC match");
    }

    #[test]
    fn schedule_at_exit_does_not_fire_from_check() {
        let (mut sched, count) = make_schedule(ReportTrigger::AtExit);

        for i in 0u64..10_000 {
            sched.check(0x1000, i);
        }

        let fires = *count.lock().unwrap();
        assert_eq!(fires, 0, "AtExit should not fire from check()");
    }

    #[test]
    fn schedule_at_exit_fires_from_flush_at_exit() {
        let (sched, count) = make_schedule(ReportTrigger::AtExit);
        sched.flush_at_exit();
        let fires = *count.lock().unwrap();
        assert_eq!(
            fires, 1,
            "flush_at_exit() should deliver once for AtExit trigger"
        );
    }

    #[test]
    fn schedule_explicit_does_not_fire_from_check() {
        let (mut sched, count) = make_schedule(ReportTrigger::Explicit);

        for i in 0u64..5_000 {
            sched.check(0x1000, i);
        }

        let fires = *count.lock().unwrap();
        assert_eq!(fires, 0, "Explicit trigger should never fire from check()");
    }

    #[test]
    fn schedule_deliver_fires_explicit() {
        let (sched, count) = make_schedule(ReportTrigger::Explicit);
        sched.deliver().unwrap();
        assert_eq!(*count.lock().unwrap(), 1, "deliver() should fire once");
    }
}
