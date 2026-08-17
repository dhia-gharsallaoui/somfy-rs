//! The broker to talk to and the two namespaces to talk in, validated where
//! they are entered.
//!
//! Same posture as [`WifiCredentials`](crate::WifiCredentials) next door, and
//! for the same reason: every rule here describes a value that would be
//! *accepted* somewhere downstream and then silently fail — a broker that never
//! connects, or a discovery config published where nothing reads it. Those look
//! identical to a device that is simply switched off.
//!
//! ## Why the address is an [`Ipv4Addr`] and not a host name
//!
//! Because the firmware has no resolver. `embassy-net` is built here without
//! its `dns` feature, so a host name would be a value this crate accepts, the
//! flash stores, and the network layer can do nothing with — the exact shape of
//! failure the requirements spec was written from. Storing an address instead
//! makes an unresolvable broker unrepresentable rather than merely unlikely.
//!
//! ## Why the two namespaces are validated by `somfy-mqtt`
//!
//! Because a record must not be able to deliver a value the topic builder would
//! refuse. `somfy-mqtt` owns the rules R2 and R3 state, so this crate calls
//! them rather than restating them — including the cross-field one, where each
//! root is individually valid and the *pair* is not. Catching that here puts
//! the rejection in front of the person typing it; catching it at boot puts it
//! three flashes away.
//!
//! ## What is stored here is not a secret
//!
//! The broker password is written to flash in the clear, exactly as the Wi-Fi
//! passphrase is. See this crate's module docs for why that is stated rather
//! than mitigated. [`fmt::Debug`] redacts it so it does not reach a serial
//! console through the ordinary `{:?}` error path, which is a much smaller and
//! different claim.

use core::fmt;
use core::net::Ipv4Addr;

use heapless::String;
use somfy_mqtt::{namespaces_overlap, ConfigError, DiscoveryPrefix, StateRoot};

/// Bytes a broker username may occupy.
///
/// MQTT does not bound it; brokers do, and Mosquitto's own limit is far above
/// anything a person types. The limit exists so the record layout is fixed.
pub const MAX_BROKER_USERNAME_LEN: usize = 32;

/// Bytes a broker password may occupy. See [`MAX_BROKER_USERNAME_LEN`].
pub const MAX_BROKER_PASSWORD_LEN: usize = 64;

/// Bytes either of the two topic namespaces may occupy.
///
/// Below `somfy-mqtt`'s own 64-byte ceiling, and deliberately: this is a
/// storage bound, and `somfy-mqtt`'s is the bound its capacity proofs are built
/// on. A value between the two is refused here, by a message naming the field,
/// rather than truncated into a different namespace.
pub const MAX_TOPIC_ROOT_LEN: usize = 32;

/// Where Home Assistant looks for discovery configs, unless an estate has
/// already moved it for some other reason.
///
/// Home Assistant supports exactly one, and it is global to the installation.
/// A device that ships with anything else taxes every other MQTT device on that
/// network for as long as it is installed.
pub const DEFAULT_DISCOVERY_PREFIX: &str = "homeassistant";

/// Where this device's own state, command and availability topics live.
///
/// Anything but the discovery prefix. The two are independent namespaces and
/// the payload's `~` is what links them; conflating them is the single fault
/// that made discovery unusable on the deployed firmware.
pub const DEFAULT_STATE_ROOT: &str = "somfyrs";

/// Which value an [`MqttSettingsError`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttField {
    /// The broker's IPv4 address.
    Address,
    /// The broker's TCP port.
    Port,
    /// The MQTT username, empty for an anonymous connection.
    Username,
    /// The MQTT password.
    Password,
    /// Where discovery configs are published.
    DiscoveryPrefix,
    /// Where this device's own topics live.
    StateRoot,
}

impl MqttField {
    /// The field's name, for a message a person reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            MqttField::Address => "broker address",
            MqttField::Port => "broker port",
            MqttField::Username => "broker username",
            MqttField::Password => "broker password",
            MqttField::DiscoveryPrefix => "discovery_prefix",
            MqttField::StateRoot => "state_root",
        }
    }
}

