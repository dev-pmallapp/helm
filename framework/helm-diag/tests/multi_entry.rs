// Integration test: multi-entry ordering and queue overflow behavior.

#![allow(missing_docs)]

use helm_diag::{DiagSink, DiagLevel, DiagEntry};

// T-ORDER-01
#[test]
fn entries_arrive_in_send_order() {
    use std::io::BufRead;
    let path = std::env::temp_dir().join("helm-diag-order-test.log");
    std::fs::remove_file(&path).ok();
    let uri = format!("file:{}", path.display());
    let n = 100usize;
    {
        let (sink, monitor) = DiagSink::open(&uri).unwrap();
        for i in 0..n {
            monitor.try_send(DiagEntry {
                sim_ns: i as u64, sim_insns: i as u64,
                component: "order-test", level: DiagLevel::Info,
                pc: None, message: format!("entry-{i:04}"),
            });
        }
        drop(monitor); // must drop all senders before sink
        drop(sink);
    }
    let f = std::fs::File::open(&path).unwrap();
    let lines: Vec<_> = std::io::BufReader::new(f)
        .lines().map(|l| l.unwrap()).collect();
    assert_eq!(lines.len(), n, "expected {n} lines, got {}", lines.len());
    for (i, line) in lines.iter().enumerate() {
        assert!(line.contains(&format!("entry-{i:04}")),
            "line {i} must contain entry-{i:04}: {line:?}");
    }
    std::fs::remove_file(&path).ok();
}

// T-ORDER-02
#[test]
fn full_queue_try_send_does_not_block() {
    let (sink, monitor) = DiagSink::open("null:").unwrap();
    for i in 0..8192u64 {
        monitor.try_send(DiagEntry {
            sim_ns: i, sim_insns: i, component: "overflow",
            level: DiagLevel::Info, pc: None,
            message: format!("entry-{i}"),
        });
    }
    drop(monitor);
    drop(sink);
}
