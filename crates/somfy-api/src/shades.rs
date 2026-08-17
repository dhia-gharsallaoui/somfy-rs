//! `POST /api/v1/shades` — the body that adds a shade, and the validation that
//! decides whether it may.
//!
//! ## What the client does not send
//!
//! **The address.** It is allocated by the device from
//! [`somfy_domain::RemoteIdentity`], probing past anything the table already
//! holds. A client-chosen address is the two-controllers-one-identity failure
//! that module exists to end, offered as a form field.
//!
//! **The id.** It is the registry slot, and the device owns it.
//!
//! **`addressOrigin`.** Derived from the allocated address; see
//! [`crate::AddressOrigin`].
//!
//! **Positions.** A shade that has just been created has never been moved and
//! has never been *heard* moving, so any position here would be a guess. The
//! domain starts it fully open ([`somfy_domain::Shade::new`]) and the first
//! Open or Close corrects it against a physical limit.
//!
//! ## Why the numeric `kind`/`tiltMode` rather than string unions
//!
//! [`crate::ShadeDto`] carries them as the discriminants deployed devices emit,
//! because backups do. Accepting strings here and returning numbers there would
//! make create and read asymmetric — the UI would need a second mapping, and
//! the two could disagree. One representation, validated on the way in.

use serde::Deserialize as DeriveDeserialize;
use somfy_domain::{
    round_dead_band_ms, round_start_lag_ms, CalibrationSource, ShadeConfig, ShadeKind, TiltMode,
    FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS, FACTORY_UP_TIME_MS,
};

use crate::ApiErrorCode;

/// Longest shade name, in **bytes**. Mirrors `heapless::String<32>` in
/// [`somfy_domain::ShadeConfig`], which is the actual limit; this constant
/// exists so the check can be made *before* the string is moved into a
/// fixed-capacity buffer and can therefore report
/// [`ApiErrorCode::NameTooLong`] instead of a parse failure.
pub const NAME_MAX_BYTES: usize = 32;

/// Capacity of the inbound `name` field, deliberately larger than
/// [`NAME_MAX_BYTES`].
///
/// A `heapless::String<32>` refuses a 33-byte name inside serde, and a serde
/// error is a malformed-JSON answer — the wrong diagnosis for a user who simply
/// typed a long name. Twice the limit is enough headroom to catch every
/// realistic overshoot as [`ApiErrorCode::NameTooLong`], while still bounding
/// the buffer: something an order of magnitude past the limit is not a slip of
/// the keyboard and may fail as a parse error.
const NAME_INBOX_BYTES: usize = NAME_MAX_BYTES * 2;

/// Body of `POST /api/v1/shades`.
///
/// Deserialize-only, for the same reason [`crate::CommandDto`] is: this is a
/// payload the firmware *receives*. What it sends back is a
/// [`crate::ShadeDto`], which is a different and larger thing — it carries the
/// address the device just allocated and the id it assigned.
#[derive(Debug, Clone, PartialEq, Eq, DeriveDeserialize)]
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
pub struct CreateShadeDto {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub name: heapless::String<NAME_INBOX_BYTES>,
    pub kind: u8,
    pub tilt_mode: u8,
    pub up_time_ms: u32,
    pub down_time_ms: u32,
    pub tilt_time_ms: u32,
}