impl fmt::Display for MqttField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a set of MQTT settings was refused.
///
/// As with the Wi-Fi credentials, there is no variant meaning "accepted with
/// adjustments". A truncated state root is a different namespace, and a
/// repaired address is a different broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttSettingsError {
    /// No TCP connection could reach this address: it is unspecified,
    /// loopback, multicast or broadcast.
    Unroutable(Ipv4Addr),
    /// The port is zero, which addresses nothing.
    PortZero,
    /// A field is longer than the storage reserved for it.
    TooLong {
        /// The field that was too long.
        field: MqttField,
        /// Its length in bytes.
        len: usize,
        /// The largest length that would have been accepted.
        limit: usize,
    },
    /// A field contains a NUL byte. MQTT's own rule: a UTF-8 encoded string in
    /// a control packet must not carry U+0000, and a broker is entitled to
    /// close the connection over it — which reads from this end as rejected
    /// credentials.
    InteriorNul(MqttField),
    /// A password with no username. MQTT permits it and no broker this device
    /// will meet does anything useful with it; the operator meant to type a
    /// username.
    PasswordWithoutUsername,
    /// One of the two namespaces broke a topic rule, or the pair of them did.
    /// The inner error names the field and the rule; see `somfy-mqtt`.
    Topic(ConfigError),
}

impl From<ConfigError> for MqttSettingsError {
    fn from(error: ConfigError) -> Self {
        MqttSettingsError::Topic(error)
    }
}

impl fmt::Display for MqttSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MqttSettingsError::Unroutable(address) => write!(
                formatter,
                "{} {address} is not an address a TCP connection can reach",
                MqttField::Address,
            ),
            MqttSettingsError::PortZero => {
                write!(formatter, "{} must not be zero", MqttField::Port)
            }
            MqttSettingsError::TooLong { field, len, limit } => {
                write!(
                    formatter,
                    "{field} is {len} bytes; at most {limit} are allowed"
                )
            }
            MqttSettingsError::InteriorNul(field) => write!(
                formatter,
                "{field} contains a NUL byte, which MQTT does not allow in a string",
            ),
            MqttSettingsError::PasswordWithoutUsername => write!(
                formatter,
                "a {} was given with no {}",
                MqttField::Password,
                MqttField::Username,
            ),
            MqttSettingsError::Topic(error) => write!(formatter, "{error}"),
        }
    }
}

/// So the host-side provisioning tool can report one with `?`.
impl core::error::Error for MqttSettingsError {}

/// One broker, and the two namespaces to use on it.
///
/// ## These are not secrets at rest
///
/// A persisted `MqttSettings` is readable by anyone holding the board: flash is
/// not encrypted here. [`fmt::Debug`] redacts the password so it does not reach
/// a serial console by accident, which is a different and much smaller claim.
#[derive(Clone, PartialEq, Eq)]
pub struct MqttSettings {
    address: Ipv4Addr,
    port: u16,
    username: String<MAX_BROKER_USERNAME_LEN>,
    password: String<MAX_BROKER_PASSWORD_LEN>,
    discovery_prefix: String<MAX_TOPIC_ROOT_LEN>,
    state_root: String<MAX_TOPIC_ROOT_LEN>,
}

