//! Reading and changing what the device is provisioned with.
//!
//! # Secrets go in and never come out
//!
//! This is the module that turns an open API from an actuation risk into a
//! credential-disclosure one, and §7.3 of the design spec names that as the
//! moment to weigh it. Authentication is deferred by the owner, so the shape of
//! these types is the whole of what stands between an unauthenticated `GET` and
//! the Wi-Fi passphrase.
//!
//! The rule is therefore structural rather than careful: **no outbound type
//! here has a field a secret could be written into.** [`WifiSettingsDto`]
//! carries `psk_set: bool` and [`MqttSettingsDto`] carries
//! `password_set: bool`; neither has a `psk` or a `password`, so the response
//! writer has nowhere to put one and a future edit that wanted to would have to
//! add the field and answer for it. Everything a settings screen needs to draw
//! itself is here — *whether* a passphrase is set is what decides between "•••"
//! and "not set" — and what it does not need is the passphrase.
//!
//! This is not a claim about the secret at rest. Flash is unencrypted and
//! `somfy_config`'s own docs say so plainly. It is a claim about one specific
//! path: the LAN API cannot be asked to read one out.
//!
//! # A secret that is not sent is not a secret that is cleared
//!
//! Write-only fields create a problem the read side does not have. An operator
//! changing the broker's *port* must not have to retype its password, so an
//! absent password has to mean "leave it alone" — and then there is no way left
//! to say "there should not be one", which is a configuration an operator can
//! mean and a device can hold.
//!
//! [`SecretDto`] makes all three sayable, and says them out loud:
//! `{"secret":"keep"}`, `{"secret":"set","value":"…"}`, `{"secret":"clear"}`.
//! An absent field is not one of them — it is a malformed body — so "I forgot
//! to send it" cannot be silently read as either of the two things it might
//! have meant. That is the same posture as the rest of this crate: refuse,
//! never guess.
//!
//! # Where the rules live
//!
//! Nowhere here. Every value goes back through
//! [`somfy_config::WifiCredentials::new`] and
//! [`somfy_config::MqttSettings::new`], which are the same constructors the
//! flash record decodes through — so a value this API accepts is a value the
//! region can hold, and a value it refuses is one no path could have stored.
//! What this module adds is the translation from those typed errors into an
//! [`ApiErrorDto`] that names the offending field, which is
//! `docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md` R3 on the wire.

use core::net::Ipv4Addr;
use core::str::FromStr;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::Deserialize as DeriveDeserialize;
use serde::Serialize;
use somfy_config::{
    CredentialError, Field, MqttField, MqttSettings, MqttSettingsError, TrialPhase,
    WifiCredentials, WifiTrial, MAX_BROKER_PASSWORD_LEN, MAX_BROKER_USERNAME_LEN, MAX_PSK_LEN,
    MAX_SSID_LEN, MAX_TOPIC_ROOT_LEN,
};

use crate::errors::{ApiErrorCode, ApiErrorDto, SettingsFieldDto};

/// Longest dotted-quad IPv4 address: `255.255.255.255`.
pub const MAX_ADDRESS_LEN: usize = 15;

/// Longest JSON a [`SettingsDto`] serialises to, in bytes.
///
/// **Measured, not counted.** `tests/settings.rs` builds the widest legal value
/// — every free-text string full of control characters, which escape to
/// `\u00XX` six bytes at a time — and asserts this bound from both sides: never
/// under it, and never more than 128 bytes over it. That is the discipline
/// [`crate::SHADE_JSON_MAX_BYTES`] is held to, and for the same reason: that
/// figure was hand-counted once at 512 and was wrong by 160 bytes, which would
/// have made one shade answer with malformed JSON forever.
///
/// It is wider than a shade's because three of its strings can hold arbitrary
/// text. An SSID is whatever an access point broadcasts and a broker username
/// is whatever the broker was configured with, so neither can be restricted to
/// a character class the way the two MQTT namespaces are.
///
/// **Every byte of it is spent four times.** The firmware serialises this into
/// one fixed buffer held across the write, inside each of the web server's
/// connection task futures, which are statically allocated out of the DRAM the
/// Wi-Fi driver's heap is carved from. The measurement is **859**; this is that
/// rounded up to the next 128, which is the granularity the test's own ceiling
/// check uses. An ordinary document is 204.
pub const SETTINGS_JSON_MAX_BYTES: usize = 896;