impl CreateShadeDto {
    /// Validate the request and lower it onto the domain config, at the address
    /// the device allocated.
    ///
    /// The address is a parameter rather than a field precisely because the
    /// caller — the firmware — is the only party entitled to choose it.
    ///
    /// Every rule here already exists further in — in `somfy-domain`'s
    /// `ShadeConfig::new` or in `somfy-config`'s zero-travel-time check —
    /// restated at the boundary so a request is refused with a code the UI can
    /// translate rather than failing deeper with one it cannot. The single
    /// deliberate addition is [`ApiErrorCode::NameEmpty`]; see that variant for
    /// why it is stricter here than in the domain.
    pub fn to_config(&self, address: u32) -> Result<ShadeConfig, ApiErrorCode> {
        let name = checked_name(self.name.as_str())?;
        let kind = checked_kind(self.kind)?;
        let tilt_mode = checked_tilt_mode(self.tilt_mode)?;
        checked_lift_times(self.up_time_ms, self.down_time_ms)?;

        // `ShadeConfig::new` is the address authority and re-checks the name
        // against its own capacity; the two mapped errors are the same two
        // judgements, reported with the same codes.
        let mut config = ShadeConfig::new(name, address).map_err(|_| {
            if name.len() > NAME_MAX_BYTES {
                ApiErrorCode::NameTooLong
            } else {
                ApiErrorCode::InvalidAddress
            }
        })?;
        config.kind = kind;
        config.tilt_mode = tilt_mode;
        config.up_time_ms = self.up_time_ms;
        config.down_time_ms = self.down_time_ms;
        config.tilt_time_ms = self.tilt_time_ms;
        config.up_time_source = supplied_source(self.up_time_ms, FACTORY_UP_TIME_MS);
        config.down_time_source = supplied_source(self.down_time_ms, FACTORY_DOWN_TIME_MS);
        config.tilt_time_source = supplied_source(self.tilt_time_ms, FACTORY_TILT_TIME_MS);
        Ok(config)
    }
}

/// Body of `PATCH /api/v1/shades/{id}` — editing a shade that already exists.
///
/// # Why this endpoint has to exist
///
/// Travel times were settable only at creation, so correcting one meant
/// deleting the shade and adding it again — and a re-added shade gets a new
/// address, which costs a walk to the window and a fresh pairing. That is an
/// absurd price for a typo, and R9 makes it a MUST that it not be paid:
/// "Automatic calibration (R2) MUST NOT be the only way to set travel times."
///
/// Automatic measurement is not a substitute either, and R9 says why: a sweep
/// moves the shade through its full range twice per direction, which is not
/// always acceptable — over a desk, in a sleeping room, on an awning in wind —
/// and a sweep with nothing to check itself against is a sweep nobody can
/// catch being wrong.
///
/// # Why one partial PATCH rather than sub-resources
///
/// The alternative considered was `PUT /shades/{id}/travel-times` or a
/// resource per value. Against it:
///
/// - **A calibration is one measurement session.** Somebody with a stopwatch
///   times the shade up and down and saves both. Splitting that across requests
///   makes an atomic edit into several that can half-fail, and a shade left
///   with a new up time and an old down time is worse than one with neither.
/// - **The device hand-rolls its HTTP routing.** Every extra path is code in a
///   firmware router with no framework under it; one method on an existing path
///   is close to free.
///
/// # What it deliberately will not change
///
/// - **`address`**, and therefore `addressOrigin`. The device allocates it so
///   that no other controller transmits at it, and editing it would silently
///   break the pairing a motor already learned.
/// - **`id`**, which is the registry slot and the Home Assistant entity's
///   identity.
/// - **`myPosition`**, which looks like a setting and is not: the favourite
///   lives *in the motor*, and changing it means transmitting. It stays a
///   [`crate::CommandDto`] (`setMy`), where the transmission is visible.
/// - **Positions.** They are an estimate the device maintains, not an input.
///
/// # Absent means "leave it alone", and there is no way to say "clear it"
///
/// Every field is `Option`, and a missing one is left unchanged. A JSON `null`
/// is indistinguishable from absent here and means the same thing. That is a
/// real limitation of the shape and it costs nothing, because **no field this
/// PATCH accepts is meaningfully nullable** — the one field on a shade that
/// can be cleared is `myPosition`, which is excluded above for its own
/// reasons.
///
/// # The dead-band fields, and why they are here now
///
/// This DTO used to carry a note saying there was deliberately **no** dead-band
/// field, because two mechanisms could produce the reported symptom — a
/// mechanical band during ordinary travel, or a distinct tilt operation selected
/// by burst length — and the estimator would have to do opposite things with an
/// identical number.
///
/// The spec settled it by elimination on 2026-08-17: this project's ordinary
/// commands are three-frame bursts and these motors complete full traverses from
/// them, which cannot be true of a motor that reads a short burst as a slat
/// operation. So it is mechanical, and there are three fields rather than one,
/// because two intervals of a traverse move nothing and they are not the same
/// interval:
///
/// - `startLagMs` — before the motor moves at all, at the start of any move.
/// - `ventBandMs` — separating the slats when leaving the closed limit upward.
///   Also where [`crate::CommandDto::Vent`] stops, which is the only thing that
///   command needs to know.
/// - `closeBandMs` — compressing them at the end of a full close.
///
/// All three are **parts of** the travel times rather than additions to them, so
/// setting one does not silently change what a stored `upTimeMs` means. Each is
/// rounded onto the resolution its measurement actually has — see
/// [`somfy_domain::round_start_lag_ms`] — and what comes back from a subsequent
/// `GET` is the rounded value, because that is the number the device is running.
///
/// They are settable by hand for the same reason the travel times are, and R9
/// makes it a MUST: a sweep moves the shade through its full range, which is not
/// always acceptable, and a measurement with nothing to check itself against is
/// one nobody can catch being wrong.
#[derive(Debug, Clone, PartialEq, Eq, Default, DeriveDeserialize)]
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
pub struct PatchShadeDto {
    #[cfg_attr(feature = "ts", ts(optional, type = "string"))]
    pub name: Option<heapless::String<NAME_INBOX_BYTES>>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub kind: Option<u8>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tilt_mode: Option<u8>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub up_time_ms: Option<u32>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub down_time_ms: Option<u32>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tilt_time_ms: Option<u32>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub start_lag_ms: Option<u32>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub vent_band_ms: Option<u32>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub close_band_ms: Option<u32>,
}