impl MqttSettings {
    /// Check every field and the one relationship between two of them, or say
    /// which rule was broken.
    ///
    /// ```
    /// use core::net::Ipv4Addr;
    /// use somfy_config::{MqttSettings, DEFAULT_DISCOVERY_PREFIX, DEFAULT_STATE_ROOT};
    ///
    /// let mqtt = MqttSettings::new(
    ///     Ipv4Addr::new(192, 0, 2, 10),
    ///     1883,
    ///     "somfy",
    ///     "PLACEHOLDER_BROKER_PASSWORD",
    ///     DEFAULT_DISCOVERY_PREFIX,
    ///     DEFAULT_STATE_ROOT,
    /// )?;
    /// assert_eq!(mqtt.port(), 1883);
    /// # Ok::<(), somfy_config::MqttSettingsError>(())
    /// ```
    pub fn new(
        address: Ipv4Addr,
        port: u16,
        username: &str,
        password: &str,
        discovery_prefix: &str,
        state_root: &str,
    ) -> Result<MqttSettings, MqttSettingsError> {
        if address.is_unspecified()
            || address.is_loopback()
            || address.is_multicast()
            || address.is_broadcast()
        {
            return Err(MqttSettingsError::Unroutable(address));
        }
        if port == 0 {
            return Err(MqttSettingsError::PortZero);
        }

        check(MqttField::Username, username, MAX_BROKER_USERNAME_LEN)?;
        check(MqttField::Password, password, MAX_BROKER_PASSWORD_LEN)?;
        if username.is_empty() && !password.is_empty() {
            return Err(MqttSettingsError::PasswordWithoutUsername);
        }

        // The length is checked here so the failure names *this* crate's
        // storage bound rather than `somfy-mqtt`'s larger one, which would
        // report a limit the record cannot actually hold.
        check(
            MqttField::DiscoveryPrefix,
            discovery_prefix,
            MAX_TOPIC_ROOT_LEN,
        )?;
        check(MqttField::StateRoot, state_root, MAX_TOPIC_ROOT_LEN)?;

        // Through the real validators, and then through the cross-field check,
        // so a record cannot deliver a pair `MqttConfig::new` would refuse at
        // boot. The two values are dropped immediately: they are constructed
        // for the rules they enforce, not for their contents.
        let prefix = DiscoveryPrefix::new(discovery_prefix)?;
        let root = StateRoot::new(state_root)?;
        if namespaces_overlap(&prefix, &root) {
            return Err(MqttSettingsError::Topic(ConfigError::Overlap(
                somfy_mqtt::Field::StateRoot,
            )));
        }

        Ok(MqttSettings {
            address,
            port,
            // Every `expect` here is unreachable: `check` has just bounded each
            // length by the capacity of the string it is copied into.
            username: String::try_from(username).expect("username length checked above"),
            password: String::try_from(password).expect("password length checked above"),
            discovery_prefix: String::try_from(discovery_prefix)
                .expect("discovery prefix length checked above"),
            state_root: String::try_from(state_root).expect("state root length checked above"),
        })
    }

    /// The broker's address.
    pub fn address(&self) -> Ipv4Addr {
        self.address
    }

    /// The broker's TCP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The MQTT username, empty for an anonymous connection.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The MQTT password.
    ///
    /// Named rather than exposed through `Debug` on purpose: a caller that
    /// wants the secret has to ask for it, so the ordinary debugging route
    /// cannot print it by accident.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Whether this broker is to be connected to without credentials.
    pub fn is_anonymous(&self) -> bool {
        self.username.is_empty()
    }

    /// Where discovery configs go.
    pub fn discovery_prefix(&self) -> &str {
        &self.discovery_prefix
    }

    /// Where this device's own topics go.
    pub fn state_root(&self) -> &str {
        &self.state_root
    }

    /// The two namespaces on their own.
    pub fn namespaces(&self) -> Namespaces {
        Namespaces {
            discovery_prefix: self.discovery_prefix.clone(),
            state_root: self.state_root.clone(),
        }
    }

    /// Whether these settings name the same two namespaces as `other`.
    ///
    /// The question R5 turns on: a configuration whose namespaces have moved
    /// has left retained configs at the old addresses, and they have to be
    /// cleared before the new ones are published. Everything else about a
    /// broker — its address, its credentials — can change without a single
    /// retained topic moving.
    pub fn same_namespaces(&self, other: &MqttSettings) -> bool {
        self.namespaces() == other.namespaces()
    }
}

