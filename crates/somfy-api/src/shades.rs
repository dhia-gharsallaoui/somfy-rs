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
        let name = self.name.as_str();
        if name.is_empty() {
            return Err(ApiErrorCode::NameEmpty);
        }
        if name.len() > NAME_MAX_BYTES {
            return Err(ApiErrorCode::NameTooLong);
        }

        let kind = ShadeKind::from_raw(self.kind).ok_or(ApiErrorCode::InvalidKind)?;
        let tilt_mode = TiltMode::from_raw(self.tilt_mode).ok_or(ApiErrorCode::InvalidTiltMode)?;

        // Only the two lift times. `tiltTimeMs` may be zero: a shade with no
        // tilt has no tilt travel to time, which is what every tilt-less shade
        // in a real table looks like.
        if self.up_time_ms == 0 || self.down_time_ms == 0 {
            return Err(ApiErrorCode::TravelTimeZero);
        }

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
