//! # somfy-store
//!
//! The rolling-code persistence seam, and the one function that is allowed to
//! start a transmission.
//!
//! ## The invariant
//!
//! An incremented rolling code must reach persistent storage **before** the
//! frame carrying it goes on the air. A crash in the gap leaves the motor
//! having seen a code the controller does not remember sending, and the pairing
//! desyncs — which costs the user a physical re-pairing procedure at the shade.
//! It is the most consequential ordering in the firmware and the least visible
//! when it is wrong.
//!
//! ## How the order is enforced
//!
//! By the type system, not by review:
//!
//! - [`TransmitQueue::enqueue`] accepts only a [`TransmitTicket`].
//! - [`TransmitTicket`] has a private field, no public constructor, and no
//!   `Clone`/`Copy`/`Default`, so nothing outside this crate can build one.
//! - [`transmit`] is the only function that mints one, and it does so only
//!   after [`RollingCodeStore::commit`] has returned `Ok`.
//!
//! A call site that tries to reach the queue without committing has nothing to
//! pass to `enqueue`, so it does not compile. The `compile_fail` doctest on
//! [`TransmitTicket`] pins that; `tests/ordering.rs` — an integration test,
//! deliberately outside the crate, where the private field is genuinely out of
//! reach — pins the runtime sequence.
//!
//! ## What else is here, and why it is here rather than in the firmware
//!
//! An implementation of [`RollingCodeStore`] over flash needs two things this
//! crate also provides: a ring of slots that spreads writes over a region with
//! a large erase unit ([`SlotLayout`], [`SectorRing`], [`newest_slot`]), and a
//! record format whose validity can be judged after a power cut ([`Record`]).
//!
//! Neither has anything to do with a particular chip, and both are the parts
//! most worth testing — a slot ring that erases the wrong sector, or a decoder
//! that accepts a half-written record, loses a rolling code and costs the user
//! a physical re-pairing procedure. So they live here, host-tested, and
//! `crates/firmware` is left with the flash I/O and nothing else.
//!
//! [`seed_if_absent`] is here for the same reason and answers the same hazard
//! from the other side: a provisioned shade needs a *first* code, that number
//! comes from a configuration record which is re-read at every boot, and a boot
//! path that wrote it every time would walk the counter backwards until the
//! motor rejected everything. Its own docs carry the argument.
//!
//! ## Why a crate of its own
//!
//! `somfy-domain` states in its own module docs that it "owns no clock, no
//! channels, no rolling codes, and no repeat counts", and its `PlannedTx`
//! carries an address, a command and a repeat *policy* for exactly that reason —
//! the count that policy resolves against belongs to whatever owns the radio. A
//! store trait and a transmit queue are both of the things the domain says it
//! does not hold, so they live here instead of eroding that boundary.
//!
//! `crates/firmware` was never an option: `esp-hal`'s build script rejects a
//! host target, so nothing in that crate can be tested on the host — and the
//! tests are the point of this seam.

#![cfg_attr(not(test), no_std)]

mod record;
mod seed;
mod slots;
mod store;
mod transmit;

pub use record::{CodeTable, Record, RecordError, TableError, MAX_CODES, RECORD_LEN};
pub use seed::{seed_if_absent, RegionState, Seeded};
pub use slots::{newest_slot, SectorRing, SlotLayout, SlotWrite};
pub use store::RollingCodeStore;
pub use transmit::{
    transmit, FrameBits, TransmitError, TransmitPlan, TransmitQueue, TransmitRequest,
    TransmitTicket,
};
