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
use somfy_domain::{ShadeConfig, ShadeKind, TiltMode};

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
/// # No dead-band field, deliberately
///
/// R8 records that the first ~4 s of Up travel off the closed limit separates
/// the slats without lifting the curtain — about 13% of a 30 s traverse — and
/// requires the model to carry a per-direction dead band at the closed limit.
/// **No such field is added here, because the spec says the mechanism is not
/// yet established and the two candidates need opposite handling.**
///
/// If it is a *mechanical* dead band, the estimator must subtract that time
/// from lift travel: it happens during every ordinary traverse. If instead
/// these motors honour the reference's `euromode`, where burst length selects
/// the operation, then the same seconds are a *separate command's* effect that
/// a full-length burst never produces, and subtracting them from lift travel
/// would corrupt every estimate. The number would be identical and what the
/// estimator must do with it is opposite, so a field named for it is not a
/// neutral placeholder — it is an unresolved question with somewhere to write
/// a value.
///
/// Waiting costs nothing structurally, and this is worth stating because it is
/// what makes deferring safe rather than merely cautious:
///
/// - the euromode answer needs **no new field at all** — `tiltMode` already
///   carries `EuroMode` as a discriminant, and what is missing is tilt
///   *commands*, which this generation deliberately does not have;
/// - the mechanical answer needs **one additive field** on this DTO and on
///   [`crate::ShadeDto`], which is exactly the shape `addressOrigin` was added
///   in and breaks nothing.
///
/// The spec calls the deciding test cheap — send a short Up burst from fully
/// closed and watch whether it stops after the slats separate or runs to the
/// limit — and notes it transmits at a real motor, so it is the owner's to run.
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
        if let Some(up_time_ms) = self.up_time_ms {
            next.up_time_ms = up_time_ms;
        }
        if let Some(down_time_ms) = self.down_time_ms {
            next.down_time_ms = down_time_ms;
        }
        if let Some(tilt_time_ms) = self.tilt_time_ms {
            next.tilt_time_ms = tilt_time_ms;
        }

        // Checked on the *result*, not on the patch, so that a body setting only
        // `upTimeMs` to zero is refused even though it says nothing about the
        // other direction.
        checked_lift_times(next.up_time_ms, next.down_time_ms)?;
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
