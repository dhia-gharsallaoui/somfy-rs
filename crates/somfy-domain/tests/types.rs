use somfy_domain::{Direction, DomainError, Pos, ShadeConfig, ShadeKind, TiltMode};

#[test]
fn pos_is_clamped_and_converts() {
    assert_eq!(Pos::from_raw(20000), Pos::FULL);
    assert_eq!(Pos::from_percent(50).raw(), 5000);
    assert_eq!(Pos::from_percent(200), Pos::FULL); // >100% clamps
    assert_eq!(Pos::FULL.percent(), 100);
    assert_eq!(Pos::ZERO.percent(), 0);
    assert!(Pos::ZERO < Pos::FULL);
}

/// Up's sign is negative (moves toward position 0, open); Down's sign is
/// positive (moves toward position 100, closed).
#[test]
fn direction_up_is_negative_and_down_is_positive() {
    assert_eq!(Direction::Up.sign(), -1); // -1 moves toward 0 (open)
    assert_eq!(Direction::Idle.sign(), 0);
    assert_eq!(Direction::Down.sign(), 1); // +1 moves toward 100 (closed)
}

#[test]
fn shade_config_new_applies_default_travel_times() {
    let c = ShadeConfig::new("Kitchen", 0x1234).unwrap();
    assert_eq!(c.up_time_ms, 10_000); // default up-travel time deployed firmware ships with
    assert_eq!(c.down_time_ms, 10_000); // default down-travel time deployed firmware ships with
    assert_eq!(c.tilt_time_ms, 7_000); // default tilt time deployed firmware ships with
    assert_eq!(c.name.as_str(), "Kitchen");
}

#[test]
fn address_plausibility_guard_rejects_sentinels() {
    // Address must be in 1..0xFFFFFF (exclusive of both sentinels), matching
    // deployed firmware's plausibility guard on the shade address.
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
fn shade_kind_from_raw_round_trips_known_values() {
    // The v1.0 subset of deployed firmware's shade-kind enumeration.
    let known = [
        (0x00u8, ShadeKind::Roller),
        (0x01, ShadeKind::Blind),
        (0x02, ShadeKind::DraperyLeft),
        (0x03, ShadeKind::Awning),
        (0x04, ShadeKind::Shutter),
        (0x07, ShadeKind::DraperyRight),
        (0x08, ShadeKind::DraperyCenter),
    ];
    for (raw, kind) in known {
        assert_eq!(ShadeKind::from_raw(raw), Some(kind), "raw {raw:#x}");
    }
}

#[test]
fn shade_kind_from_raw_rejects_unsupported_and_invalid() {
    // Not-yet-supported deployed-firmware kinds: garage 0x05/0x06,
    // drycontact/gate 0x09-0x10.
    for raw in [0x05u8, 0x06, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10] {
        assert_eq!(
            ShadeKind::from_raw(raw),
            None,
            "unsupported shade kind {raw:#x}"
        );
    }
    // Arbitrary invalid bytes.
    for raw in [0x11u8, 0x42, 0xFE, 0xFF] {
        assert_eq!(ShadeKind::from_raw(raw), None, "invalid byte {raw:#x}");
    }
}

#[test]
fn tilt_mode_from_raw_round_trips_known_values() {
    let known = [
        (0x00u8, TiltMode::None),
        (0x01, TiltMode::TiltMotor),
        (0x02, TiltMode::Integrated),
        (0x03, TiltMode::TiltOnly),
        (0x04, TiltMode::EuroMode),
    ];
    for (raw, mode) in known {
        assert_eq!(TiltMode::from_raw(raw), Some(mode), "raw {raw:#x}");
    }
}

#[test]
fn tilt_mode_from_raw_rejects_invalid() {
    for raw in [0x05u8, 0x06, 0x42, 0xFF] {
        assert_eq!(TiltMode::from_raw(raw), None, "invalid byte {raw:#x}");
    }
}

#[test]
fn name_too_long_is_rejected() {
    let long = "abcdefghijklmnopqrstuvwxyz0123456789"; // 36 chars > 32
    assert!(matches!(
        ShadeConfig::new(long, 5),
        Err(DomainError::NameTooLong)
    ));
}
