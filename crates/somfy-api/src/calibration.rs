//! `POST /api/v1/shades/{id}/calibrate` — the guided travel-time measurement.
//!
//! ## What a calibration is on this device
//!
//! Nothing here can see the shade. RTS is one-way, there is no encoder and no
//! limit-switch feedback, so the only instrument available is a person watching
//! the window and a clock. A calibration is therefore: send the traverse, let
//! the operator say when it stopped, and store the interval.
//!
//! That is not a poor substitute for something better — it is the only
//! measurement the physics permits, and it is what the requirements ask for.
//! What it replaces is worse: three shades carrying 10000/10000 that nobody had
//! ever chosen, which is what made a 25%-open command move a shade about 1% on
//! 2026-08-17.
//!
//! ## Why one press beats a stopwatch, rather than merely being easier
//!
//! One end of this measurement is the device's own clock — it knows exactly when
//! it put the frame on the air. So only the *stop* carries the operator's
//! reaction delay, where timing the same traverse with a wristwatch carries it
//! at both ends. That asymmetry is the whole reason to offer this flow beside
//! the hand-entry fields rather than instead of them.
//!
//! ## What this flow deliberately does not measure
//!
//! The start lag and the two slat dead bands were measured here until
//! 2026-08-19, through a `mark` step the operator pressed as the shade passed
//! each moment. They are entered by hand now, through
//! `PATCH /api/v1/shades/{id}`, which R9 of the position-accuracy spec already
//! required as a MUST — so nothing became unmeasurable; two presses per leg
//! went away. The reasoning is recorded in that spec's implementation-status
//! section under `docs/specs/`.
//!
//! The short version is that those two marks measured worst the thing they
//! existed for. Each was a *single* press against a moment a fraction of a
//! second wide, so each carried a whole reaction delay against the interval it
//! defined — the opposite of the traverse, where the same delay is a fraction of
//! a percent of a half-minute. Both are also small enough to watch and type: the
//! slats visibly separate.

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::Deserialize as DeriveDeserialize;
use somfy_domain::CalibrationLeg;

/// Body of `POST /api/v1/shades/{id}/calibrate` — one step of a run.
///
/// # Why one route with a step in the body rather than three routes
///
/// Two reasons, and the second is the binding one.
///
/// A calibration is **one session**, not three resources. Begin, finish and
/// cancel are moments in a single conversation with an operator watching a
/// window, and a run half-abandoned across separate endpoints is worse than no
/// run at all.
///
/// And the device hand-rolls its HTTP routing on a router that is a type per
/// route wrapping the previous one. Every new *path shape* deepens a
/// monomorphised call chain the firmware measures against a fixed stack budget;
/// a route sharing an existing shape costs nothing, which `crates/firmware/src/heap.rs`
/// establishes by measurement rather than by hope. `/calibrate` joins the same
/// `(&str, id, &str)` family as `/pair`, `/command` and `/confirm-pairing`.
///
/// # What each step means
///
/// - `begin` — send the traverse and start the clock. `leg` says which
///   direction. **The caller is responsible for the shade being at the opposite
///   limit first**; nothing on the device can check that, because checking would
///   mean trusting the position estimate this run exists to replace.
/// - `finish` — the operator reports the shade stopping. The interval since
///   `begin` is the traverse time, and the estimate is re-anchored at the limit
///   the run ended on.
/// - `cancel` — abandon the run and store nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
// The wire form is a flat, internally-tagged object parsed by the manual
// `Deserialize` below, exactly as `CommandDto` is and for the same
// allocator-free reason. ts-rs cannot infer that, so it is told: tag on `step`,
// camelCase tag values.
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        tag = "step",
        rename_all = "camelCase"
    )
)]
pub enum CalibrationStepDto {
    Begin { leg: CalibrationLegDto },
    Finish,
    Cancel,
}

/// Which traverse a calibration run times.
///
/// The two are measured **independently and never mirrored**. On the estate this
/// came from, up takes 30 s and down 27 s, because closing is gravity-assisted —
/// a routine that timed one direction and doubled it would be wrong by that 10%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, DeriveDeserialize, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum CalibrationLegDto {
    /// A full traverse toward open, started at the closed limit. Measures
    /// `upTimeMs`.
    Up,
    /// A full traverse toward closed, started at the open limit. Measures
    /// `downTimeMs`.
    Down,
}

impl CalibrationLegDto {
    /// Lower onto the domain's own leg.
    pub fn to_domain(self) -> CalibrationLeg {
        match self {
            CalibrationLegDto::Up => CalibrationLeg::Up,
            CalibrationLegDto::Down => CalibrationLeg::Down,
        }
    }
}

/// Wire discriminant for [`CalibrationStepDto`]. Unit-only, so it deserializes
/// from the bare step string with no `Content` buffer.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
enum StepTag {
    Begin,
    Finish,
    Cancel,
}

/// Flat wire form: the tag plus the one field a step between them carries.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
struct CalibrationWire {
    step: StepTag,
    leg: Option<CalibrationLegDto>,
}

impl<'de> Deserialize<'de> for CalibrationStepDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CalibrationWire::deserialize(deserializer)?;
        Ok(match wire.step {
            // Required rather than defaulted: a `begin` with no leg is a
            // malformed request, and guessing a direction would drive a shade
            // the wrong way across its whole range.
            StepTag::Begin => CalibrationStepDto::Begin {
                leg: wire.leg.ok_or_else(|| D::Error::missing_field("leg"))?,
            },
            StepTag::Finish => CalibrationStepDto::Finish,
            StepTag::Cancel => CalibrationStepDto::Cancel,
        })
    }
}