impl PatchShadeDto {
    /// Produce the shade's new configuration.
    ///
    /// Returns a fresh [`ShadeConfig`] rather than mutating in place, so a
    /// request that fails validation half-way cannot leave a shade holding
    /// some of the changes — the caller either gets a whole valid config or an
    /// error, and never a partially-applied one.
    ///
    /// Validation is the *same functions* [`CreateShadeDto::to_config`] calls,
    /// not a parallel set: a name the create endpoint refuses must not be
    /// reachable by creating a shade and then renaming it.
    pub fn apply(&self, current: &ShadeConfig) -> Result<ShadeConfig, ApiErrorCode> {
        let mut next = current.clone();

        if let Some(name) = &self.name {
            let name = checked_name(name.as_str())?;
            next.name = heapless::String::new();
            // Cannot fail: `checked_name` has just bounded it by the capacity.
            next.name
                .push_str(name)
                .map_err(|_| ApiErrorCode::NameTooLong)?;
        }
        if let Some(kind) = self.kind {
            next.kind = checked_kind(kind)?;
        }
        if let Some(tilt_mode) = self.tilt_mode {
            next.tilt_mode = checked_tilt_mode(tilt_mode)?;
        }
        // A travel time this endpoint sets is one a person typed, which is a
        // different fact from the number itself and the one R7 exists to keep:
        // three shades carrying 10000/10000 that nobody had ever chosen is what
        // made a 25%-open command move a shade 1%. So the provenance moves with
        // the value, and only for the field actually present in the body.
        if let Some(up_time_ms) = self.up_time_ms {
            next.up_time_ms = up_time_ms;
            next.up_time_source = supplied_source(up_time_ms, FACTORY_UP_TIME_MS);
        }
        if let Some(down_time_ms) = self.down_time_ms {
            next.down_time_ms = down_time_ms;
            next.down_time_source = supplied_source(down_time_ms, FACTORY_DOWN_TIME_MS);
        }
        if let Some(tilt_time_ms) = self.tilt_time_ms {
            next.tilt_time_ms = tilt_time_ms;
            next.tilt_time_source = supplied_source(tilt_time_ms, FACTORY_TILT_TIME_MS);
        }
        if let Some(start_lag_ms) = self.start_lag_ms {
            next.start_lag_ms =
                round_start_lag_ms(start_lag_ms).ok_or(ApiErrorCode::InvalidDeadBand)?;
        }
        if let Some(vent_band_ms) = self.vent_band_ms {
            next.vent_band_ms =
                round_dead_band_ms(vent_band_ms).ok_or(ApiErrorCode::InvalidDeadBand)?;
        }
        if let Some(close_band_ms) = self.close_band_ms {
            next.close_band_ms =
                round_dead_band_ms(close_band_ms).ok_or(ApiErrorCode::InvalidDeadBand)?;
        }

        // Checked on the *result*, not on the patch, so that a body setting only
        // `upTimeMs` to zero is refused even though it says nothing about the
        // other direction — and so that a new band is weighed against the travel
        // time already stored, and a new travel time against the bands already
        // stored, whichever half of the pair the request happens to carry.
        checked_lift_times(next.up_time_ms, next.down_time_ms)?;
        next.checked_bands()
            .map_err(|_| ApiErrorCode::InvalidDeadBand)?;
        Ok(next)
    }
}

