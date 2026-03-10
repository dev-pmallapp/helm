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

#[test]
fn peek_time_none_on_empty_queue() {
    let q = EventQueue::new();
    assert_eq!(q.peek_time(), None);
}

#[test]
fn len_tracks_schedule_and_pop() {
    let mut q = EventQueue::new();
    assert_eq!(q.len(), 0);
    q.schedule(10, 0, 1);
    assert_eq!(q.len(), 1);
    q.schedule(20, 0, 2);
    assert_eq!(q.len(), 2);
    q.pop();
    assert_eq!(q.len(), 1);
}

#[test]
fn pop_all_empties_queue() {
    let mut q = EventQueue::new();
    q.schedule(1, 0, 0);
    q.schedule(2, 0, 1);
    q.pop();
    q.pop();
    assert!(q.is_empty());
    assert!(q.pop().is_none());
}

#[test]
fn current_time_starts_at_zero() {
    let q = EventQueue::new();
    assert_eq!(q.current_time(), 0);
}

#[test]
fn schedule_multiple_same_time_all_emitted() {
    let mut q = EventQueue::new();
    q.schedule(5, 0, 10);
    q.schedule(5, 0, 11);
    q.schedule(5, 0, 12);
    assert_eq!(q.len(), 3);
    // All three should be poppable
    let t0 = q.pop().unwrap().tag;
    let t1 = q.pop().unwrap().tag;
    let t2 = q.pop().unwrap().tag;
    let mut tags = vec![t0, t1, t2];
    tags.sort();
    assert_eq!(tags, vec![10, 11, 12]);
}
