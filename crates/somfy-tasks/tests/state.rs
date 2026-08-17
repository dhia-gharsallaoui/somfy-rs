//! The state loop, driven with a mock store and a recording queue.
//!
//! `somfy-store` already pins that its own `transmit` helper commits before it
//! enqueues. What these pin is one level up: that the **task** reaches a queue
//! only through that helper, that a store failure on one shade does not cost
//! another shade its frame, and that an overheard frame never transmits.

use core::cell::RefCell;
use somfy_domain::{
    Direction, DomainError, GroupId, Pos, ShadeCommand, ShadeConfig, ShadeId, StateDelta,
    DELTA_CAPACITY, PAIR_REPEATS,
};
use somfy_rts::{Command, Frame};
use somfy_store::{FrameBits, TransmitError};
use somfy_tasks::{ControlCommand, StateMachine, TxProfile};

mod support;
use support::{Event, MockQueue, MockStore, StoreFailed};

const A: u32 = 0x00_1101;
const B: u32 = 0x00_1102;

fn deltas() -> heapless::Vec<StateDelta, DELTA_CAPACITY> {
    heapless::Vec::new()
}

/// One shade at address `A`, with a code already in the store.
fn one_shade() -> (StateMachine, ShadeId) {
    let mut state = StateMachine::new(TxProfile::default());
    let id = state
        .registry_mut()
        .add_shade(ShadeConfig::new("A", A).unwrap())
        .unwrap();
    (state, id)
}

/// Two shades in one group.
fn two_shades() -> (StateMachine, ShadeId, ShadeId, GroupId) {
    let mut state = StateMachine::new(TxProfile::default());
    let a = state
        .registry_mut()
        .add_shade(ShadeConfig::new("A", A).unwrap())
        .unwrap();
    let b = state
        .registry_mut()
        .add_shade(ShadeConfig::new("B", B).unwrap())
        .unwrap();
    let group = state.registry_mut().add_group("All").unwrap();
    state.registry_mut().group_add_shade(group, a).unwrap();
    state.registry_mut().group_add_shade(group, b).unwrap();
    (state, a, b, group)
}

#[test]
fn a_command_commits_before_it_enqueues() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    let dispatch = state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .expect("known shade");

    assert_eq!(dispatch.planned, 1);
    assert_eq!(dispatch.sent, 1);
    assert!(dispatch.first_error.is_none());
    assert_eq!(
        log.into_inner(),
        std::vec![
            Event::Load { address: A },
            // The advanced code reaches the store...
            Event::Commit {
                address: A,
                code: 43
            },
            // ...before the frame carrying 42 reaches the radio.
            Event::Enqueue {
                address: A,
                code: 42
            },
        ]
    );
}

/// A shade provisioned at a caller-chosen id commands, dispatches and reports
/// exactly as one the registry numbered itself. Pinned here because the state
/// task is the layer between the registry's ids and the radio, and it routes
/// entirely by [`ShadeId`] — a sparse id must not be mistaken for an unknown
/// one, which is the failure that would look like a shade that never moves.
#[test]
fn a_shade_at_a_chosen_id_commands_like_any_other() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);

    let mut state = StateMachine::new(TxProfile::default());
    // 31 is the last slot the registry has, and the id a positional registry
    // would only reach with 32 shades provisioned.
    let id = state
        .registry_mut()
        .add_shade_with_id(ShadeId(31), ShadeConfig::new("A", A).unwrap())
        .expect("the last slot is in range and free");

    let dispatch = state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .expect("a chosen id is a known shade");

    assert_eq!(dispatch.planned, 1);
    assert_eq!(dispatch.sent, 1);
    assert!(dispatch.first_error.is_none());
    assert_eq!(queue.sent.len(), 1);
}

#[test]
fn a_queued_frame_carries_the_command_and_the_profile() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 7)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Up,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(queue.sent.len(), 1);
    let request = queue.sent[0];
    assert_eq!(request.frame.address, A);
    assert_eq!(request.frame.command, Command::Up);
    assert_eq!(request.frame.rolling_code, 7);
    assert_eq!(request.bits, FrameBits::Bits56);
    assert_eq!(request.repeats, somfy_tasks::TxProfile::default().repeats);
}

#[test]
fn the_profile_chooses_the_frame_width_and_repeat_count() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let mut state = StateMachine::new(TxProfile {
        bits: FrameBits::Bits80,
        repeats: 6,
    });
    let id = state
        .registry_mut()
        .add_shade(ShadeConfig::new("A", A).unwrap())
        .unwrap();

    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(queue.sent[0].bits, FrameBits::Bits80);
    assert_eq!(queue.sent[0].repeats, 6);
}

