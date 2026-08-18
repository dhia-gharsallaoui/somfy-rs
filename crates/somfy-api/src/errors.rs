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
    /// A start lag or a dead band past what the model can express, or one that
    /// does not leave any travel behind it.
    ///
    /// Both bands and the lag are intervals *inside* their direction's travel
    /// time, so a shade whose 30 s Up is 30 s of slat separation has no phase in
    /// which the curtain rises. The estimator answers that by reporting every
    /// move as instantly arrived, which is a shade that claims to be wherever it
    /// was last sent — so it is refused at the boundary instead.
    InvalidDeadBand,
    /// A vent was asked for on a shade whose slat-separation band has never
    /// been measured.
    ///
    /// The vent position **is** that band — it is not derived from a position
    /// estimate, which is the whole reason the command is trustworthy — so with
    /// nothing measured there is nothing to aim at. A vent that ran anyway would
    /// close the shade, send an Up and stop it in the same instant, which looks
    /// to the user like the button does nothing.
    ///
    /// 409 rather than 400 for the same reason as
    /// [`AddressNotAllocated`](ApiErrorCode::AddressNotAllocated): the request
    /// is well-formed, and what makes it inapplicable is the state of the shade.
    VentBandNotMeasured,
    /// A calibration was marked or finished when none was running.
    NotCalibrating,
    /// A calibration run produced numbers the device will not store — a
    /// traverse of zero or past three minutes, or marks that leave no travel
    /// between them. Nothing is stored and the shade is left as it was.
    CalibrationImplausible,

    // -----------------------------------------------------------------------
    // Settings
    //
    // These are the *rules*; which value broke one is [`ApiErrorDto::field`].
    // Splitting the two apart is what keeps this list from growing to one
    // variant per (field × rule) pair — eight settings fields against a dozen
    // rules is ninety-six codes and ninety-six translations, most of which
    // would read identically in both languages.
    //
    // It also gives the settings screen something a flat code cannot: the field
    // to point at. `docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md` R3
    // asks that an invalid value be refused "with the field named", and a form
    // that highlights the offending input is that requirement done properly
    // rather than restated in a sentence.
    // -----------------------------------------------------------------------
    /// A required settings value was empty.
    ValueEmpty,
    /// A settings value is longer than the storage reserved for it. Counted in
    /// **bytes**, like [`ApiErrorCode::NameTooLong`] and for the same reason.
    ValueTooLong,
    /// A settings value is shorter than its minimum. Only the Wi-Fi passphrase
    /// has one — WPA2 requires eight characters — and an empty passphrase is
    /// not short, it is an open network.
    ValueTooShort,
    /// A settings value contains a NUL byte. Refused rather than truncated:
    /// MQTT forbids U+0000 in a string, and a broker is entitled to close the
    /// connection over it, which reads from the device as bad credentials.
    ValueInteriorNul,
    /// The broker address is not four dot-separated decimal octets. Distinct
    /// from [`ApiErrorCode::BrokerAddressUnroutable`], which is about a
    /// well-formed address that no connection could reach.
    BrokerAddressMalformed,
    /// The broker address is unspecified, loopback, multicast or broadcast —
    /// well-formed and unreachable.
    BrokerAddressUnroutable,
    /// The broker port is zero, which addresses nothing.
    BrokerPortZero,
    /// A broker password was given with no username. MQTT permits it and no
    /// broker this device will meet does anything useful with it.
    PasswordWithoutUsername,
    /// A namespace contains an MQTT wildcard (`#` or `+`). Wildcards belong in
    /// subscriptions, never in a topic something publishes to.
    TopicWildcard,
    /// A namespace starts with `/`, which creates an empty leading segment. The
    /// second of the three failures that made discovery unusable on the C++
    /// build: the payload said `/shades/1` while the publisher wrote
    /// `shades/1`, and every entity was permanently unavailable.
    TopicLeadingSlash,
    /// A namespace ends with `/`, which would produce an empty final segment.
    TopicTrailingSlash,
    /// A namespace contains `//`, an empty interior segment. The third of the
    /// three: `homeassistant//cover/1/config` is ignored outright.
    TopicEmptySegment,
    /// A namespace contains a character no topic segment may carry. The
    /// permitted set is `[a-zA-Z0-9_-]`, plus `/` as a separator.
    TopicIllegalCharacter,
    /// `state_root` and `discovery_prefix` name the same namespace, or one sits
    /// inside the other. The one rejection that belongs to a *pair* of values:
    /// both can be individually valid and still put this device's availability
    /// on Home Assistant's own birth topic, which marks it available while it is
    /// offline.
    NamespacesOverlap,
    /// A secret was to be kept and there is none stored to keep. Reached by
    /// configuring a broker for the first time without supplying a password
    /// while asking for the existing one — there is no existing one.
    ///
    /// It exists because the alternative is to treat "keep what you have" as
    /// "have nothing", which would silently configure an anonymous broker
    /// connection under an operator who thought they had set a password.
    SecretNotSet,
    /// A Wi-Fi trial was confirmed or cancelled and none is running. Usually a
    /// stale browser tab: the trial already ended, one way or the other.
    NoTrialInProgress,
    /// A second Wi-Fi trial was started while one was already in flight.
    ///
    /// Refused rather than queued or replaced: two candidates would mean two
    /// deadlines and a confirmation that could not say which credential it was
    /// confirming — and whoever started the second one has, by definition, not
    /// yet found out whether the first worked.
    TrialInProgress,
    /// A Wi-Fi trial was confirmed while the station has not associated with
    /// the candidate network. Confirming means "I reached the device on the new
    /// network", and this device is not on it, so the claim cannot be true.
    TrialNotAssociated,
    /// The configuration region refused the write. The settings were **not**
    /// stored and the device is running on what it had before.
    ///
    /// The one 5xx here, and for the same reason
    /// [`InvalidAddress`](ApiErrorCode::InvalidAddress) is: the request was
    /// fine, the device could not carry it out, and there is nothing the caller
    /// could send instead.
    SettingsUnwritable,
}

