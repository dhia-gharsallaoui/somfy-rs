//! `POST /api/v1/shades/{id}/calibrate` — the guided travel-time measurement.
//!
//! ## What a calibration is on this device
//!
//! Nothing here can see the shade. RTS is one-way, there is no encoder and no
//! limit-switch feedback, so the only instrument available is a person watching
//! the window and a clock. A calibration is therefore: send the traverse, let
//! the operator say when things happened, and store the intervals.
//!
//! That is not a poor substitute for something better — it is the only
//! measurement the physics permits, and it is what the requirements ask for.
//! What it replaces is worse: three shades carrying 10000/10000 that nobody had
//! ever chosen, which is what made a 25%-open command move a shade about 1% on
//! 2026-08-17.
//!
//! ## Three numbers from one traverse
//!
//! The Up leg yields the traverse time, the start lag and the slat-separation
//! band, because they are three moments of the same movement. That is what keeps
//! the dead-time and dead-band requirements from costing any extra shade travel,
//! which matters: R9 records that a sweep through the full range is not always
//! acceptable — a shade over a desk, a sleeping room, an awning in wind.
//!
//! ## The honest limit of the method
//!
//! A human tap lands a couple of hundred milliseconds after what it aims at. The
//! band is the *difference* of two taps, so that delay cancels out of it; the
//! start lag is a single tap and carries it whole. So a measured lag is worth
//! less than a measured band, and both are worth less than a traverse, which is
//! seconds long. This is why the hand-entry route of R9 exists beside this one
//! rather than being replaced by it.

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::Deserialize as DeriveDeserialize;
use somfy_domain::{CalibrationLeg, CalibrationMark};

/// Body of `POST /api/v1/shades/{id}/calibrate` — one step of a run.
///
/// # Why one route with a step in the body rather than four routes
///
/// Two reasons, and the second is the binding one.
///
/// A calibration is **one session**, not four resources. Begin, mark, finish and
/// cancel are moments in a single conversation with an operator holding a
/// stopwatch, and a run half-abandoned across separate endpoints is worse than
/// no run at all.
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
/// - `mark` — the operator reports a moment. `motionBegan` fixes the start lag;
///   `curtainMoved` fixes the dead band at this leg's closed end. Both are
///   optional. A repeated mark replaces the earlier one, so a mis-tap is
///   corrected by tapping again.
/// - `finish` — the operator reports the shade stopping. The interval since
///   `begin` is the traverse time, the marks are carved out of it rather than
///   added to it, and the estimate is re-anchored at the limit the run ended on.
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
    Mark { mark: CalibrationMarkDto },
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
    /// `upTimeMs`, and from its marks `startLagMs` and `ventBandMs`.
    Up,
    /// A full traverse toward closed, started at the open limit. Measures
    /// `downTimeMs`, and from its mark `closeBandMs`.
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

/// A moment the operator reports during a run.
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
pub enum CalibrationMarkDto {
    /// The shade began to move at all.
    MotionBegan,
    /// The curtain itself began to move, as opposed to the slats: on the Up leg
    /// the moment it starts to rise, on the Down leg the moment it reaches the
    /// sill.
    CurtainMoved,
}

impl CalibrationMarkDto {
    /// Lower onto the domain's own mark.
    pub fn to_domain(self) -> CalibrationMark {
        match self {
            CalibrationMarkDto::MotionBegan => CalibrationMark::MotionBegan,
            CalibrationMarkDto::CurtainMoved => CalibrationMark::CurtainMoved,
        }
    }
}

/// Wire discriminant for [`CalibrationStepDto`]. Unit-only, so it deserializes
/// from the bare step string with no `Content` buffer.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
enum StepTag {
    Begin,
    Mark,
    Finish,
    Cancel,
}

/// Flat wire form: the tag plus the two fields the steps between them carry.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
struct CalibrationWire {
    step: StepTag,
    leg: Option<CalibrationLegDto>,
    mark: Option<CalibrationMarkDto>,
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
            StepTag::Mark => CalibrationStepDto::Mark {
                mark: wire.mark.ok_or_else(|| D::Error::missing_field("mark"))?,
            },
            StepTag::Finish => CalibrationStepDto::Finish,
            StepTag::Cancel => CalibrationStepDto::Cancel,
        })
    }
}
