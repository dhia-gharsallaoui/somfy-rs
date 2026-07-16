//! A day in the life: web command, mid-range stop, wall-remote
//! interference, favorite recall — the integration surface Plans 4/5
//! will drive.

use heapless::Vec;
use somfy_domain::{
    Controller, Direction, PlannedTx, Pos, ShadeCommand, ShadeConfig, StateDelta, TX_CAPACITY,
};
use somfy_rts::{Command, Frame};

#[test]
fn full_scenario_tracks_position_through_mixed_control() {
    let mut c = Controller::new();
    let id = c
        .registry
        .add_shade(ShadeConfig::new("Salon", 0x11_1111).unwrap())
        .unwrap();
    c.registry
        .shade_mut(id)
        .unwrap()
        .link_remote(0x22_2222)
        .unwrap();

    // TX_CAPACITY (= MAX_SHADES * 2 = 64) is the crate's structural worst case;
    // one `command_shade`/`tick` call plans far fewer, but the caller buffer is
    // always sized to the workspace constant so a shared buffer survives every
    // call shape (see the exported `TX_CAPACITY` contract).
    let mut tx: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
    let mut deltas: Vec<StateDelta, 32> = Vec::new();

    // 08:00 — the web UI asks for 40%. From rest (0%) that is a downward seek,
    // and the sync tick crosses no pending arrival, so exactly one Down frame.
    c.command_shade(
        id,
        ShadeCommand::GoTo(Pos::from_percent(40)),
        0,
        &mut tx,
        &mut deltas,
    )
    .unwrap();
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].command, Command::Down);

    // Position advances and the arrival plans the My stop (mid-range target of
    // an explicit seek self-stops via My, not a hard limit).
    tx.clear();
    c.tick(4_000, &mut tx, &mut deltas);
    assert_eq!(c.registry.shade(id).unwrap().pos(), Pos::from_percent(40));
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].command, Command::My);

    // 12:00 — someone presses Down on the wall remote (3 repeats heard). The
    // deduper collapses the repeats (same address+rolling_code within 2 s) to a
    // single logical Down; overheard control only tracks the estimate.
    tx.clear();
    deltas.clear();
    let press = Frame {
        key: 0xA3,
        command: Command::Down,
        rolling_code: 500,
        address: 0x22_2222,
    };
    c.on_rx_frame(&press, 100_000, &mut deltas);
    c.on_rx_frame(&press, 100_120, &mut deltas);
    c.on_rx_frame(&press, 100_240, &mut deltas);
    c.tick(103_000, &mut tx, &mut deltas); // 3 s down from 40% = 70%
    assert_eq!(c.registry.shade(id).unwrap().pos(), Pos::from_percent(70));
    assert!(tx.is_empty(), "no retransmit for overheard control");

    // They stop it with My on the wall remote (distinct rolling_code = new
    // logical event). My while moving freezes the estimate at the live 70%.
    let stop = Frame {
        key: 0xA4,
        command: Command::My,
        rolling_code: 501,
        address: 0x22_2222,
    };
    c.on_rx_frame(&stop, 103_000, &mut deltas);
    assert_eq!(c.registry.shade(id).unwrap().direction(), Direction::Idle);
    assert_eq!(c.registry.shade(id).unwrap().pos(), Pos::from_percent(70));

    // 20:00 — the app sets and recalls a favorite. SetMy is a pure state change
    // (no frame); My-while-idle recalls the favorite as an upward seek (70% ->
    // 15%), so exactly one Up frame and no stray arrival stop from the sync.
    tx.clear();
    c.command_shade(
        id,
        ShadeCommand::SetMy(Some(Pos::from_percent(15))),
        200_000,
        &mut tx,
        &mut deltas,
    )
    .unwrap();
    c.command_shade(id, ShadeCommand::My, 200_000, &mut tx, &mut deltas)
        .unwrap();
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].command, Command::Up); // 70% -> 15% is upward
    tx.clear();
    c.tick(210_000, &mut tx, &mut deltas);
    assert_eq!(c.registry.shade(id).unwrap().pos(), Pos::from_percent(15));
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].command, Command::My); // mid-range stop again
}
