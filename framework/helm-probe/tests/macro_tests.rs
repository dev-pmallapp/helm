#![allow(missing_docs)]

use helm_probe::{probe, Probe};

#[test]
fn macro_skips_eval_no_listeners() {
    let p: Probe<u32> = Probe::new();
    let mut evaluated = false;
    probe!(p, {
        evaluated = true;
        0u32
    });
    assert!(
        !evaluated,
        "event expression must not be evaluated without listeners"
    );
}

#[cfg(feature = "instrumentation")]
mod debug_only {
    use helm_probe::{probe, CpuStepEvent, Probe};
    use std::sync::{Arc, Mutex};

    #[test]
    fn macro_delivers_when_subscribed() {
        let mut p: Probe<u32> = Probe::new();
        let log: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        p.subscribe(move |val: &u32| l2.lock().unwrap().push(*val));
        probe!(p, 77u32);
        assert_eq!(log.lock().unwrap()[0], 77);
    }

    #[test]
    fn macro_delivers_struct_event() {
        let mut p: Probe<CpuStepEvent> = Probe::new();
        let log: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        p.subscribe(move |ev: &CpuStepEvent| l2.lock().unwrap().push(ev.pc));
        probe!(
            p,
            CpuStepEvent {
                pc: 0x4000_0000,
                raw: 0xD503_201F,
                insn_class: helm_probe::InsnClass::Unknown,
                is_stub: false,
                #[cfg(feature = "probe-full")]
                insn_count: 0,
            }
        );
        assert_eq!(log.lock().unwrap()[0], 0x4000_0000);
    }

    #[test]
    fn macro_evaluates_once_per_call() {
        let mut p: Probe<u32> = Probe::new();
        let mut n = 0u32;
        let log: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        p.subscribe(move |val: &u32| l2.lock().unwrap().push(*val));
        probe!(p, {
            n += 1;
            n
        });
        probe!(p, {
            n += 1;
            n
        });
        assert_eq!(n, 2, "expression must be evaluated exactly once per call");
    }
}
