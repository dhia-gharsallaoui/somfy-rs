//! The real bounded channel, driven from outside the crate.
//!
//! These live in `tests/` for the same reason `somfy-store`'s ordering suite
//! does: inside the crate the private sender is reachable, so an in-crate test
//! would prove nothing about what `crates/firmware` can do. Out here the only
//! way to put a request in the channel is the one the firmware has.

use core::cell::RefCell;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use somfy_rts::{Command, RollingCode};
use somfy_store::{transmit, FrameBits, RollingCodeStore, TransmitError, TransmitPlan};
use somfy_tasks::{QueueFull, TransmitChannel};

mod support;
use support::MockStore;

const ADDRESS: u32 = 0x00_1234;

fn plan() -> TransmitPlan {
    TransmitPlan {
        address: ADDRESS,
        command: Command::Up,
        bits: FrameBits::Bits56,
        repeats: 2,
    }
}

/// The channel's producer end is reachable only through
/// [`somfy_store::transmit`], and what comes out the other end is the frame
/// whose code was committed on the way in.
#[test]
fn a_committed_transmission_arrives_at_the_radio_end() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(ADDRESS, 42)]);
    let channel: TransmitChannel<NoopRawMutex, 4> = TransmitChannel::new();
    let mut queue = channel.queue();

    let code = transmit(&mut store, &mut queue, plan()).expect("transmit");

    assert_eq!(code, 42);
    let request = channel
        .requests()
        .try_receive()
        .expect("one request queued");
    assert_eq!(request.frame.rolling_code, 42);
    assert_eq!(request.frame.address, ADDRESS);
    assert_eq!(request.bits, FrameBits::Bits56);
    assert_eq!(request.repeats, 2);
    // The persisted value moved on before the request existed.
    assert_eq!(store.code(ADDRESS), Some(43));
}

/// A failed commit must leave the channel empty. This is the same claim
/// `somfy-store` makes against a mock queue, re-made against the real one —
/// the queue that a broken implementation would have let through.
#[test]
fn a_failed_commit_puts_nothing_in_the_channel() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(ADDRESS, 42)]);
    store.commit_fails_for = Some(ADDRESS);
    let channel: TransmitChannel<NoopRawMutex, 4> = TransmitChannel::new();
    let mut queue = channel.queue();

    let result = transmit(&mut store, &mut queue, plan());

    assert!(matches!(result, Err(TransmitError::Store(_))));
    assert!(channel.requests().try_receive().is_none());
    assert_eq!(store.code(ADDRESS), Some(42));
}

/// A full channel refuses rather than blocking, and the code it refused stays
/// committed — the safe direction, because a skipped code is one a motor
/// accepts and a replayed one is not.
#[test]
fn a_full_channel_refuses_and_leaves_the_code_advanced() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(ADDRESS, 1)]);
    let channel: TransmitChannel<NoopRawMutex, 2> = TransmitChannel::new();
    let mut queue = channel.queue();

    assert_eq!(transmit(&mut store, &mut queue, plan()), Ok(1));
    assert_eq!(transmit(&mut store, &mut queue, plan()), Ok(2));
    let overflowed = transmit(&mut store, &mut queue, plan());

    assert_eq!(overflowed, Err(TransmitError::Queue(QueueFull)));
    // Committed anyway: three commits happened, only two frames were queued.
    assert_eq!(store.code(ADDRESS), Some(4));
    let requests = channel.requests();
    assert_eq!(requests.try_receive().unwrap().frame.rolling_code, 1);
    assert_eq!(requests.try_receive().unwrap().frame.rolling_code, 2);
    assert!(requests.try_receive().is_none());
}

/// Draining makes room again: the queue is a backlog, not a fuse.
#[test]
fn a_drained_channel_accepts_again() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(ADDRESS, 1)]);
    let channel: TransmitChannel<NoopRawMutex, 1> = TransmitChannel::new();
    let mut queue = channel.queue();

    assert_eq!(transmit(&mut store, &mut queue, plan()), Ok(1));
    assert_eq!(
        transmit(&mut store, &mut queue, plan()),
        Err(TransmitError::Queue(QueueFull))
    );

    let requests = channel.requests();
    assert_eq!(requests.try_receive().unwrap().frame.rolling_code, 1);
    assert_eq!(transmit(&mut store, &mut queue, plan()), Ok(3));
    assert_eq!(requests.try_receive().unwrap().frame.rolling_code, 3);
}

/// Two handles onto the same channel still feed one radio task. Nothing about
/// the seam depends on there being a single producer — only on every producer
/// having had to commit first.
#[test]
fn several_handles_feed_the_same_channel_in_order() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[(ADDRESS, 10)]);
    let channel: TransmitChannel<NoopRawMutex, 4> = TransmitChannel::new();
    let mut first = channel.queue();
    let mut second = channel.queue();

    transmit(&mut store, &mut first, plan()).expect("transmit");
    transmit(&mut store, &mut second, plan()).expect("transmit");

    let requests = channel.requests();
    assert_eq!(requests.try_receive().unwrap().frame.rolling_code, 10);
    assert_eq!(requests.try_receive().unwrap().frame.rolling_code, 11);
}

/// A store with no record for the address is reported, not seeded — and so
/// nothing reaches the radio. The store trait says this; asserting it through
/// the real channel is what makes it a property of the wiring rather than of a
/// mock.
#[test]
fn an_unprovisioned_address_never_reaches_the_radio() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, &[]);
    let channel: TransmitChannel<NoopRawMutex, 4> = TransmitChannel::new();
    let mut queue = channel.queue();

    let result = transmit(&mut store, &mut queue, plan());

    assert_eq!(
        result,
        Err(TransmitError::NoStoredCode { address: ADDRESS })
    );
    assert!(channel.requests().try_receive().is_none());
    // And an explicit seed is what makes it transmittable — visibly, in the
    // caller, never inside the store.
    store.commit(ADDRESS, RollingCode(1)).expect("seed");
    assert_eq!(transmit(&mut store, &mut queue, plan()), Ok(1));
}
