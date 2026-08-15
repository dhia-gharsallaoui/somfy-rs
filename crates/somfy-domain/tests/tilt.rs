use somfy_domain::{tilt_first, Direction, Motion, Pos, TiltMode};

#[test]
fn integrated_mode_tilts_before_lifting() {
    // Integrated-tilt sequencing rule: moving up with tilt != 0 tilts first;
    // moving down with tilt != 100 tilts first.
    assert!(tilt_first(
        TiltMode::Integrated,
        Direction::Up,
        Pos::from_percent(30)
    ));
    assert!(!tilt_first(TiltMode::Integrated, Direction::Up, Pos::ZERO));
    assert!(tilt_first(
        TiltMode::Integrated,
        Direction::Down,
        Pos::from_percent(30)
    ));
    assert!(!tilt_first(
        TiltMode::Integrated,
        Direction::Down,
        Pos::FULL
    ));
}

#[test]
fn non_integrated_modes_never_tilt_first() {
    for mode in [
        TiltMode::None,
        TiltMode::TiltMotor,
        TiltMode::TiltOnly,
        TiltMode::EuroMode,
    ] {
        assert!(!tilt_first(mode, Direction::Up, Pos::from_percent(50)));
        assert!(!tilt_first(mode, Direction::Down, Pos::from_percent(50)));
    }
}

#[test]
fn idle_lift_never_requires_tilt_first() {
    assert!(!tilt_first(
        TiltMode::Integrated,
        Direction::Idle,
        Pos::from_percent(50)
    ));
}

#[test]
fn tilt_axis_is_a_motion_with_tilt_time() {
    // The tilt axis reuses Motion with tilt_time for both directions,
    // matching deployed firmware's tilt branches, which use a single tilt
    // time regardless of direction.
    let mut t = Motion::new(Pos::ZERO);
    t.set_target(Pos::FULL, 0);
    let s = t.tick(3_500, 7_000, 7_000);
    assert_eq!(s.pos, Pos::from_percent(50));
}
