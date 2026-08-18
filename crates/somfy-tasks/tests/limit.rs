//! The per-shade command rate limit, driven through the same seam both
//! transports use.
//!
//! These run against [`somfy_tasks::StateMachine::apply`] rather than against
//! the bucket in isolation, because the two claims worth pinning are about
//! *where* it sits: that a refusal costs the store nothing, and that the
//! multi-step commands the controller drives on its own clock cannot be
//! starved by it.

use core::cell::RefCell;
use somfy_domain::{
    GroupId, ShadeCommand, ShadeConfig, ShadeId, StateDelta, DELTA_CAPACITY, MAX_SHADES,
};
use somfy_tasks::{ControlCommand, Refused, StateMachine, TxProfile, BURST, REFILL_INTERVAL_MS};

mod support;
use support::{Event, MockQueue, MockStore};

const A: u32 = 0x00_1101;
const B: u32 = 0x00_1102;

fn deltas() -> heapless::Vec<StateDelta, DELTA_CAPACITY> {
    heapless::Vec::new()
}

/// One shade at `A`. `My` is used throughout below because it is the cheapest
/// command in the domain: with the shade idle and no favourite it plans no
/// frame at all, so what these tests observe is the limiter and nothing else.
fn one_shade() -> (StateMachine, ShadeId) {
    let mut state = StateMachine::new(TxProfile::default());
    let id = state
        .registry_mut()
        .add_shade(ShadeConfig::new("A", A).unwrap())
        .unwrap();
    (state, id)
}

fn up(id: ShadeId) -> ControlCommand {
    ControlCommand::Shade {
        id,
        command: ShadeCommand::Up,
    }
}

#[test]
fn a_full_bucket_admits_exactly_burst_commands_back_to_back() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    for n in 0..BURST {
        assert!(
            state
                .apply(&mut store, &mut queue, up(id), 0, &mut deltas())
                .is_ok(),
            "command {n} of a full bucket should be admitted",
        );
    }

    let refusal = state
        .apply(&mut store, &mut queue, up(id), 0, &mut deltas())
        .unwrap_err();
    let Refused::TooSoon(too_soon) = refusal else {
        panic!("expected a rate-limit refusal, got {refusal:?}");
    };
    // The bucket refilled nothing at all, so the wait is one whole interval.
    assert_eq!(too_soon.retry_after_ms, REFILL_INTERVAL_MS);
}

#[test]
fn a_refused_command_reaches_neither_the_store_nor_the_queue() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    for _ in 0..BURST {
        let _ = state.apply(&mut store, &mut queue, up(id), 0, &mut deltas());
    }
    let sent_before = queue.sent.len();
    let log_before = log.borrow().len();

    assert!(state
        .apply(&mut store, &mut queue, up(id), 0, &mut deltas())
        .is_err());

    // The whole point: a refusal is not a cheaper commit, it is no commit. The
    // rolling code is untouched and nothing was enqueued.
    assert_eq!(queue.sent.len(), sent_before);
    assert_eq!(log.borrow().len(), log_before);
}

#[test]
fn waiting_one_interval_buys_exactly_one_command() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    for _ in 0..BURST {
        let _ = state.apply(&mut store, &mut queue, up(id), 0, &mut deltas());
    }

    // One second short of the interval is still refused, and the delay it
    // reports is the second that is missing. Seconds rather than milliseconds
    // because that is the resolution the schedule is stored at — see
    // `somfy_tasks::CommandLimiter` for the 128 bytes of DRAM that bought.
    let refusal = state
        .apply(
            &mut store,
            &mut queue,
            up(id),
            REFILL_INTERVAL_MS - 1_000,
            &mut deltas(),
        )
        .unwrap_err();
    assert_eq!(refusal, Refused::TooSoon(too_soon(1_000)));

    assert!(state
        .apply(
            &mut store,
            &mut queue,
            up(id),
            REFILL_INTERVAL_MS,
            &mut deltas()
        )
        .is_ok());
    // …and only one: the next is refused again.
    assert!(state
        .apply(
            &mut store,
            &mut queue,
            up(id),
            REFILL_INTERVAL_MS,
            &mut deltas()
        )
        .is_err());
}

#[test]
fn allowance_does_not_accumulate_past_a_full_bucket() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, id) = one_shade();

    // A week of silence is a full bucket, not a week's worth of commands.
    let week_ms = 7 * 24 * 60 * 60 * 1_000;
    for n in 0..BURST {
        assert!(
            state
                .apply(&mut store, &mut queue, up(id), week_ms, &mut deltas())
                .is_ok(),
            "command {n} after a week idle should be admitted",
        );
    }
    assert!(state
        .apply(&mut store, &mut queue, up(id), week_ms, &mut deltas())
        .is_err());
}

#[test]
fn hammering_one_shade_does_not_touch_another() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1), (B, 1)]);
    let mut queue = MockQueue::new(&log);
    let mut state = StateMachine::new(TxProfile::default());
    let a = state
        .registry_mut()
        .add_shade(ShadeConfig::new("A", A).unwrap())
        .unwrap();
    let b = state
        .registry_mut()
        .add_shade(ShadeConfig::new("B", B).unwrap())
        .unwrap();

    for _ in 0..BURST + 8 {
        let _ = state.apply(&mut store, &mut queue, up(a), 0, &mut deltas());
    }

    // The anti-lockout property, and the reason this is not one device-wide
    // bucket: whatever is happening at `a`, the operator can still move `b`.
    assert!(state
        .apply(&mut store, &mut queue, up(b), 0, &mut deltas())
        .is_ok());
}

