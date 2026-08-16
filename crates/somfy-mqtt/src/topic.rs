//! The two roots, the builder that turns one of them into a topic, and the
//! only string type anything is allowed to publish to.
//!
//! # Why this is one module
//!
//! `discovery_prefix` and `state_root` are independent namespaces. Joining them
//! is the single fault that made Home Assistant discovery unusable on the
//! deployed firmware, so the goal here is not to *avoid* writing
//! that join — it is to leave nowhere to write it.
//!
//! The mechanism is module privacy, and it is deliberately confined to this one
//! file so that it can be checked by reading it:
//!
//! - [`DiscoveryPrefix`] and [`StateRoot`] hold their text in a private field
//!   and expose **no** way to read it — no `as_str`, no `Display`, no `Deref`,
//!   no `AsRef<str>`, no public field. Not even to the rest of this crate.
//! - The only readers of those fields are [`DiscoveryPrefix::topic`] and
//!   [`StateRoot::topic`], each of which reads its own root and seeds a
//!   [`TopicBuf`] with it.
//! - [`TopicBuf`] can be seeded exactly once, at construction, and afterwards
//!   accepts only single segments. It has no method that takes a root.
//!
//! So concatenating the two requires reading text this module never hands out.
//! From outside the crate that is impossible; inside it, it would require
//! adding a method here, in a file whose entire subject is why that method must
//! not exist.
//!
//! # Why [`Topic`] is a type and not a `&str`
//!
//! A finished topic has to be readable — something has to hand the bytes to a
//! broker. So [`Topic::as_str`] exists, and a caller with two topics can
//! certainly paste them together into some other string. What it cannot do is
//! turn the result back into a [`Topic`]: the field is private and there is no
//! public constructor. Every seam that publishes or subscribes takes a
//! `&Topic`, so a hand-built string has nowhere to go. This is the same
//! enforcement `somfy-store` uses to make a transmission impossible without a
//! committed rolling code.

use crate::error::{ConfigError, Field};
use crate::validate::{check_root, is_segment};
use core::fmt;
use core::fmt::Write as _;
use heapless::String;

/// Bytes a fully-built topic may occupy.
///
/// Chosen so that construction is **infallible**: `config.rs` asserts at
/// compile time that this exceeds the longest topic the crate can build, from
/// the individual field limits below. That assertion is what lets a topic
/// builder panic on overflow rather than truncate — a truncated topic is a
/// valid-looking address for something else entirely, which is exactly the
/// class of fault this crate exists to remove.
pub const TOPIC_CAPACITY: usize = 256;

/// Bytes a `discovery_prefix` may occupy.
///
/// Home Assistant's own default is `homeassistant` (13 bytes). The limit is
/// generous rather than derived; what matters is that it exists, so that
/// [`TOPIC_CAPACITY`] can be proven sufficient at compile time.
pub const MAX_DISCOVERY_PREFIX_LEN: usize = 64;

/// Bytes a `state_root` may occupy. See [`MAX_DISCOVERY_PREFIX_LEN`].
pub const MAX_STATE_ROOT_LEN: usize = 64;

/// Where Home Assistant looks for discovery configs.
///
/// Home Assistant supports exactly **one** discovery prefix, and it is global
/// to the installation. A device that forces it to be changed taxes every other
/// MQTT device on that network for as long as it is installed, so the default
/// is `homeassistant` and the configuration exists only for estates that have
/// already moved it for some other reason.
///
/// This value addresses *discovery configs only*. It is never part of a state,
/// command or availability topic. Putting availability at
/// `{discovery_prefix}/status`, in particular, collides with Home Assistant's
/// own birth and will topic: HA's birth message would then mark this device
/// available while it is offline, which is worse than having no availability at
/// all.
///
/// # Concatenating the two roots does not compile
///
/// There is no way to read the text back out, so there is nothing to paste:
///
/// ```compile_fail,E0599
/// let prefix = somfy_mqtt::DiscoveryPrefix::new("homeassistant").unwrap();
/// let root = somfy_mqtt::StateRoot::new("somfyrs").unwrap();
/// let bug = format!("{}/{}", prefix.as_str(), root.as_str());
/// ```
///
/// Nor through `Display`, which is deliberately not implemented:
///
/// ```compile_fail,E0277
/// let prefix = somfy_mqtt::DiscoveryPrefix::new("homeassistant").unwrap();
/// let root = somfy_mqtt::StateRoot::new("somfyrs").unwrap();
/// let bug = format!("{prefix}/{root}");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryPrefix(String<MAX_DISCOVERY_PREFIX_LEN>);