/// Longest secret this API will carry inbound.
///
/// The Wi-Fi passphrase's limit, which is the larger of the two — the broker
/// password's is [`somfy_config::MAX_BROKER_PASSWORD_LEN`], the same figure
/// today. One buffer for both, because [`SecretDto`] is one type and a
/// per-field capacity would make it two.
pub const MAX_SECRET_LEN: usize = if MAX_PSK_LEN > MAX_BROKER_PASSWORD_LEN {
    MAX_PSK_LEN
} else {
    MAX_BROKER_PASSWORD_LEN
};

/// Inbound text is given twice the room it may keep.
///
/// The same trick [`crate::CreateShadeDto`] uses on a shade's name, and for the
/// same reason: a value one byte over its limit must come back as a typed
/// rejection naming the field, not as a serde parse failure that says only
/// "malformed body". Doubling is enough to make the ordinary mistake — a
/// passphrase pasted with something extra on the end — land in the branch that
/// can explain itself.
const INBOX: usize = 2;

// ---------------------------------------------------------------------------
// Outbound: what the device is configured with
// ---------------------------------------------------------------------------

/// Everything the settings screen reads, in one response.
///
/// One document rather than three endpoints because the three are read together
/// on every visit and polled together while a trial runs, and because `None` in
/// either half is a *value* — a device with no broker still receives, decodes
/// and tracks — which a 404 from a separate endpoint would have muddled with
/// "that address does not exist".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, DeriveDeserialize)]
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
pub struct SettingsDto {
    /// `null` when no credential is stored — a freshly flashed board, which is
    /// an ordinary state and not an error.
    pub wifi: Option<WifiSettingsDto>,
    /// `null` when no broker is provisioned. Also an ordinary state: the
    /// controller runs without one and publishes nothing.
    pub mqtt: Option<MqttSettingsDto>,
    /// The live credential trial, if one is running. `null` the rest of the
    /// time, which is almost always.
    pub wifi_trial: Option<WifiTrialDto>,
}

/// The stored Wi-Fi credential, minus the passphrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, DeriveDeserialize)]
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
pub struct WifiSettingsDto {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub ssid: heapless::String<MAX_SSID_LEN>,
    /// Whether a passphrase is stored — **not** what it is. `false` means an
    /// open network, which is a configuration and not an omission.
    pub psk_set: bool,
}

impl WifiSettingsDto {
    /// Snapshot a stored credential for the wire.
    pub fn of(credentials: &WifiCredentials) -> WifiSettingsDto {
        let mut ssid = heapless::String::new();
        // Infallible: the source is a `String<MAX_SSID_LEN>` and so is this.
        let _ = ssid.push_str(credentials.ssid());
        WifiSettingsDto {
            ssid,
            psk_set: !credentials.is_open(),
        }
    }
}

/// The stored broker settings, minus the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, DeriveDeserialize)]
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
pub struct MqttSettingsDto {
    /// Dotted quad. A string rather than four numbers because it is a string in
    /// the form the operator types it into, and rendering it is then the one
    /// thing the screen does not have to get right.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub address: heapless::String<MAX_ADDRESS_LEN>,
    pub port: u16,
    /// Empty for an anonymous connection. Not a secret — it is half of the
    /// pair, and knowing the username buys nothing without the password.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub username: heapless::String<MAX_BROKER_USERNAME_LEN>,
    /// Whether a password is stored, **not** what it is.
    pub password_set: bool,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub discovery_prefix: heapless::String<MAX_TOPIC_ROOT_LEN>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub state_root: heapless::String<MAX_TOPIC_ROOT_LEN>,
}

impl MqttSettingsDto {
    /// Snapshot stored broker settings for the wire.
    pub fn of(settings: &MqttSettings) -> MqttSettingsDto {
        let mut address: heapless::String<MAX_ADDRESS_LEN> = heapless::String::new();
        // `write!` into a `heapless::String` through `core::fmt::Write`. The
        // buffer is the longest dotted quad there is, so this cannot truncate.
        let _ = core::fmt::Write::write_fmt(&mut address, format_args!("{}", settings.address()));

        let mut username = heapless::String::new();
        let _ = username.push_str(settings.username());
        let mut discovery_prefix = heapless::String::new();
        let _ = discovery_prefix.push_str(settings.discovery_prefix());
        let mut state_root = heapless::String::new();
        let _ = state_root.push_str(settings.state_root());

        MqttSettingsDto {
            address,
            port: settings.port(),
            username,
            password_set: !settings.password().is_empty(),
            discovery_prefix,
            state_root,
        }
    }
}