#[test]
fn a_group_command_charges_every_member_once() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1), (B, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, a, _b, group) = two_in_a_group();

    let fan_out = ControlCommand::Group {
        id: group,
        command: ShadeCommand::Up,
    };
    for _ in 0..BURST {
        assert!(state
            .apply(&mut store, &mut queue, fan_out, 0, &mut deltas())
            .is_ok());
    }

    // Each member has now spent its whole bucket, so neither the group nor
    // either member individually may move.
    assert!(state
        .apply(&mut store, &mut queue, fan_out, 0, &mut deltas())
        .is_err());
    assert!(state
        .apply(&mut store, &mut queue, up(a), 0, &mut deltas())
        .is_err());
}

#[test]
fn a_group_is_refused_whole_rather_than_half_moved() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1), (B, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, a, _b, group) = two_in_a_group();

    // Exhaust one member only.
    for _ in 0..BURST {
        let _ = state.apply(&mut store, &mut queue, up(a), 0, &mut deltas());
    }
    let sent_before = queue.sent.len();

    let refused = state.apply(
        &mut store,
        &mut queue,
        ControlCommand::Group {
            id: group,
            command: ShadeCommand::Up,
        },
        0,
        &mut deltas(),
    );

    assert!(refused.is_err());
    // The same standard the domain already holds for a member whose frame width
    // cannot carry the command: nothing moved, so there is nothing to inspect
    // shade by shade afterwards.
    assert_eq!(queue.sent.len(), sent_before);
}

/// **The load-bearing one.** A vent is Down, a whole travel time of waiting, Up,
/// then stop — and the second and third legs are planned by the clock rather
/// than by a client. A limiter that could refuse them would leave the shade
/// closed with no vent coming.
#[test]
fn a_vents_later_legs_survive_an_exhausted_bucket() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);

    let mut state = StateMachine::new(TxProfile::default());
    let mut config = ShadeConfig::new("A", A).unwrap();
    config.vent_band_ms = 1_500;
    let id = state.registry_mut().add_shade(config).unwrap();
    let travel = &state
        .registry()
        .shade(id)
        .expect("the shade was just added")
        .config;
    let down_time_ms = u64::from(travel.down_time_ms);
    let start_lag_ms = u64::from(travel.start_lag_ms);
    let vent_band_ms = u64::from(travel.vent_band_ms);

    // Spend every token but one *before* starting the vent. Ordering matters:
    // every command abandons whatever preceded it, so draining the bucket
    // afterwards would cancel the very sequence under test.
    for _ in 0..BURST - 1 {
        assert!(state
            .apply(&mut store, &mut queue, up(id), 0, &mut deltas())
            .is_ok());
    }
    assert!(state
        .apply(
            &mut store,
            &mut queue,
            ControlCommand::Shade {
                id,
                command: ShadeCommand::Vent
            },
            0,
            &mut deltas()
        )
        .is_ok());
    assert!(
        state
            .apply(&mut store, &mut queue, up(id), 0, &mut deltas())
            .is_err(),
        "the bucket must be empty for this test to mean anything",
    );

    let sent_after_start = queue.sent.len();

    // Leg two: the motor has reached its closed limit, so the Up goes out.
    let at_limit = down_time_ms;
    let dispatch = state.tick(&mut store, &mut queue, at_limit, &mut deltas());
    assert_eq!(dispatch.sent, 1, "the vent's Up leg was starved");

    // Leg three: the slats have separated, so the stop goes out.
    let separated = at_limit + start_lag_ms + vent_band_ms;
    let dispatch = state.tick(&mut store, &mut queue, separated, &mut deltas());
    assert_eq!(dispatch.sent, 1, "the vent's stop was starved");

    assert_eq!(queue.sent.len(), sent_after_start + 2);
}

/// A shade the registry does not have is the domain's refusal, not the
/// limiter's — it costs no flash, so there is nothing to rate limit, and
/// reporting it as a rate limit would send the operator looking for traffic
/// that does not exist.
#[test]
fn a_shade_outside_the_registry_is_refused_by_the_domain() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(A, 1)]);
    let mut queue = MockQueue::new(&log);
    let (mut state, _) = one_shade();

    let beyond = ShadeId(MAX_SHADES as u8 + 4);
    for _ in 0..BURST + 4 {
        let refusal = state
            .apply(&mut store, &mut queue, up(beyond), 0, &mut deltas())
            .unwrap_err();
        assert!(
            matches!(refusal, Refused::Domain(_)),
            "a nonexistent shade should never be reported as a rate limit",
        );
    }
    assert!(log.borrow().is_empty());
}

fn two_in_a_group() -> (StateMachine, ShadeId, ShadeId, GroupId) {
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

fn too_soon(retry_after_ms: u64) -> somfy_tasks::TooSoon {
    somfy_tasks::TooSoon { retry_after_ms }
}

// Keeps `Event` in use for the two tests that read the store log by length.
const _: fn() -> Option<Event> = || None;
