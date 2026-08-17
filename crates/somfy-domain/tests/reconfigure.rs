//! Changing a shade's configuration while it is moving.
//!
//! # The failure these pin
//!
//! The estimator computes position **absolutely** — from the start anchor, the
//! elapsed time and the travel time — rather than integrating step by step.
//! That is what makes it immune to a missed tick, and it means a travel time is
//! read as though it had applied to the whole move. So changing one mid-move
//! re-interprets travel that already happened.
//!
//! Concretely: a shade 10 s into a 30 s close is about a third shut. Correct
//! the time to 10 s and the next tick computes `elapsed = 10000`, clamps it to
//! the new travel time, and reports **arrived, fully closed** — so the
//! controller plans an arrival stop that halts the motor a third of the way
//! down and then tells Home Assistant and the UI the shade is shut.
//!
//! It is not an adversarial case. It is the calibration workflow the
//! position-accuracy requirements ask for — time the shade with a stopwatch,
//! then save the measurement — performed while the shade is still travelling,
//! which is exactly when somebody has just timed it.

use heapless::Vec;
use somfy_domain::{PlannedTx, Pos, Shade, ShadeCommand, ShadeConfig};

fn shade_with(down_ms: u32, up_ms: u32) -> Shade {
    let mut config = ShadeConfig::new("Salon", 0x80_1234).expect("a legal address");
    config.down_time_ms = down_ms;
    config.up_time_ms = up_ms;
    Shade::new(config)
}

#[test]
fn correcting_a_travel_time_mid_move_does_not_teleport_the_estimate() {
    let mut shade = shade_with(30_000, 30_000);
    let mut out: Vec<PlannedTx, 4> = Vec::new();

    shade.handle(ShadeCommand::Down, 0, &mut out);
    shade.tick(10_000, &mut out);
    let before = shade.pos().percent();
    assert_eq!(before, 33, "a third of the way down a 30 s close");

    // The stopwatch said ten seconds, not thirty.
    let mut corrected = shade.config.clone();
    corrected.down_time_ms = 10_000;
    corrected.up_time_ms = 10_000;
    shade.reconfigure(corrected, 10_000);

    assert_eq!(
        shade.pos().percent(),
        before,
        "re-anchoring must not move the estimate by itself",
    );
    let snapshot = shade.tick(10_100, &mut out);
    assert!(
        snapshot.pos.percent() < 40,
        "one tick later it should have crept on from {before}%, not jumped — got {}%",
        snapshot.pos.percent(),
    );
    assert!(!snapshot.arrived, "it is a third of the way down, not shut");
}

/// The new time governs what is left, which is the only reading that can be
/// right: nothing knows what the old number should have been.
#[test]
fn the_corrected_time_applies_to_the_remaining_travel() {
    let mut shade = shade_with(30_000, 30_000);
    let mut out: Vec<PlannedTx, 4> = Vec::new();

    shade.handle(ShadeCommand::Down, 0, &mut out);
    shade.tick(10_000, &mut out);

    let mut corrected = shade.config.clone();
    corrected.down_time_ms = 12_000;
    shade.reconfigure(corrected, 10_000);

    // A third shut, with 12 s now standing for the whole traverse: the
    // remaining two thirds should take about 8 s.
    shade.tick(15_000, &mut out);
    assert!(
        shade.pos().percent() < 100,
        "5 s into 8 s of remaining travel, it is not shut yet — got {}%",
        shade.pos().percent(),
    );
    shade.tick(19_000, &mut out);
    assert_eq!(
        shade.pos().percent(),
        100,
        "9 s into 8 s of remaining travel, it is shut",
    );
}

/// An idle shade is unaffected: re-anchoring a motion that is not moving must
/// not disturb the position it is holding, however absurd the new times are.
#[test]
fn reconfiguring_an_idle_shade_leaves_its_position_alone() {
    let mut shade = shade_with(10_000, 10_000);
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(40)), 0, &mut out);
    shade.tick(4_000, &mut out);
    shade.handle(ShadeCommand::My, 4_000, &mut out);
    let held = shade.pos().percent();

    let mut corrected = shade.config.clone();
    corrected.down_time_ms = 1;
    corrected.up_time_ms = 1;
    shade.reconfigure(corrected, 4_000);
    shade.tick(60_000, &mut out);

    assert_eq!(shade.pos().percent(), held);
}

/// The address is the shade's, whatever the incoming configuration says. A
/// motor obeys an address; nothing in this protocol can tell it the address
/// moved, so a shade whose address changed is one that stops responding and is
/// fixed only by walking to it.
#[test]
fn reconfiguring_cannot_move_the_address() {
    let mut shade = shade_with(10_000, 10_000);
    let mut elsewhere = ShadeConfig::new("Salon", 0x80_9999).unwrap();
    elsewhere.up_time_ms = 30_000;
    shade.reconfigure(elsewhere, 0);
    assert_eq!(shade.config.address, 0x80_1234);
    assert_eq!(shade.config.up_time_ms, 30_000, "the rest still applied");
}

/// Everything else in the configuration does arrive — this is an edit, not a
/// filter.
#[test]
fn reconfiguring_applies_the_name_and_the_kind() {
    let mut shade = shade_with(10_000, 10_000);
    let mut next = shade.config.clone();
    next.name = heapless::String::try_from("Cuisine").unwrap();
    next.kind = somfy_domain::ShadeKind::Blind;
    shade.reconfigure(next, 0);
    assert_eq!(shade.config.name.as_str(), "Cuisine");
    assert_eq!(shade.config.kind, somfy_domain::ShadeKind::Blind);
}
