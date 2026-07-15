use somfy_domain::{Direction, Motion, Pos};

const UP_MS: u32 = 10_000;
const DOWN_MS: u32 = 10_000;

#[test]
fn moves_down_proportionally_to_elapsed_time() {
    let mut m = Motion::new(Pos::ZERO);
    m.set_target(Pos::FULL, 1_000);
    let s = m.tick(3_500, UP_MS, DOWN_MS); // 2.5s of a 10s run = 25%
    assert_eq!(s.pos, Pos::from_percent(25));
    assert_eq!(s.direction, Direction::Down);
    assert!(!s.arrived);
}

#[test]
fn moves_up_proportionally_from_partial_position() {
    let mut m = Motion::new(Pos::from_percent(80));
    m.set_target(Pos::from_percent(30), 0);
    let s = m.tick(2_000, UP_MS, DOWN_MS); // 2s of 10s = 20% traveled up
    assert_eq!(s.pos, Pos::from_percent(60));
    assert_eq!(s.direction, Direction::Up);
}

#[test]
fn snaps_to_target_and_reports_arrival_once() {
    let mut m = Motion::new(Pos::ZERO);
    m.set_target(Pos::from_percent(50), 0);
    let s = m.tick(5_000, UP_MS, DOWN_MS);
    assert_eq!(s.pos, Pos::from_percent(50));
    assert!(s.arrived);
    let s2 = m.tick(6_000, UP_MS, DOWN_MS); // stays put after arrival
    assert_eq!(s2.pos, Pos::from_percent(50));
    assert_eq!(s2.direction, Direction::Idle);
    assert!(!s2.arrived);
}

#[test]
fn overshoot_clamps_to_target_not_past() {
    let mut m = Motion::new(Pos::ZERO);
    m.set_target(Pos::from_percent(50), 0);
    let s = m.tick(60_000, UP_MS, DOWN_MS); // way past arrival time
    assert_eq!(s.pos, Pos::from_percent(50));
}

#[test]
fn reversal_mid_travel_uses_live_position_as_new_start() {
    let mut m = Motion::new(Pos::ZERO);
    m.set_target(Pos::FULL, 0);
    m.tick(2_000, UP_MS, DOWN_MS); // at 20%
    m.set_target(Pos::ZERO, 2_000); // reverse toward open
    let s = m.tick(3_000, UP_MS, DOWN_MS); // 1s up from 20% = 10%
    assert_eq!(s.pos, Pos::from_percent(10));
    assert_eq!(s.direction, Direction::Up);
}

#[test]
fn halt_freezes_position() {
    let mut m = Motion::new(Pos::ZERO);
    m.set_target(Pos::FULL, 0);
    m.tick(4_000, UP_MS, DOWN_MS);
    m.halt(4_000, UP_MS, DOWN_MS);
    let s = m.tick(9_000, UP_MS, DOWN_MS);
    assert_eq!(s.pos, Pos::from_percent(40));
    assert_eq!(s.direction, Direction::Idle);
}

#[test]
fn zero_travel_time_jumps_instantly() {
    // Somfy.cpp:1126-1129
    let mut m = Motion::new(Pos::ZERO);
    m.set_target(Pos::FULL, 0);
    let s = m.tick(1, UP_MS, 0);
    assert_eq!(s.pos, Pos::FULL);
    assert!(s.arrived);
}

#[test]
fn asymmetric_travel_times_use_direction_specific_time() {
    let mut m = Motion::new(Pos::FULL);
    m.set_target(Pos::ZERO, 0); // moving UP uses up_time
    let s = m.tick(2_500, 5_000, 20_000); // 2.5s of 5s up = 50% traveled
    assert_eq!(s.pos, Pos::from_percent(50));
}