#[test]
fn a_failed_commit_transmits_nothing_and_is_reported() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    store.commit_fails_for = Some(A);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    let dispatch = state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(dispatch.planned, 1);
    assert_eq!(dispatch.sent, 0);
    assert_eq!(
        dispatch.first_error,
        Some(TransmitError::Store(StoreFailed))
    );
    // The claim that matters is an absence.
    assert!(queue.sent.is_empty());
    assert!(!log
        .borrow()
        .iter()
        .any(|event| matches!(event, Event::Enqueue { .. })));
    assert_eq!(store.code(A), Some(42));
}

#[test]
fn an_unprovisioned_shade_is_reported_not_seeded() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    let dispatch = state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(dispatch.sent, 0);
    assert_eq!(
        dispatch.first_error,
        Some(TransmitError::NoStoredCode { address: A })
    );
    assert!(queue.sent.is_empty());
}

/// A group command is where "report, do not propagate" earns its keep: one
/// shade's unreadable store must not stop the other shade moving.
#[test]
fn one_shades_store_failure_does_not_cost_the_other_shade_its_frame() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 5), (B, 9)]);
    store.load_fails_for = Some(A);
    let mut queue = MockQueue::new(&log);
    let (mut state, _a, _b, group) = two_shades();

    let dispatch = state
        .command_group(
            &mut store,
            &mut queue,
            group,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .expect("known group");

    assert_eq!(dispatch.planned, 2);
    assert_eq!(dispatch.sent, 1);
    assert_eq!(
        dispatch.first_error,
        Some(TransmitError::Store(StoreFailed))
    );
    assert_eq!(queue.sent.len(), 1);
    assert_eq!(queue.sent[0].frame.address, B);
    assert_eq!(queue.sent[0].frame.rolling_code, 9);
    assert_eq!(store.code(B), Some(10));
}

/// An overheard frame drives the estimate and must never be retransmitted —
/// retransmitting would double-drive the motor. The signature already makes it
/// impossible (no store, no queue); this pins the behaviour that goes with it.
#[test]
fn an_overheard_frame_moves_the_estimate_and_queues_nothing() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, _id) = one_shade();

    let frame = Frame {
        key: 0xA1,
        command: Command::Down,
        rolling_code: 7,
        address: A,
    };
    let mut seen = deltas();
    state.on_rx_frame(&frame, 0, &mut seen);

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].direction, Direction::Down);
    assert!(queue.sent.is_empty());
    // Nothing touched the store either: an observation is not a transmission.
    assert!(log.borrow().is_empty());
    assert_eq!(store.code(A), Some(42));

    // And the shade really is moving: a later tick reports progress.
    let mut later = deltas();
    let dispatch = state.tick(&mut store, &mut queue, 5_000, &mut later);
    assert_eq!(dispatch.planned, 0);
    assert_eq!(later[0].pos, Pos::from_percent(50));
}

/// An arrival stop is planned by `tick`, not by a command, and it is just as
/// much a transmission — so it goes through the same commit-then-enqueue path.
#[test]
fn an_arrival_stop_planned_by_tick_commits_before_it_enqueues() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 100)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    // A mid-range seek: the motor will not self-stop, so the controller owes it
    // a stop frame once the estimate says it has arrived.
    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::GoTo(Pos::from_percent(50)),
            0,
            &mut deltas(),
        )
        .unwrap();
    assert_eq!(queue.sent.len(), 1);
    log.borrow_mut().clear();

    // Default travel time is 10 s, so half travel is 5 s.
    let dispatch = state.tick(&mut store, &mut queue, 5_000, &mut deltas());

    assert_eq!(dispatch.planned, 1);
    assert_eq!(dispatch.sent, 1);
    assert_eq!(
        log.borrow().clone(),
        std::vec![
            Event::Load { address: A },
            Event::Commit {
                address: A,
                code: 102
            },
            Event::Enqueue {
                address: A,
                code: 101
            },
        ]
    );
    assert_eq!(queue.sent[1].frame.command, Command::My);
}

/// One press is one code, and consecutive presses step it by one each.
#[test]
fn consecutive_commands_advance_one_code_each() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 7)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    for at in 0..4u64 {
        state
            .command_shade(
                &mut store,
                &mut queue,
                id,
                ShadeCommand::Down,
                at * 60_000,
                &mut deltas(),
            )
            .unwrap();
    }

    assert_eq!(queue.codes(), std::vec![7, 8, 9, 10]);
    assert_eq!(store.code(A), Some(11));
}

