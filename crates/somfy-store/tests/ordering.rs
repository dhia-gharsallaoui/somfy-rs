//! The persist-before-transmit invariant, exercised from **outside** the crate.
//!
//! These live in `tests/` rather than a `#[cfg(test)]` module on purpose:
//! inside `somfy-store` the private field of `TransmitTicket` is reachable, so
//! an in-crate test could forge a ticket and would prove nothing about what a
//! call site can do. Out here the guarantee is the real one.

use core::cell::RefCell;
use somfy_rts::{Command, RollingCode};
use somfy_store::{
    transmit, FrameBits, RollingCodeStore, TransmitError, TransmitPlan, TransmitQueue,
    TransmitRequest, TransmitTicket,
};

const ADDRESS: u32 = 0x00_1234;

/// Everything the store and the queue do, in the order they did it, on one
/// shared timeline. Asserting on this sequence is how "commit before enqueue"
/// becomes a testable claim rather than a hope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    Load { address: u32 },
    Commit { address: u32, code: u16 },
    Enqueue { address: u32, code: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoreFailed;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueFull;

/// A store whose reads and writes can each be made to fail, recording every
/// call on the shared timeline.
struct MockStore<'a> {
    log: &'a RefCell<Vec<Event>>,
    stored: Option<RollingCode>,
    load_fails: bool,
    commit_fails: bool,
}

impl<'a> MockStore<'a> {
    fn new(log: &'a RefCell<Vec<Event>>, stored: Option<u16>) -> Self {
        MockStore {
            log,
            stored: stored.map(RollingCode),
            load_fails: false,
            commit_fails: false,
        }
    }
}

impl RollingCodeStore for MockStore<'_> {
    type Error = StoreFailed;

    fn load(&mut self, address: u32) -> Result<Option<RollingCode>, StoreFailed> {
        self.log.borrow_mut().push(Event::Load { address });
        if self.load_fails {
            return Err(StoreFailed);
        }
        Ok(self.stored)
    }

    fn commit(&mut self, address: u32, code: RollingCode) -> Result<(), StoreFailed> {
        self.log.borrow_mut().push(Event::Commit {
            address,
            code: code.0,
        });
        if self.commit_fails {
            // A failed commit must leave the persisted value alone, exactly as
            // a flash write that never landed would.
            return Err(StoreFailed);
        }
        self.stored = Some(code);
        Ok(())
    }
}

/// A queue that records what it was handed, so a test can assert on the
/// *absence* of a transmission and not merely on an error return.
struct MockQueue<'a> {
    log: &'a RefCell<Vec<Event>>,
    sent: Vec<TransmitRequest>,
    full: bool,
}

impl<'a> MockQueue<'a> {
    fn new(log: &'a RefCell<Vec<Event>>) -> Self {
        MockQueue {
            log,
            sent: Vec::new(),
            full: false,
        }
    }
}

impl TransmitQueue for MockQueue<'_> {
    type Error = QueueFull;

    fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), QueueFull> {
        let request = ticket.into_request();
        self.log.borrow_mut().push(Event::Enqueue {
            address: request.frame.address,
            code: request.frame.rolling_code,
        });
        if self.full {
            return Err(QueueFull);
        }
        self.sent.push(request);
        Ok(())
    }
}

fn plan() -> TransmitPlan {
    TransmitPlan {
        address: ADDRESS,
        command: Command::Up,
        bits: FrameBits::Bits56,
        repeats: 2,
    }
}

#[test]
fn commit_is_observed_before_enqueue() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(42));
    let mut queue = MockQueue::new(&log);

    let sent_code = transmit(&mut store, &mut queue, plan()).expect("transmit");

    assert_eq!(sent_code, 42);
    assert_eq!(
        log.into_inner(),
        vec![
            Event::Load { address: ADDRESS },
            // The *advanced* code is persisted first...
            Event::Commit {
                address: ADDRESS,
                code: 43
            },
            // ...and only then does the frame carrying 42 reach the queue.
            Event::Enqueue {
                address: ADDRESS,
                code: 42
            },
        ]
    );
}

