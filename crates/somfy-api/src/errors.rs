//! The typed rejection the REST surface returns instead of an English sentence.
//!
//! ## Why a code and not a message
//!
//! The UI ships English and French, and its completeness rule is that an
//! untranslated key fails the build (`ui/src/i18n/fr.ts` is a total
//! `Record<MessageKey, string>`). A firmware that answered `400` with
//! `"name is too long"` would put a permanently-English string on a French
//! screen, and no amount of care in the UI could fix it — the device has no
//! French and no business having any.
//!
//! So the wire carries a discriminant and the UI owns the wording. That also
//! buys a drift gate for free: `ui/src/api/errors.ts` maps
//! [`ApiErrorCode`] to a message key through a **total** `Record`, so a variant
//! added here and regenerated fails `tsc` until the UI translates it.
//!
//! ## Why the payload is only the code
//!
//! A parallel `message` field would have to be built somewhere, and the only
//! place able to build it is the firmware — which is the thing we just
//! established cannot. `{"code":"nameTooLong"}` is also perfectly legible to a
//! developer holding `curl`, which is the one job a free-text message would
//! have done better.

use serde::{Deserialize, Serialize};

/// Why the device refused a request.
///
/// Every variant is something a user can act on, which is the admission test: a
/// code the UI can only render as "something went wrong" belongs in a log, not
/// on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub enum ApiErrorCode {
    /// A shade with no name. The domain permits it — a migrated backup may
    /// carry one and refusing it there would lose the shade — but a *newly
    /// created* shade with no name is a row the owner cannot identify in a
    /// list, so this boundary is deliberately stricter than the domain.
    NameEmpty,
    /// Over [`crate::NAME_MAX_BYTES`]. Counted in **bytes, not characters**:
    /// the field is a `heapless::String<32>` and an accented French name costs
    /// two bytes per accent, so a 32-character name can still be too long.
    NameTooLong,
    /// A `kind` byte [`somfy_domain::ShadeKind::from_raw`] does not model.
    ///
    /// Import policy defaults an unknown kind to Roller and warns; **create
    /// must not**, because there is nothing to salvage — nobody has lost a
    /// shade yet, and silently handing back a roller to someone who asked for a
    /// garage door is a worse answer than "no".
    InvalidKind,
    /// A `tiltMode` byte [`somfy_domain::TiltMode::from_raw`] does not model.
    InvalidTiltMode,
    /// An opening or closing time of zero. It is the divisor the position
    /// estimate is computed from, so zero is not a slow shade — it is an
    /// estimator with no scale.
    TravelTimeZero,
    /// The address the device allocated is one
    /// [`somfy_domain::ShadeConfig::new`] refuses (the `0` / `0xFF_FFFF`
    /// sentinels). Not reachable through
    /// [`somfy_domain::RemoteIdentity::address_for`], and carried anyway
    /// because an allocator that has gone wrong should say so rather than be
    /// unrepresentable.
    InvalidAddress,
    /// [`somfy_domain::MAX_SHADES`] shades already exist. Distinct from every
    /// other code here because the fix is to remove a shade, not to correct a
    /// field.
    RegistryFull,
    /// No shade has that id.
    NotFound,
    /// Pairing was asked for on a shade whose address is
    /// [`crate::AddressOrigin::Imported`].
    ///
    /// Refused rather than accepted-and-ignored. The `Prog` frame would go out
    /// perfectly well and teach the motor an address it already obeys, so the
    /// user would stand at a shade watching for a jog that means nothing —
    /// and the two-controllers-one-identity clash the address carries would
    /// survive the whole procedure.
    AddressNotAllocated,
}

/// The body of a non-2xx REST response. The HTTP status says how the client
/// should treat it; [`ApiErrorCode`] says what to tell the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct ApiErrorDto {
    pub code: ApiErrorCode,
}

impl From<ApiErrorCode> for ApiErrorDto {
    fn from(code: ApiErrorCode) -> ApiErrorDto {
        ApiErrorDto { code }
    }
}