#[test]
fn an_unknown_shade_is_a_domain_error_and_touches_neither_store_nor_queue() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, _) = one_shade();

    let result = state.command_shade(
        &mut store,
        &mut queue,
        ShadeId(31),
        ShadeCommand::Down,
        0,
        &mut deltas(),
    );

    assert_eq!(result.unwrap_err(), DomainError::NotFound);
    assert!(log.borrow().is_empty());
    assert!(queue.sent.is_empty());
}

/// A refused queue still leaves the code committed. Pinned at the task level
/// because this is the one failure mode where the store and the radio
/// deliberately disagree, and a future edit that "fixed" it by rolling the
/// counter back would replay a code the motor may already have accepted.
#[test]
fn a_refused_queue_leaves_the_code_advanced() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    queue.full = true;
    let (mut state, id) = one_shade();

    let dispatch = state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(dispatch.sent, 0);
    assert!(matches!(
        dispatch.first_error,
        Some(TransmitError::Queue(_))
    ));
    assert!(queue.sent.is_empty());
    assert_eq!(store.code(A), Some(43));
}

/// `apply` is what the state task actually calls; it must route both kinds of
/// command through the same commit-then-enqueue path as the direct methods.
#[test]
fn apply_routes_shade_and_group_commands() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1), (B, 50)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, a, _b, group) = two_shades();

    let one = state
        .apply(
            &mut store,
            &mut queue,
            ControlCommand::Shade {
                id: a,
                command: ShadeCommand::Down,
            },
            0,
            &mut deltas(),
        )
        .unwrap();
    assert_eq!((one.planned, one.sent), (1, 1));

    let both = state
        .apply(
            &mut store,
            &mut queue,
            ControlCommand::Group {
                id: group,
                command: ShadeCommand::Up,
            },
            60_000,
            &mut deltas(),
        )
        .unwrap();
    assert_eq!((both.planned, both.sent), (2, 2));

    let addresses: std::vec::Vec<u32> = queue.sent.iter().map(|r| r.frame.address).collect();
    assert_eq!(addresses, std::vec![A, A, B]);
    assert_eq!(store.code(A), Some(3));
    assert_eq!(store.code(B), Some(51));
}

/// A command for a group that does not exist reaches neither store nor queue.
#[test]
fn apply_reports_an_unknown_group() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, _) = one_shade();

    let result = state.apply(
        &mut store,
        &mut queue,
        ControlCommand::Group {
            id: GroupId(7),
            command: ShadeCommand::Up,
        },
        0,
        &mut deltas(),
    );

    assert_eq!(result.unwrap_err(), DomainError::NotFound);
    assert!(queue.sent.is_empty());
    assert!(log.borrow().is_empty());
}

/// **A known divergence, pinned rather than fixed.**
///
/// The domain updates a shade's motion model when it handles the command, and
/// the frames it plans are dispatched afterwards. So a store failure leaves the
/// estimate believing the shade is moving when nothing was transmitted and the
/// motor never heard anything.
///
/// It is not fixable here: `somfy_domain::Controller` has no way to undo a
/// command, and adding one would mean either pre-flighting the store (which
/// cannot promise the commit will succeed) or making the domain aware of
/// transmission outcomes — a change to the Plan 2 boundary that says the domain
/// owns no rolling codes. The recovery that exists is the same one that covers
/// a motor that simply did not hear: an overheard frame or the next command
/// re-anchors the estimate.
///
/// This test exists so the behaviour is a recorded fact rather than a surprise,
/// and so that a future fix has something to flip.
#[test]
fn a_failed_commit_still_moves_the_estimate() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    store.commit_fails_for = Some(A);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    let mut seen = deltas();
    let dispatch = state
        .command_shade(&mut store, &mut queue, id, ShadeCommand::Down, 0, &mut seen)
        .unwrap();

    assert_eq!(dispatch.sent, 0, "nothing was transmitted");
    assert!(queue.sent.is_empty());
    // ...and yet:
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].direction,
        Direction::Down,
        "the estimate moved anyway — this is the divergence"
    );
}

