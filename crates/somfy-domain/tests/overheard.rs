use heapless::Vec;
use somfy_domain::{Direction, DomainError, PlannedTx, Pos, Shade, ShadeCommand, ShadeConfig};
use somfy_rts::Command;

fn shade() -> Shade {
    Shade::new(ShadeConfig::new("Test", 0x111111).unwrap())
}

#[test]
fn link_remote_enforces_limit_and_duplicates() {
    let mut s = shade();
    for i in 1..=7u32 {
        s.link_remote(0x20_0000 + i).unwrap();
    }
    assert!(matches!(
        s.link_remote(0x20_0099),
        Err(DomainError::RegistryFull)
    ));
    let mut s2 = shade();
    s2.link_remote(0x222222).unwrap();
    assert!(matches!(
        s2.link_remote(0x222222),
        Err(DomainError::DuplicateAddress)
    ));
    assert!(matches!(
        s2.link_remote(0),
        Err(DomainError::InvalidAddress)
    ));
}

#[test]
fn is_linked_covers_own_and_linked_addresses() {
    let mut s = shade();
    s.link_remote(0x222222).unwrap();
    assert!(s.is_linked(0x111111)); // own address
    assert!(s.is_linked(0x222222));
    assert!(!s.is_linked(0x333333));
}

#[test]
fn overheard_down_moves_estimate_without_retransmit() {
    let mut s = shade();
    s.apply_overheard(Command::Down, 0);
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    let snap = s.tick(5_000, &mut out);
    assert_eq!(snap.pos, Pos::from_percent(50));
    assert_eq!(snap.direction, Direction::Down);
    assert!(out.is_empty(), "overheard frames must never emit TX");
}

#[test]
fn overheard_my_while_moving_halts_estimate() {
    let mut s = shade();
    s.apply_overheard(Command::Down, 0);
    s.tick(3_000, &mut Vec::new());
    s.apply_overheard(Command::My, 3_000);
    assert_eq!(s.pos(), Pos::from_percent(30));
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn overheard_my_while_idle_tracks_favorite() {
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(25))),
        0,
        &mut out,
    );
    s.apply_overheard(Command::My, 0);
    let snap = s.tick(2_500, &mut Vec::new());
    assert_eq!(snap.pos, Pos::from_percent(25));
}

#[test]
fn overheard_step_down_moves_estimate_by_one_step_without_tx() {
    // Deployed firmware's StepUp/StepDown target math has NO internal-frame
    // gate, so an overheard step from a linked remote moves the estimate.
    // One press = 1% = 100 raw on the default 10 s travel, and the wall
    // remote already drove the motor, so we NEVER retransmit.
    let mut s = shade();
    s.link_remote(0x222222).unwrap();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.apply_overheard(Command::StepDown, 0);
    let snap = s.tick(1_000, &mut out);
    assert_eq!(snap.pos, Pos::from_raw(100), "one StepDown = 100 raw down");
    assert!(out.is_empty(), "overheard steps must never plan TX");
}

#[test]
fn overheard_step_up_moves_estimate_up_without_tx() {
    // From a mid position, an overheard StepUp nudges the estimate one step
    // toward open (up) and plans no TX.
    let mut s = shade();
    s.link_remote(0x222222).unwrap();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    // Seed the estimate at 50% via an overheard Down + tick.
    s.apply_overheard(Command::Down, 0);
    s.tick(5_000, &mut out);
    assert_eq!(s.pos(), Pos::from_percent(50));
    out.clear();
    s.apply_overheard(Command::StepUp, 5_000);
    let snap = s.tick(6_000, &mut out);
    assert_eq!(
        snap.pos,
        Pos::from_raw(5_000 - 100),
        "one StepUp = 100 raw up"
    );
    assert!(out.is_empty(), "overheard steps must never plan TX");
}

#[test]
fn overheard_arrival_at_midrange_does_not_plan_stop() {
    // The wall remote's motor stops on its own My; we must not TX.
    let mut s = shade();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(25))),
        0,
        &mut out,
    );
    out.clear();
    s.apply_overheard(Command::My, 0);
    s.tick(10_000, &mut out);
    assert!(out.is_empty());
}
