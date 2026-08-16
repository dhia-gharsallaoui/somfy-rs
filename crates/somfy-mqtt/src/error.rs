//! What a rejected configuration says about itself.
//!
//! Every variant carries the [`Field`] it came from. That is not decoration:
//! the failure this crate exists to prevent is a configuration that was
//! *accepted*, published to an address nobody reads, and looked like it had
//! worked. An operator who is told "the state root has a trailing slash" fixes
//! it in seconds; one who is told nothing spends an evening with a broker.

use core::fmt;

/// Which configured value a [`ConfigError`] is about.
///
/// The two roots are separate variants because they are separate values with
/// separate namespaces — see [`crate::DiscoveryPrefix`]. A validator that is
/// right for one and forgotten for the other is the asymmetry that produced the
/// failure behind this crate, so every rejection test runs over all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Where Home Assistant looks for discovery configs. Global to the whole
    /// HA installation.
    DiscoveryPrefix,
    /// Where this device's own state and command topics live.
    StateRoot,
    /// The optional device-identifying segment inside a discovery topic.
    NodeId,
    /// The stable device identifier that `unique_id`s are built from.
    DeviceId,
}

impl Field {
    /// The name an operator would recognise, for a message.
    pub const fn as_str(self) -> &'static str {
        match self {
            Field::DiscoveryPrefix => "discovery_prefix",
            Field::StateRoot => "state_root",
            Field::NodeId => "node_id",
            Field::DeviceId => "device_id",
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a configured value was refused.
///
/// There is no variant for "accepted with adjustments", and that is the point.
/// Truncating an over-long root, or stripping a stray slash, silently changes
/// the address the device publishes to — which is indistinguishable, from the
/// operator's side, from the device being broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// The value is empty. An empty root does not mean "no prefix"; it means
    /// every topic built from it gains an empty leading segment or a leading
    /// slash, and neither addresses what the operator intended.
    Empty(Field),
    /// The value contains an MQTT wildcard. Wildcards belong in
    /// *subscriptions*, never in a topic something publishes to, and a broker
    /// will reject the publish outright.
    Wildcard(Field, char),
    /// The value starts with `/`. MQTT permits it, but it creates an anonymous
    /// empty first segment that almost never matches what the operator meant —
    /// and a payload base with a leading slash disagrees with a publisher
    /// without one, which is a fault with no visible symptom beyond every
    /// entity being unavailable.
    LeadingSlash(Field),
    /// The value ends with `/`, which would produce an empty final segment.
    TrailingSlash(Field),
    /// The value contains `//`, an empty interior segment.
    EmptySegment(Field),
    /// The value contains a character that cannot appear in a topic segment
    /// this crate builds. The permitted set is `[a-zA-Z0-9_-]`, plus `/` as a
    /// separator in the two multi-segment roots.
    IllegalCharacter(Field, char),
    /// The value is longer than the storage reserved for it. The payload is the
    /// limit, so a caller can say what it is.
    TooLong(Field, usize),
    /// `state_root` and `discovery_prefix` name the same namespace, or one sits
    /// inside the other.
    ///
    /// The only error here that is about a *pair* of values rather than one.
    /// Both can be individually valid and still be wrong together: set both to
    /// `homeassistant` and availability lands on `homeassistant/status`, which
    /// is Home Assistant's own birth and will topic, so HA's birth message
    /// marks the device available whether or not it is running. Nothing short
    /// of a cross-field check catches that.
    ///
    /// The field named is `state_root`, because that is the one to move: the
    /// discovery prefix is global to the whole Home Assistant installation and
    /// changing it taxes every other MQTT device on the network.
    Overlap(Field),
}

impl ConfigError {
    /// Which field was wrong.
    pub const fn field(self) -> Field {
        match self {
            ConfigError::Empty(f)
            | ConfigError::Wildcard(f, _)
            | ConfigError::LeadingSlash(f)
            | ConfigError::TrailingSlash(f)
            | ConfigError::EmptySegment(f)
            | ConfigError::IllegalCharacter(f, _)
            | ConfigError::TooLong(f, _)
            | ConfigError::Overlap(f) => f,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = self.field();
        match self {
            ConfigError::Empty(_) => write!(f, "{field} must not be empty"),
            ConfigError::Wildcard(_, c) => {
                write!(f, "{field} must not contain the MQTT wildcard {c:?}")
            }
            ConfigError::LeadingSlash(_) => write!(f, "{field} must not start with '/'"),
            ConfigError::TrailingSlash(_) => write!(f, "{field} must not end with '/'"),
            ConfigError::EmptySegment(_) => write!(f, "{field} must not contain an empty segment"),
            ConfigError::IllegalCharacter(_, c) => {
                write!(
                    f,
                    "{field} must not contain {c:?}; allowed: a-z A-Z 0-9 _ -"
                )
            }
            ConfigError::TooLong(_, limit) => {
                write!(f, "{field} must be at most {limit} bytes")
            }
            ConfigError::Overlap(_) => write!(
                f,
                "{field} must not equal discovery_prefix or sit inside it; \
                 state published there collides with Home Assistant's own topics",
            ),
        }
    }
}