// ---------------------------------------------------------------------------
// Shared validators
//
// One copy each, called by both `to_config` and `apply`. The rule they enforce
// is that the two endpoints agree: anything reachable by creating a shade and
// then patching it must have been reachable by creating it directly.
// ---------------------------------------------------------------------------

/// The provenance to record for a travel time that arrived in a request body.
///
/// **A value equal to the factory default is recorded as a factory default,
/// whichever endpoint it came in through.** Both forms in the UI are pre-filled
/// — create with the defaults, patch with what is already stored — so leaving a
/// field alone and submitting it is not evidence anybody chose the number in it.
/// That is R7's ruling applied at the point a value enters: "a value that is
/// merely *plausible* is not evidence anybody chose it", and three shades
/// carrying identical untouched defaults is what it was raised to a MUST for.
///
/// It costs one false negative: an operator who measures a shade and gets
/// exactly 10.0 s is invited to calibrate something already right. The two
/// errors are not symmetric, and the guided calibration records
/// [`somfy_domain::CalibrationSource::Measured`] regardless of the number it
/// lands on, so the honest path out of the false negative exists.
///
/// Applied identically by both endpoints, like every other rule in this section:
/// anything reachable by creating a shade and then patching it must have been
/// reachable by creating it directly.
fn supplied_source(value_ms: u32, factory_ms: u32) -> CalibrationSource {
    if value_ms == factory_ms {
        CalibrationSource::FactoryDefault
    } else {
        CalibrationSource::OperatorSupplied
    }
}

fn checked_name(name: &str) -> Result<&str, ApiErrorCode> {
    if name.is_empty() {
        return Err(ApiErrorCode::NameEmpty);
    }
    if name.len() > NAME_MAX_BYTES {
        return Err(ApiErrorCode::NameTooLong);
    }
    Ok(name)
}

fn checked_kind(raw: u8) -> Result<ShadeKind, ApiErrorCode> {
    ShadeKind::from_raw(raw).ok_or(ApiErrorCode::InvalidKind)
}

fn checked_tilt_mode(raw: u8) -> Result<TiltMode, ApiErrorCode> {
    TiltMode::from_raw(raw).ok_or(ApiErrorCode::InvalidTiltMode)
}

/// Only the two lift times. `tiltTimeMs` may be zero: a shade with no tilt has
/// no tilt travel to time, which is what every tilt-less row in a real table
/// looks like.
fn checked_lift_times(up_time_ms: u32, down_time_ms: u32) -> Result<(), ApiErrorCode> {
    if up_time_ms == 0 || down_time_ms == 0 {
        return Err(ApiErrorCode::TravelTimeZero);
    }
    Ok(())
}
