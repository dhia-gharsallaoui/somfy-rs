//! Integration tests for the [`Controller`] facade: RX routing, group fan-out,
//! and state-delta emission.

use heapless::Vec;
use somfy_domain::{
    Controller, Direction, DomainError, GroupId, PlannedTx, Pos, ShadeCommand, ShadeConfig,
    StateDelta,
};
use somfy_rts::{Command, Frame};

fn setup() -> (Controller, somfy_domain::ShadeId) {
    let mut c = Controller::new();
    let id = c
        .registry
        .add_shade(ShadeConfig::new("A", 0x101).unwrap())
        .unwrap();
    (c, id)
}

fn bufs() -> (Vec<PlannedTx, 8>, Vec<StateDelta, 32>) {
    (Vec::new(), Vec::new())
}

#[test]
fn command_shade_plans_tx_and_emits_delta_on_tick() {
    let (mut c, id) = setup();
    let (mut tx, mut deltas) = bufs();
    c.command_shade(id, ShadeCommand::Down, 0, &mut tx, &mut deltas)
        .unwrap();
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].command, Command::Down);
    let (mut tx2, mut deltas2) = bufs();
    c.tick(5_000, &mut tx2, &mut deltas2);
    assert_eq!(deltas2.len(), 1);
    assert_eq!(deltas2[0].pos, Pos::from_percent(50));
    assert_eq!(deltas2[0].direction, Direction::Down);
}

#[test]
fn idle_ticks_emit_no_deltas() {
    let (mut c, _) = setup();
    let (mut tx, mut deltas) = bufs();
    c.tick(1_000, &mut tx, &mut deltas);
    c.tick(2_000, &mut tx, &mut deltas);
    assert!(deltas.is_empty());
    assert!(tx.is_empty());
}

#[test]
fn group_command_fans_out() {
    let mut c = Controller::new();
    let a = c
        .registry
        .add_shade(ShadeConfig::new("A", 0x101).unwrap())
        .unwrap();
    let b = c
        .registry
        .add_shade(ShadeConfig::new("B", 0x102).unwrap())
        .unwrap();
    let g = c.registry.add_group("All").unwrap();
    c.registry.group_add_shade(g, a).unwrap();
    c.registry.group_add_shade(g, b).unwrap();
    let (mut tx, mut deltas) = bufs();
    c.command_group(g, ShadeCommand::Down, 0, &mut tx, &mut deltas)
        .unwrap();
    assert_eq!(tx.len(), 2);
    let addrs: std::vec::Vec<u32> = tx.iter().map(|t| t.address).collect();
    assert!(addrs.contains(&0x101) && addrs.contains(&0x102));
}

#[test]
fn rx_frame_routes_to_linked_shade_and_dedupes_repeats() {
    let (mut c, id) = setup();
    c.registry
        .shade_mut(id)
        .unwrap()
        .link_remote(0x202)
        .unwrap();
    let frame = Frame {
        key: 0xA1,
        command: Command::Down,
        rolling_code: 7,
        address: 0x202,
    };
    let (_, mut deltas) = bufs();
    c.on_rx_frame(&frame, 0, &mut deltas);
    // repeats of the same press within the window: ignored
    c.on_rx_frame(&frame, 120, &mut deltas);
    c.on_rx_frame(&frame, 240, &mut deltas);
    let (mut tx, mut deltas2) = bufs();
    c.tick(5_000, &mut tx, &mut deltas2);
    assert_eq!(deltas2[0].pos, Pos::from_percent(50)); // moved once, not thrice
    assert!(tx.is_empty(), "overheard movement must not retransmit");
}

#[test]
fn rx_frame_from_unknown_address_is_ignored() {
    let (mut c, _) = setup();
    let frame = Frame {
        key: 0xA1,
        command: Command::Down,
        rolling_code: 7,
        address: 0x999,
    };
    let (_, mut deltas) = bufs();
    c.on_rx_frame(&frame, 0, &mut deltas);
    let (mut tx, mut deltas2) = bufs();
    c.tick(5_000, &mut tx, &mut deltas2);
    assert!(deltas2.is_empty());
}

#[test]
fn deltas_deduplicate_unchanged_state() {
    let (mut c, id) = setup();
    let (mut tx, mut deltas) = bufs();
    c.command_shade(
        id,
        ShadeCommand::GoTo(Pos::from_percent(50)),
        0,
        &mut tx,
        &mut deltas,
    )
    .unwrap();
    let (mut tx1, mut d1) = bufs();
    c.tick(10_000, &mut tx1, &mut d1); // arrives (50%) + plans My stop
    assert_eq!(d1.len(), 1);
    assert_eq!(tx1.len(), 1);
    let (mut tx2, mut d2) = bufs();
    c.tick(11_000, &mut tx2, &mut d2); // nothing changed since
    assert!(d2.is_empty());
    assert!(tx2.is_empty());
}

#[test]
fn reused_slot_emits_first_delta_for_new_shade() {
    // A slot-stable registry reuses the lowest free slot on the next add. The
    // controller keys its "last emitted" cache by slot, so a re-added shade in
    // a reused slot must NOT be suppressed by the previous occupant's stale
    // state — even when the two happen to be numerically identical.
    let mut c = Controller::new();
    let a = c
        .registry
        .add_shade(ShadeConfig::new("A", 0x101).unwrap())
        .unwrap();
    let (mut tx, mut deltas) = bufs();
    // A starts moving down: records (pos=0, tilt=0, Down) for this slot.
    c.command_shade(a, ShadeCommand::Down, 0, &mut tx, &mut deltas)
        .unwrap();
    // Remove A; the recorded slot state is now stale.
    c.registry.remove_shade(a).unwrap();
    // B reuses the freed slot (lowest free slot first).
    let b = c
        .registry
        .add_shade(ShadeConfig::new("B", 0x102).unwrap())
        .unwrap();
    assert_eq!(a, b, "B must reuse A's slot for this test to be meaningful");
    // B starts moving down too: numerically identical to A's stale entry.
    let (mut tx2, mut deltas2) = bufs();
    c.command_shade(b, ShadeCommand::Down, 0, &mut tx2, &mut deltas2)
        .unwrap();
    assert_eq!(
        deltas2.len(),
        1,
        "re-added shade must emit its first delta despite stale slot state"
    );
    assert_eq!(deltas2[0].id, b);
    assert_eq!(deltas2[0].direction, Direction::Down);
}

#[test]
fn command_group_missing_group_is_not_found() {
    let mut c = Controller::new();
    let (mut tx, mut deltas) = bufs();
    let missing = GroupId(9);
    assert!(matches!(
        c.command_group(missing, ShadeCommand::Down, 0, &mut tx, &mut deltas),
        Err(DomainError::NotFound)
    ));
}

#[test]
fn command_group_empty_group_is_ok_with_no_work() {
    let mut c = Controller::new();
    let g = c.registry.add_group("Empty").unwrap();
    let (mut tx, mut deltas) = bufs();
    c.command_group(g, ShadeCommand::Down, 0, &mut tx, &mut deltas)
        .unwrap();
    assert!(tx.is_empty());
    assert!(deltas.is_empty());
}
