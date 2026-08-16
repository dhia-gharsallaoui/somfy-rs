//! Test doubles shared by the state and queue suites.
//!
//! The store and the queue write to **one** timeline, which is the only way
//! "commit happened before enqueue" becomes something a test can assert rather
//! than something a reviewer has to notice. Same construction as
//! `somfy-store/tests/ordering.rs`, one level up: there it pins
//! `somfy_store::transmit`, here it pins the task body that calls it, which is
//! where a future edit could plausibly get the order wrong.

#![allow(dead_code)] // each suite uses a different subset

use core::cell::RefCell;
use somfy_rts::RollingCode;
use somfy_store::{RollingCodeStore, TransmitQueue, TransmitRequest, TransmitTicket};

/// Everything the store and the queue did, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Load { address: u32 },
    Commit { address: u32, code: u16 },
    Enqueue { address: u32, code: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreFailed;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueRefused;

/// A store holding a code per address, with per-address failure injection.
///
/// Per-address rather than a single slot because the interesting task-level
/// case is a *group* command: several shades, one of whose stored codes cannot
/// be read, and the others must still move.
pub struct MockStore<'a> {
    log: &'a RefCell<Vec<Event>>,
    pub stored: Vec<(u32, RollingCode)>,
    pub load_fails_for: Option<u32>,
    pub commit_fails_for: Option<u32>,
}

impl<'a> MockStore<'a> {
    pub fn new(log: &'a RefCell<Vec<Event>>, stored: &[(u32, u16)]) -> Self {
        MockStore {
            log,
            stored: stored
                .iter()
                .map(|(address, code)| (*address, RollingCode(*code)))
                .collect(),
            load_fails_for: None,
            commit_fails_for: None,
        }
    }

    pub fn code(&self, address: u32) -> Option<u16> {
        self.stored
            .iter()
            .find(|(a, _)| *a == address)
            .map(|(_, code)| code.0)
    }
}

impl RollingCodeStore for MockStore<'_> {
    type Error = StoreFailed;

    fn load(&mut self, address: u32) -> Result<Option<RollingCode>, StoreFailed> {
        self.log.borrow_mut().push(Event::Load { address });
        if self.load_fails_for == Some(address) {
            return Err(StoreFailed);
        }
        Ok(self
            .stored
            .iter()
            .find(|(a, _)| *a == address)
            .map(|(_, code)| *code))
    }

    fn commit(&mut self, address: u32, code: RollingCode) -> Result<(), StoreFailed> {
        self.log.borrow_mut().push(Event::Commit {
            address,
            code: code.0,
        });
        if self.commit_fails_for == Some(address) {
            // A failed write leaves the persisted value where it was, exactly
            // as a flash write that never landed would.
            return Err(StoreFailed);
        }
        match self.stored.iter_mut().find(|(a, _)| *a == address) {
            Some(slot) => slot.1 = code,
            None => self.stored.push((address, code)),
        }
        Ok(())
    }
}

/// A queue that records what it was handed, so a test can assert on the
/// *absence* of a transmission rather than only on an error return.
pub struct MockQueue<'a> {
    log: &'a RefCell<Vec<Event>>,
    pub sent: Vec<TransmitRequest>,
    pub full: bool,
}

impl<'a> MockQueue<'a> {
    pub fn new(log: &'a RefCell<Vec<Event>>) -> Self {
        MockQueue {
            log,
            sent: Vec::new(),
            full: false,
        }
    }

    pub fn codes(&self) -> Vec<u16> {
        self.sent.iter().map(|r| r.frame.rolling_code).collect()
    }
}

impl TransmitQueue for MockQueue<'_> {
    type Error = QueueRefused;

    fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), QueueRefused> {
        let request = ticket.into_request();
        self.log.borrow_mut().push(Event::Enqueue {
            address: request.frame.address,
            code: request.frame.rolling_code,
        });
        if self.full {
            return Err(QueueRefused);
        }
        self.sent.push(request);
        Ok(())
    }
}
