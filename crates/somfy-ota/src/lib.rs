//! What an over-the-air update decides, separated from what it touches.
//!
//! The firmware half of an update is flash I/O: erase a sector, write a page,
//! read it back, rewrite one 32-byte record in `otadata`. None of that is where
//! an update goes wrong. What goes wrong is *judgement* — accepting a file that
//! is not a firmware image for this chip, marking a slot bootable before the
//! bytes are all there, rolling a good release back because the router
//! rebooted, or failing to roll a bad one back at all. Every one of those is a
//! decision over values, so every one of them is in here, on the host side of
//! the fence, with tests.
//!
//! Three pieces, and they are deliberately unaware of each other:
//!
//! - [`image`] — the ESP-IDF application image format, as a streaming
//!   verifier. It answers "is this a firmware image, for this chip, complete
//!   and internally consistent?" without ever holding more than a few dozen
//!   bytes.
//! - [`verdict`] — what a running image should do about the `otadata` state it
//!   booted with. Three inputs, three outcomes, and the interesting one is the
//!   ambiguity it exists to sidestep.
//! - [`selftest`] — which checks may fail an update and which may only be
//!   reported. That distinction is the whole of the router-reboot problem.
//!
//! # What is deliberately *not* here
//!
//! The flash, the partition table and `otadata` itself. `esp-bootloader-esp-idf`
//! already models those and this firmware already depends on it; a second
//! partition-table reader is exactly the kind of duplicate this workspace's
//! rules exist to prevent. `crates/firmware/src/ota.rs` is the thin layer that
//! joins the two, and it is thin on purpose.

#![no_std]

pub mod image;
pub mod selftest;
pub mod verdict;

pub use image::{Accepted, Chip, ImageError, Verifier};
pub use selftest::{Leg, LegState, SelfTest, SelfTestOutcome};
pub use verdict::{BootVerdict, ImageState, RollBackReason};
