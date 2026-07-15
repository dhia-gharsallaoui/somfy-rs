use somfy_domain::{Direction, DomainError, Pos, ShadeConfig};

#[test]
fn pos_is_clamped_and_converts() {
    assert_eq!(Pos::from_raw(20000), Pos::FULL);
    assert_eq!(Pos::from_percent(50).raw(), 5000);
    assert_eq!(Pos::from_percent(200), Pos::FULL); // >100% clamps
    assert_eq!(Pos::FULL.percent(), 100);
    assert_eq!(Pos::ZERO.percent(), 0);
    assert!(Pos::ZERO < Pos::FULL);
}

#[test]
fn direction_signs_match_cpp() {
    assert_eq!(Direction::Up.sign(), -1); // C++ -1 moves toward 0 (open)
    assert_eq!(Direction::Idle.sign(), 0);
    assert_eq!(Direction::Down.sign(), 1); // C++ +1 moves toward 100 (closed)
}

#[test]
fn shade_config_defaults_match_cpp() {
    let c = ShadeConfig::new("Kitchen", 0x1234).unwrap();
    assert_eq!(c.up_time_ms, 10_000); // Somfy.h:314
    assert_eq!(c.down_time_ms, 10_000); // Somfy.h:315
    assert_eq!(c.tilt_time_ms, 7_000); // Somfy.h:316
    assert_eq!(c.name.as_str(), "Kitchen");
}

#[test]
fn address_plausibility_guard_matches_cpp() {
    // Somfy.cpp:169-170: address must be in 1..0xFFFFFF (exclusive of both sentinels)
    assert!(matches!(
        ShadeConfig::new("X", 0),
        Err(DomainError::InvalidAddress)
    ));
    assert!(matches!(
        ShadeConfig::new("X", 0xFF_FFFF),
        Err(DomainError::InvalidAddress)
    ));
    assert!(ShadeConfig::new("X", 1).is_ok());
    assert!(ShadeConfig::new("X", 0xFF_FFFE).is_ok());
}

#[test]
fn name_too_long_is_rejected() {
    let long = "abcdefghijklmnopqrstuvwxyz0123456789"; // 36 chars > 32
    assert!(matches!(
        ShadeConfig::new(long, 5),
        Err(DomainError::NameTooLong)
    ));
}