impl DiscoveryPrefix {
    /// Validate and store a discovery prefix, or say which rule it broke.
    ///
    /// ```
    /// use somfy_mqtt::{ConfigError, DiscoveryPrefix, Field};
    ///
    /// assert!(DiscoveryPrefix::new("homeassistant").is_ok());
    /// assert_eq!(
    ///     DiscoveryPrefix::new(""),
    ///     Err(ConfigError::Empty(Field::DiscoveryPrefix)),
    /// );
    /// ```
    pub fn new(value: &str) -> Result<DiscoveryPrefix, ConfigError> {
        check_root(value, Field::DiscoveryPrefix, MAX_DISCOVERY_PREFIX_LEN)?;
        let mut inner = String::new();
        inner
            .push_str(value)
            .map_err(|_| ConfigError::TooLong(Field::DiscoveryPrefix, MAX_DISCOVERY_PREFIX_LEN))?;
        Ok(DiscoveryPrefix(inner))
    }

    /// Start a topic in the discovery namespace.
    ///
    /// One of exactly two readers of a root's text in this crate. The other is
    /// [`StateRoot::topic`]; neither can see the other's field.
    pub(crate) fn topic(&self) -> TopicBuf {
        TopicBuf::rooted(&self.0)
    }
}

/// Where this device's own state, command and availability topics live.
///
/// Independent of [`DiscoveryPrefix`] in both directions: a discovery config
/// published under `homeassistant/` points at state under this root through the
/// payload's `~` field, which is the entire reason `~` exists.
///
/// See [`DiscoveryPrefix`] for the compile-fail demonstrations that the two
/// cannot be joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRoot(String<MAX_STATE_ROOT_LEN>);

impl StateRoot {
    /// Validate and store a state root, or say which rule it broke.
    ///
    /// ```
    /// use somfy_mqtt::{ConfigError, Field, StateRoot};
    ///
    /// assert!(StateRoot::new("home/blinds").is_ok());
    /// assert_eq!(StateRoot::new("home//blinds"), Err(ConfigError::EmptySegment(Field::StateRoot)));
    /// ```
    pub fn new(value: &str) -> Result<StateRoot, ConfigError> {
        check_root(value, Field::StateRoot, MAX_STATE_ROOT_LEN)?;
        let mut inner = String::new();
        inner
            .push_str(value)
            .map_err(|_| ConfigError::TooLong(Field::StateRoot, MAX_STATE_ROOT_LEN))?;
        Ok(StateRoot(inner))
    }

    /// Start a topic in the state namespace. See [`DiscoveryPrefix::topic`].
    pub(crate) fn topic(&self) -> TopicBuf {
        TopicBuf::rooted(&self.0)
    }
}

