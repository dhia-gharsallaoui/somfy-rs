//! # somfy-domain
//!
//! `no_std` domain model for somfy-rs: shade/group/room registries, the
//! travel-time position estimator (port of the C++ `SomfyShade::checkMovement`
//! dead-reckoning), command orchestration (commands in → [`PlannedTx`] radio
//! work + [`StateDelta`] events out), and overheard-remote tracking.
//!
//! ## Intentional deviations from the C++ reference
//!
//! All documented in the design spec:
//! - Positions are fixed-point ([`Pos`], hundredths of a percent) instead of
//!   floats — deterministic, no accumulated rounding drift.
//! - Sun/wind/dry-contact handling is deferred post-1.0 (spec §1.3); the tilt
//!   estimator axis exists but full tilt command plumbing lands with the API
//!   layer that exposes it.
//! - The mid-range arrival stop is scheduled only for explicit position seeks
//!   (`GoTo`/favorite recall), never for `Step` — the C++ `settingPos` analog:
//!   Step targets and native motor moves self-stop, so no `My` is planned on
//!   their arrival (see [`Shade::tick`]).
//! - Overheard `My`-while-idle recalls the favorite immediately; the C++ defers
//!   ~500 ms to disambiguate a My *recall* from a My *set* on the physical
//!   button. The domain sees an already-decoded command, so no such wait
//!   applies (see [`Shade::apply_overheard`]).
//! - **`My`-while-idle favorite recall always *simulates*.** The C++ DEFAULT
//!   (the `simMy` flag is off, Somfy.cpp:2880-2887) sends a raw `My`/Favorite
//!   frame and lets the motor recall its own HARDWARE-stored favorite; this
//!   crate always simulates the move from the software `my_pos` instead. So
//!   `My`-while-idle with `my_pos == None` is a **no-op** here, whereas the C++
//!   default would still transmit and drive the shade to a position the software
//!   cannot predict. Reconciling this — a raw-`My` passthrough command or a
//!   `simMy` config bit that toggles simulate-vs-passthrough — is a Plan 4
//!   decision item (see [`Shade::handle`]'s `My` arm).
//!
//! ## Ownership boundaries
//!
//! This crate owns no clock, no channels, and no rolling codes: callers inject
//! `now_ms` and drain the output buffers; rolling-code state stays in the
//! radio/persistence layer (`somfy_rts::RollingCode`).
//!
//! The TX buffer contract is **per-call**: caller buffers are sized to
//! [`TX_CAPACITY`] (the structural worst case of one call — a full group
//! commanded at once) and must be drained between calls, not accumulated across
//! them. A shade's internal buffer plans at most two frames per call (a
//! sync-crossed arrival stop plus the command's own frame).

#![cfg_attr(not(test), no_std)]

mod controller;
mod motion;
mod registry;
mod shade;
mod tilt;
mod types;

pub use controller::{Controller, StateDelta, DELTA_CAPACITY, RX_DEDUPE_WINDOW_MS, TX_CAPACITY};
pub use motion::{Motion, MotionSnapshot};
pub use registry::{GroupId, Registry, RoomId, ShadeId, MAX_GROUPS, MAX_ROOMS, MAX_SHADES};
pub use shade::{PlannedTx, Shade, ShadeCommand};
pub use tilt::tilt_first;
pub use types::{Direction, DomainError, Pos, ShadeConfig, ShadeKind, TiltMode};
