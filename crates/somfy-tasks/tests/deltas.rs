//! The delta channel's subscriber slots, which are what bound the web server's
//! WebSockets.
//!
//! # Why this is a test and not a comment
//!
//! `crates/firmware/src/api/events.rs` admits a WebSocket by *taking a
//! subscription*, and refuses one with `503` when there is none to take. That
//! makes the admission check and the resource the same object — there is no
//! counter to keep in step with reality, and no path that takes one without the
//! other. The whole design rests on two properties of `PubSubChannel` that this
//! crate does not own:
//!
//! 1. `subscriber()` **refuses** past [`DELTA_SUBSCRIBERS`] rather than
//!    over-subscribing or panicking. If it panicked, a sixth browser tab would
//!    reboot the device — which is a worse version of the lockout the whole
//!    design is written against.
//! 2. A dropped subscription **frees its slot**. If it did not, the server
//!    would refuse every WebSocket for the rest of the boot after enough tabs
//!    had been opened and closed, and the only cure would be a power cycle.
//!
//! Neither is checkable on the device without hardware, and both are the kind
//! of upstream behaviour that changes in a minor release. They are pinned here,
//! against the real channel type the firmware uses, at the real capacity.
//!
//! # What this deliberately does not test
//!
//! That REST keeps working while every WebSocket slot is taken. That is not a
//! property of this channel — it is `api::REST_TASKS_RESERVED = HTTP_TASKS −
//! WS_MAX`, asserted at compile time in the firmware, where a violation stops
//! the build.

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use somfy_domain::{Direction, Pos, ShadeId, StateDelta};
use somfy_tasks::{DeltaChannel, DELTA_SUBSCRIBERS};

/// The firmware instantiates this channel with `CriticalSectionRawMutex`, and
/// these use the no-op one: `critical-section` has no implementation on a host
/// and linking one in would be testing that crate rather than this property.
/// Nothing below depends on the mutex — subscriber accounting is the same code
/// either way, and every one of these tests is single-threaded.
type Channel = DeltaChannel<NoopRawMutex>;

fn delta(id: u8) -> StateDelta {
    StateDelta {
        id: ShadeId(id),
        pos: Pos::from_percent(50),
        tilt_pos: Pos::ZERO,
        direction: Direction::Idle,
    }
}

/// Exactly [`DELTA_SUBSCRIBERS`] may subscribe, and the next is refused rather
/// than admitted or fatal.
///
/// The firmware turns that refusal into `503 Service Unavailable` with a
/// `Retry-After`. A panic here instead would reach the firmware's panic handler,
/// which reboots the board — so "refuses" rather than "aborts" is load-bearing.
#[test]
fn the_channel_admits_exactly_its_capacity_and_then_refuses() {
    let channel = Channel::new();
    let held: heapless::Vec<_, DELTA_SUBSCRIBERS> = (0..DELTA_SUBSCRIBERS)
        .map(|n| {
            channel
                .subscriber()
                .unwrap_or_else(|error| panic!("subscriber {n} refused: {error:?}"))
        })
        .collect();
    assert_eq!(held.len(), DELTA_SUBSCRIBERS);
    assert!(
        channel.subscriber().is_err(),
        "one past capacity must be refused, not admitted",
    );
}

/// A slot comes back when its subscription is dropped.
///
/// This is what makes a closed browser tab, a torn-down socket and a task whose
/// future was dropped all return capacity without anything having to notice
/// they happened. Every one of those paths ends in `Drop` and in nothing else.
#[test]
fn dropping_a_subscription_returns_its_slot() {
    let channel = Channel::new();
    let mut held: heapless::Vec<_, DELTA_SUBSCRIBERS> = heapless::Vec::new();
    for _ in 0..DELTA_SUBSCRIBERS {
        let _ = held.push(channel.subscriber().expect("within capacity"));
    }
    assert!(channel.subscriber().is_err(), "full, as set up");

    drop(held.pop().expect("one to drop"));
    let recovered = channel.subscriber();
    assert!(
        recovered.is_ok(),
        "a dropped subscription must free its slot, or a device would refuse \
         every websocket for the rest of the boot once enough tabs had been \
         opened and closed",
    );

    // And the recovered slot is a real one: full again with it held.
    drop(recovered);
    let _again = channel.subscriber().expect("still exactly one free");
    assert!(channel.subscriber().is_err());
}

/// The firmware's own division: one slot for the broker session, the rest for
/// WebSockets.
///
/// Pinned here so that lowering [`DELTA_SUBSCRIBERS`] shows up as a failing test
/// naming the reason, rather than only as the firmware's compile-time assertion
/// in a build somebody may not run.
#[test]
fn there_is_a_slot_for_the_broker_and_at_least_one_websocket() {
    // A `const` block, because the claim is about one constant and clippy is
    // right that a run-time assertion on that is the wrong shape — this fails
    // the build rather than a test run.
    const {
        assert!(
            DELTA_SUBSCRIBERS >= 2,
            "one slot belongs to the broker session for the life of the program, \
             so anything less than two means no websocket can ever be served on \
             a board that also has a broker",
        )
    };
}

/// The publisher never waits on a subscriber, which is the property that keeps
/// a browser that has stopped reading from slowing the state task down.
///
/// A subscriber that never reads is filled and then *lagged* — it is told how
/// many it missed and carries on — while the publish itself returns
/// immediately every time.
#[test]
fn a_subscriber_that_never_reads_does_not_block_the_publisher() {
    let channel = Channel::new();
    let mut ignored = channel.subscriber().expect("within capacity");
    let publisher = channel.immediate_publisher();

    // Far more than the queue holds. Each of these returns at once; a channel
    // that parked the publisher would hang this test rather than fail it.
    for id in 0..200u8 {
        publisher.publish_immediate(delta(id % 32));
    }

    // The reader is told it fell behind rather than silently given stale data.
    let first = embassy_futures::block_on(ignored.next_message());
    assert!(
        matches!(first, embassy_sync::pubsub::WaitResult::Lagged(_)),
        "a subscriber that fell behind must be told so; got {first:?}",
    );
}