/// True if the two namespaces are the same, or one sits inside the other.
///
/// **The only place in this crate where both roots' text is visible at once**,
/// and the reason it is safe: it returns a `bool`. It cannot build anything, and
/// [`TopicBuf`] is still seeded from exactly one root, so the guarantee that a
/// topic cannot be made from both is unaffected.
///
/// Why the check has to exist: each root can be perfectly valid on its own and
/// still be wrong *together*. Set both to `homeassistant` — the second
/// configuration anyone tries after the first one fails — and availability
/// lands on `homeassistant/status`, which is Home Assistant's own birth and
/// will topic. HA's birth message then marks the device available at the moment
/// HA restarts, whether or not the device is running. That is worse than having
/// no availability at all, and neither root is individually invalid, so nothing
/// short of a cross-field check catches it.
///
/// Nesting is refused for a related reason: state published beneath the
/// discovery prefix is state Home Assistant tries to parse as discovery
/// configs.
///
/// The comparison is at `/` boundaries, so `home` and `homeassistant` are
/// unrelated namespaces rather than an overlap.
pub(crate) fn namespaces_overlap(prefix: &DiscoveryPrefix, root: &StateRoot) -> bool {
    nests(&prefix.0, &root.0)
}

fn nests(a: &str, b: &str) -> bool {
    fn under(parent: &str, child: &str) -> bool {
        child.len() > parent.len()
            && child.starts_with(parent)
            && child.as_bytes()[parent.len()] == b'/'
    }
    a == b || under(a, b) || under(b, a)
}

/// A complete, valid MQTT topic.
///
/// Guaranteed non-empty, with no leading or trailing `/` and no empty segment,
/// because the only things that can build one are the builders in this crate
/// and their inputs are validated before they get here.
///
/// It cannot be built from a string:
///
/// ```compile_fail,E0423
/// let bug = somfy_mqtt::Topic(String::from("homeassistant//cover/1/config"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic(String<TOPIC_CAPACITY>);

impl Topic {
    /// The bytes to hand a broker.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false — a topic cannot be empty. Present because clippy asks for
    /// it wherever `len` exists, and because asserting it in a test is cheaper
    /// than reasoning about it.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A topic under construction, seeded by exactly one root.
///
/// Seeding happens at construction and nowhere else: there is no `root` method,
/// and [`TopicBuf::segment`] takes a single segment, which a root — the only
/// multi-segment value in this crate — is not. Together with the roots having
/// no text accessor, that is what makes a two-root topic unwritable.
pub(crate) struct TopicBuf(String<TOPIC_CAPACITY>);

impl TopicBuf {
    /// Seed with a root's text. Private to this module by construction: the
    /// only callers are [`DiscoveryPrefix::topic`] and [`StateRoot::topic`],
    /// which are the only code anywhere that can see a root's field.
    fn rooted(root: &str) -> TopicBuf {
        let mut inner = String::new();
        push(&mut inner, root);
        TopicBuf(inner)
    }

    /// Append one `/`-separated segment.
    ///
    /// The debug assertion is a backstop, not the guarantee. Everything that
    /// reaches here is either a literal chosen by the firmware or a value
    /// already validated to `[a-zA-Z0-9_-]`, so a multi-segment or empty push
    /// would be a bug in this crate rather than a bad configuration — and an
    /// empty segment is precisely what produced `homeassistant//cover/…` in the
    /// field.
    pub(crate) fn segment(mut self, segment: &str) -> TopicBuf {
        debug_assert!(
            is_segment(segment),
            "topic segments are single and non-empty; got {segment:?}",
        );
        push(&mut self.0, "/");
        push(&mut self.0, segment);
        self
    }

    /// Append a numeric segment, such as a shade id.
    pub(crate) fn number(mut self, value: u8) -> TopicBuf {
        push(&mut self.0, "/");
        write!(&mut self.0, "{value}").expect("topic capacity proven at compile time");
        self
    }

    /// Finish. There is no other way to obtain a [`Topic`].
    pub(crate) fn finish(self) -> Topic {
        Topic(self.0)
    }
}

/// Append, or panic.
///
/// `config.rs` proves at compile time that [`TOPIC_CAPACITY`] exceeds the
/// longest topic this crate can build, so this cannot fail. It panics rather
/// than truncating if that proof is ever broken: a truncated topic is a
/// perfectly well-formed address for the wrong thing, and would be published,
/// retained, and never questioned.
fn push(into: &mut String<TOPIC_CAPACITY>, text: &str) {
    into.push_str(text)
        .expect("topic capacity proven at compile time");
}