/// What a live Wi-Fi trial is waiting for.
///
/// A separate type from [`somfy_config::TrialPhase`] for the reason every DTO
/// here is separate: `somfy-config` depends on neither `serde` nor `ts-rs`, and
/// the mapping is exhaustive, so a third phase added there stops this
/// compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, DeriveDeserialize)]
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
pub enum TrialPhaseDto {
    /// The candidate is on the radio and the station has not joined yet.
    Associating,
    /// Joined. Waiting for the operator to reach the device and confirm.
    AwaitingConfirmation,
}

impl TrialPhaseDto {
    /// Carry the phase onto the wire.
    pub fn of(phase: TrialPhase) -> TrialPhaseDto {
        match phase {
            TrialPhase::Associating => TrialPhaseDto::Associating,
            TrialPhase::AwaitingConfirmation => TrialPhaseDto::AwaitingConfirmation,
        }
    }
}

/// A credential that is on the radio and not in flash.
///
/// Carries the candidate's SSID, because the screen has to be able to say
/// *which* network to join, and an SSID is broadcast in the clear by the access
/// point several times a second. It does not carry the candidate passphrase,
/// for the same reason nothing else here does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, DeriveDeserialize)]
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
pub struct WifiTrialDto {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub ssid: heapless::String<MAX_SSID_LEN>,
    pub phase: TrialPhaseDto,
    /// Milliseconds left in the current phase before the previous credential is
    /// put back. Zero means the deadline has passed and the revert has not been
    /// polled yet, which is a moment rather than a state.
    pub remaining_ms: u32,
}

impl WifiTrialDto {
    /// Snapshot a live trial for the wire.
    pub fn of(trial: &WifiTrial, now_ms: u64) -> WifiTrialDto {
        let mut ssid = heapless::String::new();
        let _ = ssid.push_str(trial.candidate().ssid());
        WifiTrialDto {
            ssid,
            phase: TrialPhaseDto::of(trial.phase()),
            // `try_from`, not `as`: the longest deadline is three minutes so
            // this cannot truncate, and a cast that silently could is not worth
            // the characters it saves.
            remaining_ms: u32::try_from(trial.remaining_ms(now_ms)).unwrap_or(u32::MAX),
        }
    }
}

// ---------------------------------------------------------------------------
// Inbound: changing what the device is configured with
// ---------------------------------------------------------------------------

/// What to do with a write-only field.
///
/// Hand-tagged rather than derived, like [`crate::CommandDto`] and for the same
/// reason: `serde`'s internally-tagged enums buffer through `Content`, which
/// needs `alloc`, and this crate has none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        tag = "secret",
        rename_all = "camelCase"
    )
)]
pub enum SecretDto {
    /// Leave whatever is stored. Refused with
    /// [`ApiErrorCode::SecretNotSet`] when nothing is.
    Keep,
    /// Replace it with this.
    Set {
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        value: heapless::String<{ MAX_SECRET_LEN * INBOX }>,
    },
    /// There should not be one. An open Wi-Fi network, or an anonymous broker.
    Clear,
}

/// The tag half of [`SecretDto`]'s wire form.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
enum SecretTag {
    Keep,
    Set,
    Clear,
}

/// The flat wire form [`SecretDto`] is read out of.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
struct SecretWire {
    secret: SecretTag,
    value: Option<heapless::String<{ MAX_SECRET_LEN * INBOX }>>,
}

impl<'de> Deserialize<'de> for SecretDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SecretWire::deserialize(deserializer)?;
        Ok(match wire.secret {
            SecretTag::Keep => SecretDto::Keep,
            SecretTag::Set => SecretDto::Set {
                value: wire.value.ok_or_else(|| D::Error::missing_field("value"))?,
            },
            SecretTag::Clear => SecretDto::Clear,
        })
    }
}

impl SecretDto {
    /// Resolve against what is stored.
    ///
    /// `stored` is `None` when the containing settings do not exist yet, which
    /// is what makes [`SecretDto::Keep`] refusable rather than merely empty.
    fn resolve<'a>(
        &'a self,
        stored: Option<&'a str>,
        field: SettingsFieldDto,
    ) -> Result<&'a str, ApiErrorDto> {
        match self {
            SecretDto::Keep => stored.ok_or(ApiErrorDto::field(ApiErrorCode::SecretNotSet, field)),
            SecretDto::Set { value } => Ok(value.as_str()),
            SecretDto::Clear => Ok(""),
        }
    }
}