/// Which configured value a settings rejection is about.
///
/// Carried beside [`ApiErrorCode`] rather than folded into it — see the block
/// comment on the settings codes above. Absent for every rejection that is not
/// about a value the operator typed.
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
pub enum SettingsFieldDto {
    /// The Wi-Fi network name.
    Ssid,
    /// The Wi-Fi passphrase.
    Psk,
    /// The broker's IPv4 address.
    BrokerAddress,
    /// The broker's TCP port.
    BrokerPort,
    /// The broker username, empty for an anonymous connection.
    BrokerUsername,
    /// The broker password.
    BrokerPassword,
    /// Where Home Assistant looks for discovery configs.
    DiscoveryPrefix,
    /// Where this device's own state and command topics live.
    StateRoot,
}

impl ApiErrorCode {
    /// The HTTP status this rejection is reported with.
    ///
    /// # Why the mapping lives here rather than in the router
    ///
    /// It is a property of the *rejection*, not of the code that happens to be
    /// answering: "this name is too long" is a malformed field wherever it is
    /// said, and "the registry is full" is a conflict with collection state
    /// wherever it is said. Beside the variant it describes, an exhaustive
    /// match makes a code added below unable to reach a router until somebody
    /// has decided what it means over HTTP — the compiler asks.
    /// `ui/mock/plugin.ts` holds the same gate on the mock side, as a total
    /// `Record<ApiErrorCode, number>`, and the two must agree because the same
    /// client code runs against both.
    ///
    /// # The two choices worth defending
    ///
    /// - **[`RegistryFull`](ApiErrorCode::RegistryFull) is 409, not 507.** The
    ///   device is not out of storage in any sense the client can wait out; it
    ///   is at its shade limit, and the fix is to remove a shade. 409 says "the
    ///   state of this collection conflicts with what you asked", which is
    ///   exactly the situation.
    /// - **[`AddressNotAllocated`](ApiErrorCode::AddressNotAllocated) is 409,
    ///   not 400.** The request is perfectly well-formed. What makes it
    ///   inapplicable is a property of the shade — its address belongs to
    ///   another controller — so it is a conflict with resource state rather
    ///   than a malformed body, and a UI that highlighted a form field over it
    ///   would be pointing at nothing.
    ///
    /// [`InvalidAddress`](ApiErrorCode::InvalidAddress) is the one 5xx, and
    /// deliberately: the client does not choose the address, this device does,
    /// so an address the domain refuses is this device's fault and there is
    /// nothing the caller could have sent instead.
    pub const fn http_status(self) -> u16 {
        match self {
            ApiErrorCode::NameEmpty
            | ApiErrorCode::NameTooLong
            | ApiErrorCode::InvalidKind
            | ApiErrorCode::InvalidTiltMode
            | ApiErrorCode::TravelTimeZero
            | ApiErrorCode::InvalidDeadBand
            | ApiErrorCode::CalibrationImplausible
            | ApiErrorCode::ValueEmpty
            | ApiErrorCode::ValueTooLong
            | ApiErrorCode::ValueTooShort
            | ApiErrorCode::ValueInteriorNul
            | ApiErrorCode::BrokerAddressMalformed
            | ApiErrorCode::BrokerAddressUnroutable
            | ApiErrorCode::BrokerPortZero
            | ApiErrorCode::PasswordWithoutUsername
            | ApiErrorCode::TopicWildcard
            | ApiErrorCode::TopicLeadingSlash
            | ApiErrorCode::TopicTrailingSlash
            | ApiErrorCode::TopicEmptySegment
            | ApiErrorCode::TopicIllegalCharacter
            | ApiErrorCode::NamespacesOverlap
            | ApiErrorCode::SecretNotSet => 400,
            ApiErrorCode::NotFound => 404,
            ApiErrorCode::RegistryFull
            | ApiErrorCode::AddressNotAllocated
            | ApiErrorCode::VentBandNotMeasured
            | ApiErrorCode::NotCalibrating
            // Both are conflicts with the state of a trial rather than
            // malformed requests: the body was fine and there was nothing else
            // the caller could have sent, because what is wrong is that no
            // trial is running or that this one has not associated yet.
            | ApiErrorCode::NoTrialInProgress
            | ApiErrorCode::TrialInProgress
            | ApiErrorCode::TrialNotAssociated => 409,
            ApiErrorCode::InvalidAddress | ApiErrorCode::SettingsUnwritable => 500,
        }
    }
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
    /// Which settings value the rejection is about, when it is about one.
    ///
    /// **Omitted from the wire entirely when absent**, so every response this
    /// crate produced before settings existed is byte-identical to what it
    /// produces now — `{"code":"nameTooLong"}`, not
    /// `{"code":"nameTooLong","field":null}`. The `skip_serializing_if` is what
    /// buys that; without it the `Refusal` writer's measured content length
    /// would move for every existing endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub field: Option<SettingsFieldDto>,
}

impl ApiErrorDto {
    /// A rejection that names the value it is about.
    pub const fn field(code: ApiErrorCode, field: SettingsFieldDto) -> ApiErrorDto {
        ApiErrorDto {
            code,
            field: Some(field),
        }
    }
}

impl From<ApiErrorCode> for ApiErrorDto {
    fn from(code: ApiErrorCode) -> ApiErrorDto {
        ApiErrorDto { code, field: None }
    }
}
