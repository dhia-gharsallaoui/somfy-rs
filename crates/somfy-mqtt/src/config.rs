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

use crate::entity::{
    ButtonDiscovery, CalibrationDiscovery, Component, CoverDiscovery, DeviceEntity,
    DiagnosticDiscovery, ShadeTopic,
};
use crate::error::{ConfigError, Field};
use crate::ident::{
    DeviceId, NodeId, ObjectId, UniqueId, MAX_NODE_ID_LEN, MAX_OBJECT_ID_LEN, MAX_SHADE_ID_DIGITS,
};
use crate::setup::{SetupDiscovery, SetupEntity};
use crate::topic::{
    namespaces_overlap, DiscoveryPrefix, StateRoot, Topic, MAX_DISCOVERY_PREFIX_LEN,
    MAX_STATE_ROOT_LEN, TOPIC_CAPACITY,
};
use crate::url::ConfigurationUrl;
use somfy_domain::ShadeId;

/// The last segment of every discovery topic.
const CONFIG_SEGMENT: &str = "config";

/// The segment that groups per-shade state under the state root.
const SHADES_SEGMENT: &str = "shades";

/// The segment that groups the controller's own diagnostics under the state
/// root.
///
/// Distinct from [`SHADES_SEGMENT`] and from [`STATUS_SEGMENT`], so a device
/// entity cannot address what a shade owns however either set grows —
/// `tests/device_entities.rs` checks that against all 256 shade ids rather than
/// leaving it as an observation about three string literals.
const DEVICE_SEGMENT: &str = "device";

/// The availability topic's segment under the state root.
const STATUS_SEGMENT: &str = "status";

/// The segment that groups the add-a-shade form under the state root.
///
/// A fourth namespace beside [`SHADES_SEGMENT`], [`DEVICE_SEGMENT`] and
/// [`STATUS_SEGMENT`], for the same reason the third one is separate: a form
/// entity must not be able to address what a shade or a diagnostic owns however
/// any of the sets grows. `tests/setup_form.rs` checks that against all 256
/// shade ids and every [`DeviceEntity`] rather than leaving it as an
/// observation about four string literals.
const SETUP_SEGMENT: &str = "setup";

/// The segment that turns a form topic into the one the firmware subscribes to.
const SET_SEGMENT: &str = "set";

