//! The configured device, and every topic derived from it.
//!
//! # The shape of the API is the requirement
//!
//! Each method below draws on exactly one namespace.
//! [`MqttConfig::discovery_topic`] builds from the discovery prefix;
//! [`MqttConfig::availability_topic`], [`MqttConfig::shade_base`] and
//! [`MqttConfig::shade_topic`] build from the state root. Nothing here reaches
//! for both, and — see [`crate::DiscoveryPrefix`] — nothing anywhere could,
//! because neither root will hand out its text.
//!
//! [`MqttConfig::new`] takes the four values as distinct types rather than four
//! strings in a row, so the discovery prefix and the state root cannot be
//! swapped at the call site either. Passing them the wrong way round is not a
//! mistake to catch in review; it is a program that does not compile.
//!
//! It is also the one place a *pair* of values is judged. Each root is
//! validated where it is built, but two individually valid roots can still name
//! the same namespace — see [`crate::ConfigError::Overlap`].

use crate::entity::{Component, CoverDiscovery, ShadeTopic};
use crate::error::{ConfigError, Field};
use crate::ident::{
    DeviceId, NodeId, ObjectId, UniqueId, MAX_NODE_ID_LEN, MAX_OBJECT_ID_LEN, MAX_SHADE_ID_DIGITS,
};
use crate::topic::{
    namespaces_overlap, DiscoveryPrefix, StateRoot, Topic, MAX_DISCOVERY_PREFIX_LEN,
    MAX_STATE_ROOT_LEN, TOPIC_CAPACITY,
};
use somfy_domain::ShadeId;

/// The last segment of every discovery topic.
const CONFIG_SEGMENT: &str = "config";

/// The segment that groups per-shade state under the state root.
const SHADES_SEGMENT: &str = "shades";

/// The availability topic's segment under the state root.
const STATUS_SEGMENT: &str = "status";

/// A validated MQTT configuration: two independent namespaces and two
/// identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttConfig {
    discovery_prefix: DiscoveryPrefix,
    state_root: StateRoot,
    node_id: NodeId,
    device_id: DeviceId,
}

impl MqttConfig {
    /// Assemble a configuration from four already-validated values.
    ///
    /// Each value was refused or accepted at its own point of entry, so the
    /// only thing left to check is the one fault that belongs to a *pair*: the
    /// two namespaces overlapping. See [`ConfigError::Overlap`] — both roots
    /// can be individually valid and still put availability on Home
    /// Assistant's own birth topic.
    ///
    /// ```
    /// use somfy_mqtt::{DeviceId, DiscoveryPrefix, MqttConfig, NodeId, StateRoot};
    ///
    /// let config = MqttConfig::new(
    ///     DiscoveryPrefix::new("homeassistant")?,
    ///     StateRoot::new("somfyrs")?,
    ///     NodeId::new("somfyrs")?,
    ///     DeviceId::new("a1b2c3d4")?,
    /// )?;
    /// assert_eq!(config.availability_topic().as_str(), "somfyrs/status");
    /// # Ok::<(), somfy_mqtt::ConfigError>(())
    /// ```
    ///
    /// The two roots cannot be passed the wrong way round:
    ///
    /// ```compile_fail,E0308
    /// use somfy_mqtt::{DeviceId, DiscoveryPrefix, MqttConfig, NodeId, StateRoot};
    ///
    /// let config = MqttConfig::new(
    ///     StateRoot::new("somfyrs").unwrap(),
    ///     DiscoveryPrefix::new("homeassistant").unwrap(),
    ///     NodeId::new("somfyrs").unwrap(),
    ///     DeviceId::new("a1b2c3d4").unwrap(),
    /// );
    /// ```
    pub fn new(
        discovery_prefix: DiscoveryPrefix,
        state_root: StateRoot,
        node_id: NodeId,
        device_id: DeviceId,
    ) -> Result<MqttConfig, ConfigError> {
        if namespaces_overlap(&discovery_prefix, &state_root) {
            return Err(ConfigError::Overlap(Field::StateRoot));
        }
        Ok(MqttConfig {
            discovery_prefix,
            state_root,
            node_id,
            device_id,
        })
    }

    /// The stable device identifier, for building `unique_id`s.
    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// The optional device segment of a discovery topic.
    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// `{discovery_prefix}/{component}/{node_id}/{object_id}/config`.
    ///
    /// The component comes **immediately** after the prefix. Home Assistant
    /// silently ignores a config whose second segment is anything else, so the
    /// order here is the whole contract.
    ///
    /// The state root appears nowhere in this topic. That is R1, and it is the
    /// single reason MQTT discovery is unusable on deployed firmware:
    /// there, the state root was prepended, and the resulting
    /// `mydevice/homeassistant/cover/1/config` was not under Home
    /// Assistant's prefix at all.
    pub fn discovery_topic(&self, component: Component, object_id: &ObjectId) -> Topic {
        self.discovery_prefix
            .topic()
            .segment(component.as_str())
            .segment(self.node_id.as_str())
            .segment(object_id.as_str())
            .segment(CONFIG_SEGMENT)
            .finish()
    }

