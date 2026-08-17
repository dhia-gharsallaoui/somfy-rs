//! # somfy-domain
//!
//! `no_std` domain model for somfy-rs: shade/group/room registries, the
//! travel-time position estimator (dead-reckoning a shade's position from
//! elapsed motion time — the same technique deployed motors use internally,
//! since they have no absolute position sensor), command orchestration
//! (commands in → [`PlannedTx`] radio work + [`StateDelta`] events out), and
//! overheard-remote tracking.
//!
//! ## Intentional deviations from deployed firmware behaviour
//!
//! All documented in the design spec:
//! - Positions are fixed-point ([`Pos`], hundredths of a percent) instead of
//!   floats — deterministic, no accumulated rounding drift.
//! - Sun/wind/dry-contact handling is deferred post-1.0 (spec §1.3); the tilt
//!   estimator axis exists but full tilt command plumbing lands with the API
//!   layer that exposes it.
//! - The mid-range arrival stop is scheduled only for explicit position seeks
//!   (`GoTo`/favorite recall), tracked via a position-seek-in-progress flag —
//!   never for `Step`: Step targets and native motor moves self-stop, so no
//!   `My` is planned on their arrival (see [`Shade::tick`]).
//! - Overheard `My`-while-idle recalls the favorite immediately; a physical
//!   remote's My button defers ~500 ms before committing to a recall, so it
//!   can tell a tap (recall) apart from a press-and-hold (set a new
//!   favorite). The domain sees an already-decoded command, not raw button
//!   timing, so no such wait applies (see [`Shade::apply_overheard`]).
//! - **`My`-while-idle favorite recall always *simulates*.** Deployed
//!   firmware ships with passthrough as the default: it sends a raw
//!   `My`/Favorite frame and lets the motor recall its own HARDWARE-stored
//!   favorite; this crate always simulates the move from the software
//!   `my_pos` instead. So `My`-while-idle with `my_pos == None` is a
//!   **no-op** here, whereas the deployed-firmware default would still
//!   transmit and drive the shade to a position the software cannot predict.
//!   Reconciling this — a raw-`My` passthrough command or a config bit that
//!   toggles simulate-vs-passthrough — is a Plan 4 decision item (see
//!   [`Shade::handle`]'s `My` arm).
//!
//! ## Pairing
//!
//! [`RemoteIdentity`] gives this controller a virtual-remote identity of its
//! own, derived from the device-unique half of its MAC, and allocates a
//! per-shade address from it; [`ShadeCommand::Pair`] is what teaches a motor
//! one. Both live in `pairing.rs`, whose docs carry the argument for why a
//! controller sharing another controller's identity is a controller that will
//! stop working.
//!
//! [`PairingState`] is the third piece, and it is deliberately **not** a claim
//! that pairing succeeded — nothing in a one-way protocol can make that claim.
//! It records whether a person reported the shade working, which is what
//! [`Shade::confirm_pairing`] stores and what everything downstream gates an
//! announcement on.
//!
//! ## Ownership boundaries
//!
//! This crate owns no clock, no channels, no rolling codes, and no repeat
//! counts: callers inject `now_ms` and drain the output buffers; rolling-code
//! state stays in the radio/persistence layer (`somfy_rts::RollingCode`), and a
//! [`PlannedTx`] carries a [`Repeats`] *policy* that the radio layer resolves
//! against its own configured count.
//!
//! The TX buffer contract is **per-call**: caller buffers are sized to
//! [`TX_CAPACITY`] (the structural worst case of one call — a full group
//! commanded at once) and must be drained between calls, not accumulated across
//! them. A shade's internal buffer plans at most two frames per call (a
//! sync-crossed arrival stop plus the command's own frame).

#![cfg_attr(not(test), no_std)]

mod controller;
mod motion;
mod pairing;
mod registry;
mod shade;
mod tilt;
mod types;

pub use controller::{
    Controller, StateDelta, DELTA_CAPACITY, MAX_ACTIVITIES, RX_DEDUPE_WINDOW_MS, TX_CAPACITY,
};
pub use motion::{Motion, MotionSnapshot};
pub use pairing::{
    allocate_if_absent, allocate_with, AllocateError, Allocated, RemoteIdentity, PAIR_REPEATS,
};
pub use registry::{GroupId, Registry, RoomId, ShadeId, MAX_GROUPS, MAX_ROOMS, MAX_SHADES};
pub use shade::{
    Activity, Calibrating, CalibrationLeg, CalibrationMark, CalibrationOutcome, PlannedTx, Repeats,
    Shade, ShadeCommand, MAX_LINKED_REMOTES, MAX_TRAVEL_TIME_MS, ROUTE_VIA_LIMIT_RAW, STOP_REPEATS,
};
pub use tilt::tilt_first;
pub use types::{
    round_dead_band_ms, round_start_lag_ms, CalibrationSource, Direction, DomainError, FrameWidth,
    PairingState, Pos, RadioProtocol, ShadeConfig, ShadeKind, TiltMode, TravelProfile,
    DEAD_BAND_RESOLUTION_MS, FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS, FACTORY_UP_TIME_MS,
    MAX_DEAD_BAND_MS, MAX_START_LAG_MS, START_LAG_RESOLUTION_MS,
};