/// The two topic namespaces a configuration published under, without the broker
/// it published to.
///
/// Kept separately because it is the only part of an *old* configuration that
/// is still needed once that configuration has been replaced: R5 requires the
/// retained configs at the old addresses to be cleared before the new ones are
/// published, and the addresses are all the old settings contribute. The old
/// broker's credentials are not just unnecessary, they are a secret with no
/// reason to be in memory.
///
/// Small on purpose — two bounded strings — because the firmware holds several
/// of these at boot, one per superseded configuration still readable in the
/// flash ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespaces {
    discovery_prefix: String<MAX_TOPIC_ROOT_LEN>,
    state_root: String<MAX_TOPIC_ROOT_LEN>,
}

impl Namespaces {
    /// Where discovery configs went.
    pub fn discovery_prefix(&self) -> &str {
        &self.discovery_prefix
    }

    /// Where the device's own topics went.
    pub fn state_root(&self) -> &str {
        &self.state_root
    }
}

/// Redacts the password. See [`crate::WifiCredentials`]'s `Debug` for the
/// argument; it is the same one, and the same error paths reach it.
impl fmt::Debug for MqttSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MqttSettings")
            .field("address", &self.address)
            .field("port", &self.port)
            .field(
                "username",
                &if self.is_anonymous() {
                    "<anonymous>"
                } else {
                    self.username.as_str()
                },
            )
            .field(
                "password",
                &if self.password.is_empty() {
                    "<anonymous>"
                } else {
                    "<redacted>"
                },
            )
            .field("discovery_prefix", &self.discovery_prefix.as_str())
            .field("state_root", &self.state_root.as_str())
            .finish()
    }
}

/// Bound one field's length and reject an embedded NUL.
fn check(field: MqttField, value: &str, limit: usize) -> Result<(), MqttSettingsError> {
    let len = value.len();
    if len > limit {
        return Err(MqttSettingsError::TooLong { field, len, limit });
    }
    if value.as_bytes().contains(&0) {
        return Err(MqttSettingsError::InteriorNul(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order matters for the same reason it does in the Wi-Fi rules: a
    /// value that is both too long *and* contains a NUL should report the
    /// length, because that is the one an operator can act on without reading
    /// the string byte by byte.
    #[test]
    fn a_length_failure_is_reported_before_a_nul() {
        let over = "u".repeat(MAX_BROKER_USERNAME_LEN) + "\0";
        assert_eq!(
            check(MqttField::Username, &over, MAX_BROKER_USERNAME_LEN),
            Err(MqttSettingsError::TooLong {
                field: MqttField::Username,
                len: MAX_BROKER_USERNAME_LEN + 1,
                limit: MAX_BROKER_USERNAME_LEN,
            })
        );
    }

    #[test]
    fn moving_either_namespace_is_a_namespace_change_and_moving_neither_is_not() {
        let base = MqttSettings::new(
            Ipv4Addr::new(192, 0, 2, 10),
            1883,
            "",
            "",
            DEFAULT_DISCOVERY_PREFIX,
            DEFAULT_STATE_ROOT,
        )
        .expect("valid");

        // A different broker at the same namespaces retains nothing new.
        let moved_broker = MqttSettings::new(
            Ipv4Addr::new(198, 51, 100, 7),
            8883,
            "somfy",
            "PLACEHOLDER_BROKER_PASSWORD",
            DEFAULT_DISCOVERY_PREFIX,
            DEFAULT_STATE_ROOT,
        )
        .expect("valid");
        assert!(base.same_namespaces(&moved_broker));

        for (prefix, root) in [
            ("ha", DEFAULT_STATE_ROOT),
            (DEFAULT_DISCOVERY_PREFIX, "blinds"),
        ] {
            let moved = MqttSettings::new(Ipv4Addr::new(192, 0, 2, 10), 1883, "", "", prefix, root)
                .expect("valid");
            assert!(!base.same_namespaces(&moved), "({prefix}, {root})");
        }
    }
}