    /// `{state_root}/status` — where `online` and the last will go.
    ///
    /// Under the state root, never under the discovery prefix.
    /// `{discovery_prefix}/status` is Home Assistant's own birth and will
    /// topic: availability published there is not merely misplaced, it is
    /// actively wrong, because HA's birth message would mark this device
    /// available at the moment HA restarts, whether or not the device is
    /// running.
    pub fn availability_topic(&self) -> Topic {
        self.state_root.topic().segment(STATUS_SEGMENT).finish()
    }

    /// `{state_root}/shades/{id}` — the payload's `~`.
    ///
    /// Absolute, with no leading slash. A leading slash here is the second of
    /// the three observed failures: the payload said `/shades/1` while the
    /// publisher wrote `shades/1`, which are different topics, so every entity
    /// was permanently unavailable. It cannot arise now because an empty state
    /// root is refused rather than accepted.
    pub fn shade_base(&self, shade: ShadeId) -> Topic {
        self.state_root
            .topic()
            .segment(SHADES_SEGMENT)
            .number(shade.0)
            .finish()
    }

    /// The absolute topic for one of a shade's [`ShadeTopic`]s.
    ///
    /// This is what the firmware publishes to or subscribes to, and — after `~`
    /// expansion — what the discovery payload resolves to. Both come from
    /// [`ShadeTopic::segments`].
    pub fn shade_topic(&self, shade: ShadeId, topic: ShadeTopic) -> Topic {
        let mut buf = self
            .state_root
            .topic()
            .segment(SHADES_SEGMENT)
            .number(shade.0);
        for segment in topic.segments() {
            buf = buf.segment(segment);
        }
        buf.finish()
    }

    /// Every topic a shade owns, paired with its absolute address.
    ///
    /// Filter on [`ShadeTopic::role`] to get the publish set or the subscribe
    /// set; both come from here so they cannot drift from the payload.
    pub fn shade_topics(
        &self,
        shade: ShadeId,
        has_tilt: bool,
    ) -> impl Iterator<Item = (ShadeTopic, Topic)> + '_ {
        ShadeTopic::for_shade(has_tilt).map(move |topic| (topic, self.shade_topic(shade, topic)))
    }

    /// The `cover` discovery config for one shade.
    ///
    /// `has_tilt` decides whether the tilt topics are carried. It is the
    /// caller's judgement rather than a read of the stored tilt mode — see
    /// [`ShadeTopic::for_shade`] for why that distinction matters today.
    pub fn cover_discovery<'a>(
        &self,
        shade: ShadeId,
        name: &'a str,
        has_tilt: bool,
    ) -> CoverDiscovery<'a> {
        CoverDiscovery {
            base: self.shade_base(shade),
            availability: self.availability_topic(),
            object_id: ObjectId::for_shade(shade),
            unique_id: UniqueId::for_shade(&self.device_id, Component::Cover, shade),
            name,
            has_tilt,
        }
    }
}

// ---------------------------------------------------------------------------
// Capacity proofs
//
// Topic construction is infallible, which is only honest if the buffer is
// provably large enough. Each constant below is the longest topic of its shape
// that this crate's own limits permit; the assertions turn "it will fit" into
// something the compiler checks rather than something a reader hopes.
// ---------------------------------------------------------------------------

/// `{discovery_prefix}/{component}/{node_id}/{object_id}/config` at its widest.
const WORST_DISCOVERY_TOPIC_LEN: usize = MAX_DISCOVERY_PREFIX_LEN
    + 1
    + Component::MAX_LEN
    + 1
    + MAX_NODE_ID_LEN
    + 1
    + MAX_OBJECT_ID_LEN
    + 1
    + CONFIG_SEGMENT.len();

/// `{state_root}/shades/{id}` at its widest — also the payload's `~`.
pub(crate) const WORST_SHADE_BASE_LEN: usize =
    MAX_STATE_ROOT_LEN + 1 + SHADES_SEGMENT.len() + 1 + MAX_SHADE_ID_DIGITS;

/// `{state_root}/shades/{id}/{relative}` at its widest.
const WORST_SHADE_TOPIC_LEN: usize = WORST_SHADE_BASE_LEN + ShadeTopic::MAX_RELATIVE_LEN;

/// `{state_root}/status` at its widest.
pub(crate) const WORST_AVAILABILITY_LEN: usize = MAX_STATE_ROOT_LEN + 1 + STATUS_SEGMENT.len();

const _: () = assert!(
    TOPIC_CAPACITY >= WORST_DISCOVERY_TOPIC_LEN,
    "TOPIC_CAPACITY is too small for the longest discovery topic",
);
const _: () = assert!(
    TOPIC_CAPACITY >= WORST_SHADE_TOPIC_LEN,
    "TOPIC_CAPACITY is too small for the longest shade topic",
);
const _: () = assert!(
    TOPIC_CAPACITY >= WORST_AVAILABILITY_LEN,
    "TOPIC_CAPACITY is too small for the availability topic",
);
