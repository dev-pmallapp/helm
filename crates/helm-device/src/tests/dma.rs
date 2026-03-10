use crate::dma::*;

#[test]
fn new_engine_channels_idle() {
    let engine = DmaEngine::new(4);
    assert_eq!(engine.num_channels(), 4);
    for i in 0..4 {
        assert_eq!(engine.channel(i).unwrap().status, DmaStatus::Idle);
    }
}

#[test]
fn start_sets_running() {
    let mut engine = DmaEngine::new(2);
    let ch = DmaChannel::new(0x1000, 0x2000, 64, DmaDirection::MemToMem);
    engine.start(0, ch);
    assert_eq!(engine.channel(0).unwrap().status, DmaStatus::Running);
}

#[test]
fn tick_advances_transfer() {
    let mut engine = DmaEngine::new(1);
    let mut ch = DmaChannel::new(0x1000, 0x2000, 16, DmaDirection::MemToMem);
    ch.beat_size = 8;
    engine.start(0, ch);

    // First tick: 8 bytes
    let completed = engine.tick();
    assert!(completed.is_empty());
    assert_eq!(engine.channel(0).unwrap().bytes_transferred, 8);

    // Second tick: 16 bytes → complete
    let completed = engine.tick();
    assert_eq!(completed, vec![0]);
    assert!(engine.channel(0).unwrap().is_complete());
}

#[test]
fn total_beats_calculation() {
    let mut ch = DmaChannel::new(0, 0, 100, DmaDirection::MemToDevice);
    ch.beat_size = 8;
    assert_eq!(ch.total_beats(), 13); // ceil(100/8) = 13
}

#[test]
fn estimated_cycles() {
    let mut ch = DmaChannel::new(0, 0, 64, DmaDirection::DeviceToMem);
    ch.beat_size = 8;
    ch.stall_per_beat = 2;
    assert_eq!(ch.estimated_cycles(), 16); // 8 beats * 2 cycles
}

#[test]
fn reset_engine() {
    let mut engine = DmaEngine::new(2);
    engine.start(0, DmaChannel::new(0, 0, 8, DmaDirection::MemToMem));
    engine.tick();
    engine.reset();
    assert_eq!(engine.channel(0).unwrap().status, DmaStatus::Idle);
    assert_eq!(engine.channel(0).unwrap().bytes_transferred, 0);
}

#[test]
fn idle_channels_not_ticked() {
    let mut engine = DmaEngine::new(2);
    let completed = engine.tick();
    assert!(completed.is_empty());
}