#[test]
fn the_frame_carries_the_code_that_was_superseded_by_the_commit() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(42));
    let mut queue = MockQueue::new(&log);

    transmit(&mut store, &mut queue, plan()).expect("transmit");

    assert_eq!(queue.sent.len(), 1);
    let request = queue.sent[0];
    assert_eq!(request.frame.rolling_code, 42);
    assert_eq!(request.frame.address, ADDRESS);
    assert_eq!(request.frame.command, Command::Up);
    // Key byte is 0xA0 | low nibble of the transmitted code.
    assert_eq!(request.frame.key, 0xAA);
    assert_eq!(request.bits, FrameBits::Bits56);
    assert_eq!(request.repeats, 2);
    // The store now holds the next code, not the one just sent.
    assert_eq!(store.stored, Some(RollingCode(43)));
}

#[test]
fn a_failed_commit_transmits_nothing_at_all() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(42));
    store.commit_fails = true;
    let mut queue = MockQueue::new(&log);

    let result = transmit(&mut store, &mut queue, plan());

    assert_eq!(result, Err(TransmitError::Store(StoreFailed)));
    // The claim that matters is an absence: not a transmission with a stale
    // code, not a transmission at all.
    assert!(queue.sent.is_empty());
    assert!(!log
        .borrow()
        .iter()
        .any(|e| matches!(e, Event::Enqueue { .. })));
    // And the persisted value did not move.
    assert_eq!(store.stored, Some(RollingCode(42)));
}

#[test]
fn a_failed_load_neither_commits_nor_transmits() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(42));
    store.load_fails = true;
    let mut queue = MockQueue::new(&log);

    let result = transmit(&mut store, &mut queue, plan());

    assert_eq!(result, Err(TransmitError::Store(StoreFailed)));
    assert!(queue.sent.is_empty());
    assert_eq!(log.into_inner(), vec![Event::Load { address: ADDRESS }]);
}

#[test]
fn a_missing_record_is_reported_not_seeded() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, None);
    let mut queue = MockQueue::new(&log);

    let result = transmit(&mut store, &mut queue, plan());

    // An erased or never-written region must not masquerade as "start at 0" —
    // that replays codes the motor has already accepted.
    assert_eq!(
        result,
        Err(TransmitError::NoStoredCode { address: ADDRESS })
    );
    assert!(queue.sent.is_empty());
    assert_eq!(store.stored, None);
    assert_eq!(log.into_inner(), vec![Event::Load { address: ADDRESS }]);
}

#[test]
fn a_full_queue_leaves_the_code_committed_and_sends_nothing() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(42));
    let mut queue = MockQueue::new(&log);
    queue.full = true;

    let result = transmit(&mut store, &mut queue, plan());

    assert_eq!(result, Err(TransmitError::Queue(QueueFull)));
    assert!(queue.sent.is_empty());
    // Deliberate: committing first means a queue failure skips a code rather
    // than replaying one. Skipping forward is the direction a motor tolerates.
    assert_eq!(store.stored, Some(RollingCode(43)));
}

#[test]
fn consecutive_transmissions_advance_one_code_each() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(7));
    let mut queue = MockQueue::new(&log);

    for expected in 7..12 {
        let code = transmit(&mut store, &mut queue, plan()).expect("transmit");
        assert_eq!(code, expected);
    }

    let codes: Vec<u16> = queue.sent.iter().map(|r| r.frame.rolling_code).collect();
    assert_eq!(codes, vec![7, 8, 9, 10, 11]);
    assert_eq!(store.stored, Some(RollingCode(12)));
}

#[test]
fn the_stored_code_wraps_with_the_protocol_counter() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(u16::MAX));
    let mut queue = MockQueue::new(&log);

    let code = transmit(&mut store, &mut queue, plan()).expect("transmit");

    assert_eq!(code, u16::MAX);
    assert_eq!(store.stored, Some(RollingCode(0)));
}

#[test]
fn an_extended_command_can_be_planned_as_an_80_bit_frame() {
    let log = RefCell::new(Vec::new());
    let mut store = MockStore::new(&log, Some(3));
    let mut queue = MockQueue::new(&log);

    let plan = TransmitPlan {
        address: ADDRESS,
        command: Command::Favorite,
        bits: FrameBits::Bits80,
        repeats: 1,
    };
    transmit(&mut store, &mut queue, plan).expect("transmit");

    assert_eq!(queue.sent[0].bits, FrameBits::Bits80);
    assert_eq!(queue.sent[0].frame.command, Command::Favorite);
}