/// What to do about a live credential trial.
///
/// One request with the decision in the body rather than two paths, and it is
/// the same choice [`crate::CalibrationStepDto`] made for the same reason: on
/// this device a route is not free. `picoserve`'s router is a type per route
/// wrapping the previous one, so every path is a variant in the connection
/// task's monomorphised future — and there are `api::HTTP_TASKS` of those
/// futures, statically allocated, paid for in Wi-Fi heap on every boot whether
/// or not anybody opens the UI. Folding two endpoints into one is measured in
/// kilobytes here, not in tidiness.
///
/// The two are also genuinely one conversation: both end the same trial, and
/// exactly one of them can be right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        tag = "decision",
        rename_all = "camelCase"
    )
)]
pub enum TrialDecisionDto {
    /// The operator reached the device on the candidate network. Store it.
    Confirm,
    /// Put the stored credential back now, rather than waiting out the
    /// deadline.
    Cancel,
}

/// The tag half of [`TrialDecisionDto`]'s wire form.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
enum DecisionTag {
    Confirm,
    Cancel,
}

/// The flat wire form [`TrialDecisionDto`] is read out of.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
struct DecisionWire {
    decision: DecisionTag,
}

impl<'de> Deserialize<'de> for TrialDecisionDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match DecisionWire::deserialize(deserializer)?.decision {
            DecisionTag::Confirm => TrialDecisionDto::Confirm,
            DecisionTag::Cancel => TrialDecisionDto::Cancel,
        })
    }
}

/// A candidate Wi-Fi credential, as the settings screen sends it.
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
pub struct WifiUpdateDto {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub ssid: heapless::String<{ MAX_SSID_LEN * INBOX }>,
    pub psk: SecretDto,
}

impl WifiUpdateDto {
    /// Validate the candidate, resolving `psk` against `stored`.
    ///
    /// The result is what a trial would be started with; nothing here writes
    /// anything, and this is deliberately callable — and called — before the
    /// radio is touched, so an invalid credential costs no connection at all.
    pub fn to_credentials(
        &self,
        stored: Option<&WifiCredentials>,
    ) -> Result<WifiCredentials, ApiErrorDto> {
        let psk = self
            .psk
            .resolve(stored.map(WifiCredentials::psk), SettingsFieldDto::Psk)?;
        WifiCredentials::new(self.ssid.as_str(), psk).map_err(credential_rejection)
    }
}

/// Broker settings, as the settings screen sends them.
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
pub struct MqttUpdateDto {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub address: heapless::String<{ MAX_ADDRESS_LEN * INBOX }>,
    pub port: u16,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub username: heapless::String<{ MAX_BROKER_USERNAME_LEN * INBOX }>,
    pub password: SecretDto,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub discovery_prefix: heapless::String<{ MAX_TOPIC_ROOT_LEN * INBOX }>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub state_root: heapless::String<{ MAX_TOPIC_ROOT_LEN * INBOX }>,
}

impl MqttUpdateDto {
    /// Validate the settings, resolving `password` against `stored`.
    pub fn to_settings(&self, stored: Option<&MqttSettings>) -> Result<MqttSettings, ApiErrorDto> {
        let address = Ipv4Addr::from_str(self.address.as_str()).map_err(|_| {
            ApiErrorDto::field(
                ApiErrorCode::BrokerAddressMalformed,
                SettingsFieldDto::BrokerAddress,
            )
        })?;
        let password = self.password.resolve(
            stored.map(MqttSettings::password),
            SettingsFieldDto::BrokerPassword,
        )?;
        MqttSettings::new(
            address,
            self.port,
            self.username.as_str(),
            password,
            self.discovery_prefix.as_str(),
            self.state_root.as_str(),
        )
        .map_err(mqtt_rejection)
    }
}

// ---------------------------------------------------------------------------
// Typed errors onto the wire
//
// Three exhaustive matches, so a rule added to `somfy-config` or `somfy-mqtt`
// stops this compiling rather than reaching a screen as "something went wrong".
// ---------------------------------------------------------------------------

