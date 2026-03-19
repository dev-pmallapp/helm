use helm_probe::{CpuProbes, GicProbes, Probe};

#[test]
fn new_probe_has_no_listeners() {
    let p: Probe<u64> = Probe::new();
    assert!(!p.has_listeners());
}

#[test]
fn default_equals_new() {
    let p: Probe<u64> = Probe::default();
    assert!(!p.has_listeners());
}

#[test]
fn notify_no_listeners_no_panic() {
    let p: Probe<u64> = Probe::new();
    p.notify(&42);
}

#[test]
fn probe_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Probe<u64>>();
    assert_send_sync::<Probe<helm_probe::CpuStepEvent>>();
}

#[test]
fn cpu_probes_default() {
    let probes = CpuProbes::default();
    assert!(!probes.pre_step.has_listeners());
    assert!(!probes.post_step.has_listeners());
    assert!(!probes.fault.has_listeners());
    assert!(!probes.mem.has_listeners());
    assert!(!probes.branch.has_listeners());
}

#[test]
fn gic_probes_default() {
    let probes = GicProbes::default();
    assert!(!probes.irq_asserted.has_listeners());
    assert!(!probes.irq_deasserted.has_listeners());
    assert!(!probes.eoi.has_listeners());
}

#[cfg(debug_assertions)]
mod debug_only {
    use helm_probe::Probe;
    use std::sync::{Arc, Mutex};

    #[test]
    fn subscribe_enables_has_listeners() {
        let mut p: Probe<u64> = Probe::new();
        assert!(!p.has_listeners());
        p.subscribe(|_| {});
        assert!(p.has_listeners());
    }

    #[test]
    fn listener_count_increments() {
        let mut p: Probe<u64> = Probe::new();
        assert_eq!(p.listener_count(), 0);
        p.subscribe(|_| {});
        assert_eq!(p.listener_count(), 1);
        p.subscribe(|_| {});
        assert_eq!(p.listener_count(), 2);
    }

    #[test]
    fn notify_delivers_to_subscriber() {
        let mut p: Probe<u64> = Probe::new();
        let log: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        p.subscribe(move |val: &u64| {
            log2.lock().unwrap().push(*val);
        });
        p.notify(&42);
        p.notify(&99);
        let v = log.lock().unwrap();
        assert_eq!(&*v, &[42, 99]);
    }

    #[test]
    fn multiple_listeners_all_receive() {
        let mut p: Probe<u64> = Probe::new();
        let log_a: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let log_b: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let la = log_a.clone();
        let lb = log_b.clone();
        p.subscribe(move |val: &u64| la.lock().unwrap().push(*val));
        p.subscribe(move |val: &u64| lb.lock().unwrap().push(*val));
        p.notify(&7);
        assert_eq!(log_a.lock().unwrap().len(), 1);
        assert_eq!(log_b.lock().unwrap().len(), 1);
        assert_eq!(log_a.lock().unwrap()[0], 7);
        assert_eq!(log_b.lock().unwrap()[0], 7);
    }

    #[test]
    fn listeners_fire_in_order() {
        let mut p: Probe<u64> = Probe::new();
        let order: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let o1 = order.clone();
        let o2 = order.clone();
        let o3 = order.clone();
        p.subscribe(move |_| o1.lock().unwrap().push(1));
        p.subscribe(move |_| o2.lock().unwrap().push(2));
        p.subscribe(move |_| o3.lock().unwrap().push(3));
        p.notify(&0);
        assert_eq!(&*order.lock().unwrap(), &[1u8, 2u8, 3u8]);
    }
}
