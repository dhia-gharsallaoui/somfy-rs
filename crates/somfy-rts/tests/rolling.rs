use somfy_rts::{Command, RollingCode};

#[test]
fn transmits_current_code_then_increments() {
    let mut rc = RollingCode(41);
    let f1 = rc.next_frame(Command::Up, 0x123456);
    assert_eq!(f1.rolling_code, 41);
    assert_eq!(rc.0, 42);
    let f2 = rc.next_frame(Command::Down, 0x123456);
    assert_eq!(f2.rolling_code, 42);
    assert_eq!(rc.0, 43);
}

#[test]
fn wraps_at_u16_max() {
    let mut rc = RollingCode(u16::MAX);
    let f = rc.next_frame(Command::My, 1);
    assert_eq!(f.rolling_code, u16::MAX);
    assert_eq!(rc.0, 0);
}

#[test]
fn key_byte_is_0xa_high_nibble_and_code_low_nibble() {
    let mut rc = RollingCode(0x0102);
    let f = rc.next_frame(Command::Up, 1);
    assert_eq!(f.key, 0xA2);
}