/// A full radio queue stops the dispatch; a store failure does not.
///
/// The asymmetry is the whole point, and it is a flash-wear and deaf-window
/// property rather than a cosmetic one. A full group plans 64 frames against a
/// queue four deep, so carrying on past the first refusal would mean ~60 more
/// commits — each a ring scan, a flash write and one erase in sixteen — every
/// one of them burning a rolling code for a frame nobody will send, and every
/// one of them holding the core with interrupts disabled while the receiver
/// hears nothing.
#[test]
fn a_full_queue_stops_the_dispatch_instead_of_committing_the_rest() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 5), (B, 9)]);
    let mut queue = MockQueue::new(&log);
    queue.full = true;
    let (mut state, _a, _b, group) = two_shades();

    let dispatch = state
        .command_group(
            &mut store,
            &mut queue,
            group,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(dispatch.planned, 2);
    assert_eq!(dispatch.sent, 0);
    assert!(matches!(
        dispatch.first_error,
        Some(TransmitError::Queue(_))
    ));
    // The second shade's record was never even read, let alone written.
    assert_eq!(store.code(A), Some(6), "the first shade's code advanced");
    assert_eq!(store.code(B), Some(9), "the second shade's did not");
    assert!(
        !log.borrow()
            .iter()
            .any(|event| matches!(event, Event::Load { address } if *address == B)),
        "the dispatch must stop, not carry on paying for refusals"
    );
}

/// A store failure, by contrast, is about one address and must not stop the
/// dispatch — already covered above, restated here as the other half of the
/// same rule so the pair cannot drift apart.
#[test]
fn a_store_failure_does_not_stop_the_dispatch() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 5), (B, 9)]);
    store.commit_fails_for = Some(A);
    let mut queue = MockQueue::new(&log);
    let (mut state, _a, _b, group) = two_shades();

    let dispatch = state
        .command_group(
            &mut store,
            &mut queue,
            group,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(dispatch.sent, 1);
    assert_eq!(queue.sent.len(), 1);
    assert_eq!(queue.sent[0].frame.address, B);
}

// ---------------------------------------------------------------------------
// Repeat policy: where the domain's `Repeats` meets the controller's profile
// ---------------------------------------------------------------------------

/// One shade at address `A`, on a controller configured to repeat generously —
/// the setting an operator reaches for on a weak RF path.
fn one_shade_repeating(repeats: u8) -> (StateMachine, ShadeId) {
    let mut state = StateMachine::new(TxProfile {
        bits: FrameBits::Bits56,
        repeats,
    });
    let id = state
        .registry_mut()
        .add_shade(ShadeConfig::new("A", A).unwrap())
        .unwrap();
    (state, id)
}

/// An ordinary command takes the controller's configured repeat count. The
/// domain plans a policy, not a number, so this is where the number comes from.
#[test]
fn an_ordinary_command_takes_the_configured_repeat_count() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade_repeating(5);

    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Down,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(queue.sent.len(), 1);
    assert_eq!(queue.sent[0].repeats, 5);
}

/// A pairing burst does **not**. The repeat count of a `Prog` frame is how long
/// the PROG button was held, and a long hold removes the remote from the motor
/// instead of adding it — so a controller configured to transmit generously
/// would unpair every shade it was asked to pair.
#[test]
fn a_pairing_burst_ignores_a_generous_profile() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade_repeating(9);

    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Pair,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(queue.sent.len(), 1);
    assert_eq!(queue.sent[0].frame.command, Command::Prog);
    assert_eq!(queue.sent[0].repeats, PAIR_REPEATS);
}

/// A pairing frame still commits its rolling code before it reaches the queue,
/// like every other transmission: a motor being paired stores the code it is
/// taught, so a `Prog` sent from an uncommitted counter would leave the store
/// behind the motor from the very first frame.
#[test]
fn a_pairing_frame_commits_before_it_enqueues() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Pair,
            0,
            &mut deltas(),
        )
        .unwrap();

    assert_eq!(
        log.into_inner(),
        std::vec![
            Event::Load { address: A },
            Event::Commit {
                address: A,
                code: 43
            },
            Event::Enqueue {
                address: A,
                code: 42
            },
        ]
    );
}

/// The pairing frame is one the radio can actually put on the air.
///
/// `somfy_rts::encode56` refuses the extended commands, and the radio loop
/// checks that *before* it keys anything — so a command it refuses produces a
/// `RadioEvent::Unencodable` and no carrier at all. For a pairing button that
/// is the "appears to work, does nothing" failure with a motor at the far end
/// of it, and nothing else in the path would notice.
#[test]
fn a_pairing_frame_encodes_into_a_frame_the_radio_can_send() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 42)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    state
        .command_shade(
            &mut store,
            &mut queue,
            id,
            ShadeCommand::Pair,
            0,
            &mut deltas(),
        )
        .unwrap();

    let request = &queue.sent[0];
    assert_eq!(request.bits, FrameBits::Bits56);
    let bytes = somfy_rts::encode56(&request.frame).expect("Prog fits a 56-bit frame");
    let decoded = somfy_rts::decode56(&bytes).expect("and decodes back");
    assert_eq!(decoded.command, Command::Prog);
    assert_eq!(decoded.address, A);
    assert_eq!(decoded.rolling_code, 42);
}
