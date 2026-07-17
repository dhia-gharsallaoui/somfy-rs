use heapless::Vec;
use somfy_domain::{Direction, PlannedTx, Pos, Shade, ShadeCommand, ShadeConfig};
use somfy_rts::Command;

fn shade() -> Shade {
    Shade::new(ShadeConfig::new("Test", 0x123456).unwrap())
}

fn tx(out: &Vec<PlannedTx, 4>) -> std::vec::Vec<Command> {
    out.iter().map(|t| t.command).collect()
}

#[test]
fn up_command_targets_zero_and_transmits_up() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    s.tick(5_000, &mut out); // mid-travel at 50%
    out.clear();
    s.handle(ShadeCommand::Up, 5_000, &mut out);
    assert_eq!(tx(&out), [Command::Up]);
    assert_eq!(s.direction(), Direction::Up);
}

#[test]
fn my_while_moving_stops_and_transmits_my() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    out.clear();
    s.handle(ShadeCommand::My, 4_000, &mut out);
    assert_eq!(tx(&out), [Command::My]);
    assert_eq!(s.pos(), Pos::from_percent(40));
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn my_while_idle_goes_to_favorite() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(30))),
        0,
        &mut out,
    );
    assert!(out.is_empty()); // SetMy transmits nothing
    s.handle(ShadeCommand::My, 0, &mut out);
    assert_eq!(tx(&out), [Command::Down]); // from 0% toward 30%
    let snap = s.tick(3_000, &mut out);
    assert_eq!(snap.pos, Pos::from_percent(30));
}

#[test]
fn my_while_idle_without_favorite_is_noop() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::My, 0, &mut out);
    assert!(out.is_empty());
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn goto_midrange_emits_stop_on_arrival() {
    // Somfy.cpp:1166-1170: motor only self-stops at hard limits; a
    // mid-range target needs an explicit My at arrival.
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    assert_eq!(tx(&out), [Command::Down]);
    out.clear();
    let snap = s.tick(5_000, &mut out);
    assert!(snap.arrived);
    assert_eq!(tx(&out), [Command::My]); // the scheduled stop
}

#[test]
fn goto_full_limit_does_not_emit_stop() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::FULL), 0, &mut out);
    out.clear();
    let snap = s.tick(20_000, &mut out);
    assert!(snap.arrived);
    assert!(out.is_empty()); // hard limit: motor stops itself
}

#[test]
fn goto_current_position_is_noop() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::ZERO), 0, &mut out);
    assert!(out.is_empty());
}

#[test]
fn step_commands_nudge_target_and_emit_extended_commands() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    s.tick(5_000, &mut out);
    out.clear();
    s.handle(ShadeCommand::StepDown, 5_000, &mut out);
    assert_eq!(tx(&out), [Command::StepDown]);
    out.clear();
    let snap = s.tick(20_000, &mut out);
    // C++ Somfy.cpp:2481/2522: step target = pos +/- 100/(travel/(stepSize*frameStep)).
    // Shipped defaults (stepSize=100 @ Somfy.cpp:701 / Somfy.h:317, frameStep=1
    // @ Somfy.cpp:2452/2493, travel=10000ms) resolve to a 1% (100-raw) nudge --
    // not the brief's 5% guess. See report cross-check for the derivation.
    assert_eq!(snap.pos, Pos::from_raw(5_100)); // 50% + 1% step

    // Step targets are NOT `settingPos` targets: the C++ Step branches
    // (Somfy.cpp:2443-2525) never set settingPos, so the mid-range My at
    // Somfy.cpp:1166/1218 is skipped -- the motor self-stops after its
    // increment. No stop frame may follow a step arrival.
    assert!(out.is_empty());
}

#[test]
fn step_up_nudges_toward_open_and_clamps_at_zero() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    s.tick(5_000, &mut out);
    out.clear();
    s.handle(ShadeCommand::StepUp, 5_000, &mut out);
    assert_eq!(tx(&out), [Command::StepUp]);
    let snap = s.tick(20_000, &mut out);
    assert_eq!(snap.pos, Pos::from_raw(4_900)); // 50% - 1% step

    // At the hard limit the C++ still transmits the step frame (emitCommand
    // is unconditional, Somfy.cpp:2483) but the position cannot move.
    let mut s = shade(); // fresh shade at ZERO
    out.clear();
    s.handle(ShadeCommand::StepUp, 0, &mut out);
    assert_eq!(tx(&out), [Command::StepUp]);
    assert_eq!(s.pos(), Pos::ZERO);
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn set_my_none_clears_favorite() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(30))),
        0,
        &mut out,
    );
    assert_eq!(s.my_pos(), Some(Pos::from_percent(30)));
    s.handle(ShadeCommand::SetMy(None), 0, &mut out);
    assert_eq!(s.my_pos(), None);
    assert!(out.is_empty());
    s.handle(ShadeCommand::My, 0, &mut out); // cleared favorite: My is a no-op
    assert!(out.is_empty());
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn stop_is_never_emitted_only_my() {
    // Plan 1 contract: Stop downgrades to My (Somfy.cpp:2944); this
    // crate must never plan a Command::Stop TX.
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    s.handle(ShadeCommand::My, 3_000, &mut out);
    s.handle(ShadeCommand::GoTo(Pos::from_percent(70)), 3_000, &mut out);
    s.tick(30_000, &mut out);
    assert!(out.iter().all(|t| t.command != Command::Stop));
}

#[test]
fn planned_tx_carries_shade_address() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    assert_eq!(out[0].address, 0x123456);
}

#[test]
fn target_accessor_reports_seek_destination() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(70)), 0, &mut out);
    assert_eq!(s.target(), Pos::from_percent(70));
}
