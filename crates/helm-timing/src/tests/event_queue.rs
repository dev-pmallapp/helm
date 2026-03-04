use crate::event_queue::*;

#[test]
fn empty_queue() {
    let mut q = EventQueue::new();
    assert!(q.is_empty());
    assert!(q.pop().is_none());
}

#[test]
fn events_come_out_in_time_order() {
    let mut q = EventQueue::new();
    q.schedule(10, 0, 1);
    q.schedule(5, 0, 2);
    q.schedule(20, 0, 3);

    assert_eq!(q.pop().unwrap().tag, 2); // t=5
    assert_eq!(q.pop().unwrap().tag, 1); // t=10
    assert_eq!(q.pop().unwrap().tag, 3); // t=20
}

#[test]
fn current_time_advances() {
    let mut q = EventQueue::new();
    q.schedule(100, 0, 0);
    q.pop();
    assert_eq!(q.current_time(), 100);
}

#[test]
fn peek_does_not_consume() {
    let mut q = EventQueue::new();
    q.schedule(42, 0, 0);
    assert_eq!(q.peek_time(), Some(42));
    assert_eq!(q.len(), 1);
}
