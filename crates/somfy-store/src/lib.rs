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
//! ## Why a crate of its own
//!
//! `somfy-domain` states in its own module docs that it "owns no clock, no
//! channels, and no rolling codes", and its `PlannedTx` carries only an address
//! and a command for exactly that reason. A store trait and a transmit queue
//! are both of the things the domain says it does not hold, so they live here
//! instead of eroding that boundary.
//!
//! `crates/firmware` was never an option: `esp-hal`'s build script rejects a
//! host target, so nothing in that crate can be tested on the host — and the
//! tests are the point of this seam.

#![cfg_attr(not(test), no_std)]

mod slots;
mod store;
mod transmit;

pub use slots::{newest_slot, SlotLayout, SlotWrite};
pub use store::RollingCodeStore;
pub use transmit::{
    transmit, FrameBits, TransmitError, TransmitPlan, TransmitQueue, TransmitRequest,
    TransmitTicket,
};