/// A validated MQTT configuration: two independent namespaces and two
/// identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttConfig {
    discovery_prefix: DiscoveryPrefix,
    state_root: StateRoot,
    node_id: NodeId,
    device_id: DeviceId,
    /// Where a person goes to configure this controller. `None` is the ordinary
    /// answer for a build with no web server in it, and it is why this is not
    /// an argument to [`MqttConfig::new`]: a required value that half the
    /// builds cannot supply becomes a placeholder, and a placeholder here is a
    /// link Home Assistant renders and nobody can follow.
    configuration_url: Option<ConfigurationUrl>,
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
            configuration_url: None,
        })
    }

    /// Point Home Assistant's device page at this controller's own web server.
    ///
    /// Carried in every discovery payload's `device` block, so the link appears
    /// once per device rather than once per entity.
    ///
    /// # Why a builder rather than a fifth argument
    ///
    /// Because there is a real configuration with no answer. A build with no
    /// web server has nothing to link to, and a build whose name does not
    /// resolve — no mDNS responder — has a name that is worse than no link,
    /// because a link that fails to open reads as a device that has broken.
    /// Both are ordinary, so the value is optional and its absence is silence
    /// rather than a placeholder.
    ///
    /// ```
    /// use somfy_mqtt::{
    ///     ConfigurationUrl, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, StateRoot,
    /// };
    ///
    /// let config = MqttConfig::new(
    ///     DiscoveryPrefix::new("homeassistant")?,
    ///     StateRoot::new("somfyrs")?,
    ///     NodeId::new("somfyrs")?,
    ///     DeviceId::new("a1b2c3d4")?,
    /// )?
    /// .with_configuration_url(
    ///     ConfigurationUrl::new("http://somfy-a1b2c3d4.local").expect("a usable URL"),
    /// );
    /// assert_eq!(
    ///     config.configuration_url(),
    ///     Some("http://somfy-a1b2c3d4.local"),
    /// );
    /// # Ok::<(), somfy_mqtt::ConfigError>(())
    /// ```
    #[must_use]
    pub fn with_configuration_url(mut self, url: ConfigurationUrl) -> MqttConfig {
        self.configuration_url = Some(url);
        self
    }

    /// The address Home Assistant's device page links to, if there is one.
    pub fn configuration_url(&self) -> Option<&str> {
        self.configuration_url
            .as_ref()
            .map(ConfigurationUrl::as_str)
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

    /// `{state_root}/device` — the base every diagnostic reading sits under,
    /// and the diagnostic payloads' `~`.
    pub fn device_base(&self) -> Topic {
        self.state_root.topic().segment(DEVICE_SEGMENT).finish()
    }

    /// The absolute topic one device-level entity's reading is published to.
    pub fn device_topic(&self, entity: DeviceEntity) -> Topic {
        self.state_root
            .topic()
            .segment(DEVICE_SEGMENT)
            .segment(entity.slug())
            .finish()
    }

    /// Every device-level entity, paired with its absolute address.
    ///
    /// The device counterpart of [`MqttConfig::shade_topics`], and read by the
    /// round-trip check for the same reason: the payload and the publisher must
    /// come from one table or they will drift.
    pub fn device_topics(&self) -> impl Iterator<Item = (DeviceEntity, Topic)> + '_ {
        DeviceEntity::ALL
            .into_iter()
            .map(move |entity| (entity, self.device_topic(entity)))
    }

    /// The `cover` discovery config for one shade.
    ///
    /// `has_tilt` decides whether the tilt topics are carried. It is the
    /// caller's judgement rather than a read of the stored tilt mode — see
    /// [`ShadeTopic::for_shade`] for why that distinction matters today.
    pub fn cover_discovery<'a>(
        &'a self,
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
            device_id: self.device_id.as_str(),
            configuration_url: self.configuration_url(),
            has_tilt,
        }
    }

    /// The `button` discovery config for one shade's pairing action.
    ///
    /// Takes no `has_tilt`: pairing is not a tilt feature, and the topic it
    /// names exists on every shade.
    pub fn button_discovery<'a>(&'a self, shade: ShadeId, name: &'a str) -> ButtonDiscovery<'a> {
        ButtonDiscovery {
            base: self.shade_base(shade),
            availability: self.availability_topic(),
            // The same object id the cover uses. They do not collide: the
            // component is a separate segment of the discovery topic, and the
            // payload key is device-scoped and lands in a different Home
            // Assistant domain (`button.` rather than `cover.`).
            object_id: ObjectId::for_shade(shade),
            unique_id: UniqueId::for_shade(&self.device_id, Component::Button, shade),
            name,
            device_id: self.device_id.as_str(),
            configuration_url: self.configuration_url(),
        }
    }

    /// The `sensor` discovery config for one shade's calibration state.
    ///
    /// Takes no `has_tilt` for the reason [`MqttConfig::button_discovery`] does
    /// not: the topic it names exists on every shade. The tilt time has its own
    /// provenance in the domain and is deliberately *not* folded into this
    /// state — see `crate::CalibrationState`, whose subject is the two travel
    /// times the position estimate is computed from.
    pub fn calibration_discovery<'a>(
        &'a self,
        shade: ShadeId,
        name: &'a str,
    ) -> CalibrationDiscovery<'a> {
        CalibrationDiscovery {
            base: self.shade_base(shade),
            availability: self.availability_topic(),
            // The same object id the cover and the button use, and distinct for
            // the same reason: `sensor.` is a third Home Assistant domain, and
            // the component is a separate segment of the discovery topic.
            object_id: ObjectId::for_shade(shade),
            unique_id: UniqueId::for_shade(&self.device_id, Component::Sensor, shade),
            name,
            device_id: self.device_id.as_str(),
            configuration_url: self.configuration_url(),
        }
    }

    /// `{state_root}/setup` — the base every form topic sits under, and the
    /// setup payloads' `~`.
    pub fn setup_base(&self) -> Topic {
        self.state_root.topic().segment(SETUP_SEGMENT).finish()
    }

    /// The absolute topic one form entity's value is published to.
    ///
    /// Meaningful only where [`SetupEntity::has_state`] is true. It is built
    /// unconditionally anyway, because the retirement clears every topic the
    /// form *could* own rather than every topic it did — the same asymmetry
    /// [`MqttConfig::retire_shade`] uses, and for the same reason: clearing a
    /// topic nothing was published to costs one packet the broker discards,
    /// while failing to clear one leaves a retained value with no entity behind
    /// it.
    pub fn setup_topic(&self, entity: SetupEntity) -> Topic {
        self.state_root
            .topic()
            .segment(SETUP_SEGMENT)
            .segment(entity.leaf())
            .finish()
    }

    /// The absolute topic one form entity takes commands on.
    ///
    /// Meaningful only where [`SetupEntity::accepts_command`] is true, and
    /// **never published to** — a form command topic is subscribed, so it can
    /// never carry a retained message from this device (R6).
    pub fn setup_command_topic(&self, entity: SetupEntity) -> Topic {
        self.state_root
            .topic()
            .segment(SETUP_SEGMENT)
            .segment(entity.leaf())
            .segment(SET_SEGMENT)
            .finish()
    }

    /// Every form entity, paired with its state topic and its command topic.
    ///
    /// The counterpart of [`MqttConfig::shade_topics`] and
    /// [`MqttConfig::device_topics`], and read by the round-trip check for the
    /// same reason: the payload and the publisher must come from one table or
    /// they will drift.
    pub fn setup_topics(
        &self,
    ) -> impl Iterator<Item = (SetupEntity, Option<Topic>, Option<Topic>)> + '_ {
        SetupEntity::ALL.into_iter().map(move |entity| {
            (
                entity,
                entity.has_state().then(|| self.setup_topic(entity)),
                entity
                    .accepts_command()
                    .then(|| self.setup_command_topic(entity)),
            )
        })
    }

    /// The discovery config for one entity of the add-a-shade form.
    ///
    /// Takes no value, exactly as [`MqttConfig::diagnostic_discovery`] does:
    /// what the entity currently *holds* is published separately, on the topic
    /// this payload names.
    pub fn setup_discovery(&self, entity: SetupEntity) -> SetupDiscovery<'_> {
        SetupDiscovery {
            base: self.setup_base(),
            availability: self.availability_topic(),
            object_id: ObjectId::for_setup(entity),
            unique_id: UniqueId::for_setup(&self.device_id, entity),
            device_id: self.device_id.as_str(),
            configuration_url: self.configuration_url(),
            entity,
        }
    }

    /// The discovery config for one device-level entity.
    ///
    /// Takes no value: what the entity *reports* is published separately, on
    /// the topic this payload names, exactly as a shade's position is. That
    /// split is what lets the announcement be built without the firmware's
    /// readings being available to this crate.
    pub fn diagnostic_discovery(&self, entity: DeviceEntity) -> DiagnosticDiscovery<'_> {
        DiagnosticDiscovery {
            base: self.device_base(),
            availability: self.availability_topic(),
            object_id: ObjectId::for_device(entity),
            unique_id: UniqueId::for_device(&self.device_id, entity),
            device_id: self.device_id.as_str(),
            configuration_url: self.configuration_url(),
            entity,
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

/// `{state_root}/device/{slug}` at its widest.
const WORST_DEVICE_TOPIC_LEN: usize =
    MAX_STATE_ROOT_LEN + 1 + DEVICE_SEGMENT.len() + 1 + DeviceEntity::MAX_SLUG_LEN;

/// `{state_root}/setup` at its widest — also the setup payloads' `~`.
pub(crate) const WORST_SETUP_BASE_LEN: usize = MAX_STATE_ROOT_LEN + 1 + SETUP_SEGMENT.len();

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
const _: () = assert!(
    TOPIC_CAPACITY >= WORST_DEVICE_TOPIC_LEN,
    "TOPIC_CAPACITY is too small for the longest device topic",
);
const _: () = assert!(
    TOPIC_CAPACITY >= crate::setup::WORST_SETUP_TOPIC_LEN,
    "TOPIC_CAPACITY is too small for the longest setup topic",
);
