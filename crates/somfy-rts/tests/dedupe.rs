use somfy_rts::{Command, Frame, RxDeduper};

fn f(code: u16) -> Frame {
    Frame {
        key: 0xA0,
        command: Command::Up,
        rolling_code: code,
        address: 0xABCDEF,
    }
}

fn fa(address: u32, code: u16) -> Frame {
    Frame {
        key: 0xA0,
        command: Command::Up,
        rolling_code: code,
        address,
    }
}

#[test]
fn repeats_within_window_are_suppressed() {
    let mut d = RxDeduper::new(2000);
    assert!(d.accept(&f(10), 0));
    assert!(!d.accept(&f(10), 50));
    assert!(!d.accept(&f(10), 500));
}

#[test]
fn next_rolling_code_is_a_new_event() {
    let mut d = RxDeduper::new(2000);
    assert!(d.accept(&f(10), 0));
    assert!(d.accept(&f(11), 300));
}

#[test]
fn same_code_after_window_expiry_is_accepted() {
    let mut d = RxDeduper::new(2000);
    assert!(d.accept(&f(10), 0));
    assert!(d.accept(&f(10), 2500));
}

#[test]
fn different_addresses_do_not_collide() {
    let mut d = RxDeduper::new(2000);
    let mut g = f(10);
    g.address = 0x000001;
    assert!(d.accept(&f(10), 0));
    assert!(d.accept(&g, 10));
}

// --- Eviction tests (beyond the brief's four) ---
// The deduper keeps a bounded map of 8 (address, rolling_code) entries. When a
// new distinct key arrives while the map is full, the OLDEST entry (largest
// age relative to now_ms) must be evicted. These tests exercise that bound.

/// Under normal, strictly-increasing timestamps the genuinely-oldest entry is
/// evicted and forgotten, while a newer entry survives and still suppresses its
/// repeat.
#[test]
fn eviction_drops_the_oldest_entry() {
    let mut d = RxDeduper::new(2000);
    // Fill all 8 slots with distinct addresses at t = 0..=7.
    for i in 0..8u32 {
        assert!(d.accept(&fa(0x100 + i, 10), i));
    }
    // A 9th distinct address at t = 8 is full -> evicts the oldest (addr 0x100).
    assert!(d.accept(&fa(0x200, 10), 8));

    // The evicted oldest (addr 0x100) is forgotten: re-presenting it within the
    // window is accepted as a fresh event.
    assert!(d.accept(&fa(0x100, 10), 9));
    // A survivor (addr 0x107, inserted at t = 7) is still remembered: its repeat
    // within the window is suppressed.
    assert!(!d.accept(&fa(0x107, 10), 10));
}

/// Regression guard for the `wrapping_neg` eviction bug: when the newest
/// existing entry shares its timestamp with `now_ms` (age 0), it must NOT be
/// mistaken for the oldest and evicted. The oldest (largest age) goes instead.
#[test]
fn eviction_never_drops_the_newest_on_timestamp_tie() {
    let mut d = RxDeduper::new(2000);
    // Seven distinct addresses at t = 0..=6.
    for i in 0..7u32 {
        assert!(d.accept(&fa(0x100 + i, 10), i));
    }
    // Eighth distinct address at t = 100 fills the map (age 0 at the next call).
    assert!(d.accept(&fa(0x207, 10), 100));
    // Ninth distinct address at the SAME timestamp triggers eviction at now=100.
    assert!(d.accept(&fa(0x208, 10), 100));

    // The genuine oldest (addr 0x100, age 100) was evicted -> forgotten.
    assert!(d.accept(&fa(0x100, 10), 101));
    // The newest prior entry (addr 0x207, age 0 at eviction) must have survived,
    // so its repeat within the window is still suppressed.
    assert!(!d.accept(&fa(0x207, 10), 101));
}
