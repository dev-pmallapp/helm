use crate::event::*;

struct CountingObserver {
    count: u64,
}

impl EventObserver for CountingObserver {
    fn on_event(&mut self, _event: &SimEvent) {
        self.count += 1;
    }
}

#[test]
fn observer_receives_events() {
    let mut obs = CountingObserver { count: 0 };
    let event = SimEvent::InsnCommit {
        pc: 0x1000,
        cycle: 1,
    };
    obs.on_event(&event);
    obs.on_event(&event);
    assert_eq!(obs.count, 2);
}
