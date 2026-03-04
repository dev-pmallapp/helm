use crate::scoreboard::Scoreboard;

#[test]
fn new_scoreboard_has_zero_values() {
    let sb = Scoreboard::<u64>::new(4);
    assert_eq!(*sb.get(0), 0);
    assert_eq!(*sb.get(3), 0);
}

#[test]
fn per_vcpu_writes_are_independent() {
    let sb = Scoreboard::<u64>::new(4);
    *sb.get_mut(0) = 100;
    *sb.get_mut(1) = 200;
    assert_eq!(*sb.get(0), 100);
    assert_eq!(*sb.get(1), 200);
}

#[test]
fn iter_sums_all_slots() {
    let sb = Scoreboard::<u64>::new(4);
    *sb.get_mut(0) = 10;
    *sb.get_mut(1) = 20;
    *sb.get_mut(2) = 30;
    let total: u64 = sb.iter().sum();
    assert_eq!(total, 60);
}

#[test]
fn len_matches_construction() {
    let sb = Scoreboard::<u64>::new(8);
    assert_eq!(sb.len(), 8);
    assert!(!sb.is_empty());
}