/// Carry a refused Wi-Fi credential onto the wire.
fn credential_rejection(error: CredentialError) -> ApiErrorDto {
    let field = match error.field() {
        Field::Ssid => SettingsFieldDto::Ssid,
        Field::Psk => SettingsFieldDto::Psk,
    };
    let code = match error {
        CredentialError::Empty(_) => ApiErrorCode::ValueEmpty,
        CredentialError::TooLong { .. } => ApiErrorCode::ValueTooLong,
        CredentialError::TooShort { .. } => ApiErrorCode::ValueTooShort,
        CredentialError::InteriorNul(_) => ApiErrorCode::ValueInteriorNul,
    };
    ApiErrorDto::field(code, field)
}

/// Carry refused broker settings onto the wire.
fn mqtt_rejection(error: MqttSettingsError) -> ApiErrorDto {
    match error {
        MqttSettingsError::Unroutable(_) => ApiErrorDto::field(
            ApiErrorCode::BrokerAddressUnroutable,
            SettingsFieldDto::BrokerAddress,
        ),
        MqttSettingsError::PortZero => {
            ApiErrorDto::field(ApiErrorCode::BrokerPortZero, SettingsFieldDto::BrokerPort)
        }
        MqttSettingsError::TooLong { field, .. } => {
            ApiErrorDto::field(ApiErrorCode::ValueTooLong, mqtt_field(field))
        }
        MqttSettingsError::InteriorNul(field) => {
            ApiErrorDto::field(ApiErrorCode::ValueInteriorNul, mqtt_field(field))
        }
        // Named against the username rather than the password, because that is
        // the field to fill in: the operator typed a password and meant to type
        // both.
        MqttSettingsError::PasswordWithoutUsername => ApiErrorDto::field(
            ApiErrorCode::PasswordWithoutUsername,
            SettingsFieldDto::BrokerUsername,
        ),
        MqttSettingsError::Topic(topic) => topic_rejection(topic),
    }
}

/// Which settings field a [`MqttField`] is.
fn mqtt_field(field: MqttField) -> SettingsFieldDto {
    match field {
        MqttField::Address => SettingsFieldDto::BrokerAddress,
        MqttField::Port => SettingsFieldDto::BrokerPort,
        MqttField::Username => SettingsFieldDto::BrokerUsername,
        MqttField::Password => SettingsFieldDto::BrokerPassword,
        MqttField::DiscoveryPrefix => SettingsFieldDto::DiscoveryPrefix,
        MqttField::StateRoot => SettingsFieldDto::StateRoot,
    }
}

/// Carry a refused namespace onto the wire.
fn topic_rejection(error: somfy_mqtt::ConfigError) -> ApiErrorDto {
    let code = match error {
        somfy_mqtt::ConfigError::Empty(_) => ApiErrorCode::ValueEmpty,
        somfy_mqtt::ConfigError::Wildcard(_, _) => ApiErrorCode::TopicWildcard,
        somfy_mqtt::ConfigError::LeadingSlash(_) => ApiErrorCode::TopicLeadingSlash,
        somfy_mqtt::ConfigError::TrailingSlash(_) => ApiErrorCode::TopicTrailingSlash,
        somfy_mqtt::ConfigError::EmptySegment(_) => ApiErrorCode::TopicEmptySegment,
        somfy_mqtt::ConfigError::IllegalCharacter(_, _) => ApiErrorCode::TopicIllegalCharacter,
        somfy_mqtt::ConfigError::TooLong(_, _) => ApiErrorCode::ValueTooLong,
        somfy_mqtt::ConfigError::Overlap(_) => ApiErrorCode::NamespacesOverlap,
    };
    // `node_id` and `device_id` are not settings — the firmware derives both
    // from the factory MAC and neither is an input to `MqttSettings::new`, so
    // no rejection reaching this function can name one. Answering `None` rather
    // than inventing a field is the honest form of that: the screen has nothing
    // to highlight, and it is told so.
    match error.field() {
        somfy_mqtt::Field::DiscoveryPrefix => {
            ApiErrorDto::field(code, SettingsFieldDto::DiscoveryPrefix)
        }
        somfy_mqtt::Field::StateRoot => ApiErrorDto::field(code, SettingsFieldDto::StateRoot),
        somfy_mqtt::Field::NodeId | somfy_mqtt::Field::DeviceId => {
            ApiErrorDto { code, field: None }
        }
    }
}
