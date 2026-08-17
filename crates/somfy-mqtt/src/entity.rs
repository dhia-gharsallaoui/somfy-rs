//! The entity model, and the one table both halves of the integration are
//! derived from.
//!
//! # Why the table exists
//!
//! A discovery payload says where a topic is; the firmware publishes to where
//! it thinks the topic is. When those two disagree the entity appears in Home
//! Assistant, looks configured, and is permanently unavailable — which is
//! exactly what happened in the field, and nothing anywhere noticed, because
//! there was nothing that compared them.
//!
//! [`ShadeTopic`] is that comparison made structural. One enum owns, for each
//! topic a shade has:
//!
//! - its path segments, from which both the absolute topic and the `~`-relative
//!   payload string are built;
//! - its [`TopicRole`] — whether the firmware publishes it or subscribes to it;
//! - the discovery-payload key that carries it, if any.
//!
//! `MqttConfig::shade_topics` reads it to produce what the firmware acts on.
//! [`CoverDiscovery::render`] reads it to produce what Home Assistant acts on.
//! Neither can be changed without moving the other, and `tests/round_trip.rs`
//! checks the agreement against rendered bytes rather than against the struct
//! that produced them.

use crate::config;
use crate::ident::{ObjectId, UniqueId};
use crate::topic::Topic;
use heapless::String;

/// Bytes a rendered discovery payload may occupy.
///
/// Proven sufficient below for every input this crate's own limits permit,
/// including a name made entirely of control characters, each of which escapes
/// to six bytes.
pub const PAYLOAD_CAPACITY: usize = 1152;

/// Longest shade name the payload budget assumes.
///
/// Matches the capacity of `somfy_domain::ShadeConfig::name`, which is where
/// the name comes from. A caller passing something longer gets
/// [`PayloadError::TooLong`] rather than a truncated payload.
pub const MAX_NAME_LEN: usize = 32;

/// What every discovery payload says the entity belongs to.
///
/// A literal rather than configuration: it names the project, and a device that
/// let an operator set it would put the same board under two manufacturers on
/// two estates.
const MANUFACTURER: &str = "somfy-rs";

/// What the device is called in Home Assistant, before the device id.
///
/// The id follows it so that two controllers on one estate are distinguishable
/// without anyone renaming either. Home Assistant lets a user rename the device
/// afterwards, and that rename survives — it is stored against the
/// `identifiers`, which do not move.
const DEVICE_NAME_PREFIX: &str = "somfy-rs ";

/// A Home Assistant MQTT component.
///
/// The component is the segment immediately after the discovery prefix, and
/// getting it wrong is silent: `homeassistant/somfyrs/cover/1/config` is
/// ignored without comment, while `homeassistant/cover/somfyrs/1/config`
/// creates the entity.
///
/// It is an enum rather than a string because it is a literal from Home
/// Assistant's own set, chosen by the firmware. There is deliberately no way
/// for a configured value to become the component segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// A shade. The only component this crate builds payloads for so far.
    Cover,
    /// Numeric or textual diagnostics.
    Sensor,
    /// On/off diagnostics, such as sun and wind sensing.
    BinarySensor,
    /// A stateless action.
    Button,
    /// A toggle.
    Switch,
    /// The firmware update entity.
    Update,
}

impl Component {
    /// Every component this crate can emit.
    pub const ALL: [Component; 6] = [
        Component::Cover,
        Component::Sensor,
        Component::BinarySensor,
        Component::Button,
        Component::Switch,
        Component::Update,
    ];

    /// Bytes the longest component name occupies, for the capacity proofs.
    pub const MAX_LEN: usize = longest_component();

    /// The literal Home Assistant expects.
    pub const fn as_str(self) -> &'static str {
        match self {
            Component::Cover => "cover",
            Component::Sensor => "sensor",
            Component::BinarySensor => "binary_sensor",
            Component::Button => "button",
            Component::Switch => "switch",
            Component::Update => "update",
        }
    }
}

const fn longest_component() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < Component::ALL.len() {
        let len = Component::ALL[i].as_str().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

/// One fact the controller reports about **itself**, rather than about a shade.
///
/// # The rule that decides what is in here
///
/// **An entity backed by nothing is worse than an absent one.** A reading that
/// never changes and a control that never moves both present as a device fault
/// rather than as an unimplemented feature — which is exactly the failure the
/// requirements spec's acceptance criterion names: *"Appearing is not working;
/// the C++ build produced three entities that were permanently
/// `unavailable`."*
///
/// So every variant below is a value the firmware already holds and already
/// prints at boot. The ones it does not hold are **absent**, not stubbed, and
/// `docs/provenance.md` records each omission with the condition for adding it
/// — sun and wind sensing (the domain defers them), the last received frame
/// (the frame channel has one consumer), a firmware-update entity (there is no
/// OTA to drive it).
///
/// # Why these are device-level rather than per-shade
///
/// None of them is about a shade, so announcing one per shade would report the
/// same number several times and turn an announcement that costs `k + 3N` into
/// one that costs `k·N`. [`crate::SHADE_COMPONENTS`] carries the per-shade set;
/// this carries the per-device one, and both halves of the lifecycle read
/// whichever applies.
///
/// # Every variant is published
///
/// There is deliberately no `role`: nothing subscribes in the device namespace,
/// and adding a device-level *command* would need a subscription in
/// `MqttConfig::announce` as well as a variant here, which is a change big
/// enough to be worth noticing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEntity {
    /// Seconds since boot.
    Uptime,
    /// The station's signal strength, in dBm.
    WifiSignal,
    /// Bytes of heap not currently in use.
    HeapFree,
    /// The largest the heap has been since boot.
    ///
    /// The figure `crates/firmware/src/heap.rs` sizes the heap from, and the
    /// one measurement Plan 5 has been carrying forward unread under real
    /// traffic since Task 2.
    HeapPeak,
    /// Slots in the rolling-code region that hold neither a valid record nor
    /// blank flash.
    ///
    /// Above zero on a device nobody power-cut, this is the single most
    /// operationally important thing the controller knows about itself — and
    /// until now it was visible only on a serial cable.
    RollcodeDamaged,
}

impl DeviceEntity {
    /// Every device-level entity. **Read by both halves of the lifecycle**, so
    /// a variant added here is announced and retired together.
    pub const ALL: [DeviceEntity; 5] = [
        DeviceEntity::Uptime,
        DeviceEntity::WifiSignal,
        DeviceEntity::HeapFree,
        DeviceEntity::HeapPeak,
        DeviceEntity::RollcodeDamaged,
    ];

    /// Bytes the longest [`slug`](DeviceEntity::slug) occupies.
    pub const MAX_SLUG_LEN: usize = longest_slug();

    /// Bytes the longest [`label`](DeviceEntity::label) occupies.
    pub const MAX_LABEL_LEN: usize = longest_label();

    /// The topic segment and `object_id` this entity uses.
    ///
    /// A firmware literal in `[a-z_]`, so it satisfies R2's character class by
    /// construction — there is no user text anywhere near it.
    pub const fn slug(self) -> &'static str {
        match self {
            DeviceEntity::Uptime => "uptime",
            DeviceEntity::WifiSignal => "wifi_signal",
            DeviceEntity::HeapFree => "heap_free",
            DeviceEntity::HeapPeak => "heap_peak",
            DeviceEntity::RollcodeDamaged => "rollcode_damaged",
        }
    }

    /// What Home Assistant displays.
    pub const fn label(self) -> &'static str {
        match self {
            DeviceEntity::Uptime => "Uptime",
            DeviceEntity::WifiSignal => "Wi-Fi signal",
            DeviceEntity::HeapFree => "Free heap",
            DeviceEntity::HeapPeak => "Peak heap use",
            DeviceEntity::RollcodeDamaged => "Damaged rolling-code slots",
        }
    }

    /// Which Home Assistant component this entity is.
    ///
    /// All sensors today. It is a method rather than a constant so that a
    /// binary sensor joining the set is a change to one arm rather than to the
    /// topic builder.
    pub const fn component(self) -> Component {
        match self {
            DeviceEntity::Uptime
            | DeviceEntity::WifiSignal
            | DeviceEntity::HeapFree
            | DeviceEntity::HeapPeak
            | DeviceEntity::RollcodeDamaged => Component::Sensor,
        }
    }

    /// Home Assistant's `device_class`, which is what decides the icon and how
    /// the value is formatted.
    pub const fn device_class(self) -> Option<&'static str> {
        match self {
            DeviceEntity::Uptime => Some("duration"),
            DeviceEntity::WifiSignal => Some("signal_strength"),
            DeviceEntity::HeapFree | DeviceEntity::HeapPeak => Some("data_size"),
            // A count of damaged flash slots is not one of Home Assistant's
            // classes, and picking a near-miss would change how the number is
            // displayed for no gain.
            DeviceEntity::RollcodeDamaged => None,
        }
    }

    /// The unit, which must be one Home Assistant accepts for the
    /// [`device_class`](DeviceEntity::device_class) above.
    pub const fn unit(self) -> Option<&'static str> {
        match self {
            DeviceEntity::Uptime => Some("s"),
            DeviceEntity::WifiSignal => Some("dBm"),
            DeviceEntity::HeapFree | DeviceEntity::HeapPeak => Some("B"),
            DeviceEntity::RollcodeDamaged => None,
        }
    }

    /// Home Assistant's `state_class`, which is what puts the reading into the
    /// long-term statistics an operator can graph.
    pub const fn state_class(self) -> Option<&'static str> {
        match self {
            DeviceEntity::Uptime
            | DeviceEntity::WifiSignal
            | DeviceEntity::HeapFree
            | DeviceEntity::HeapPeak
            | DeviceEntity::RollcodeDamaged => Some("measurement"),
        }
    }
}

const fn longest_slug() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < DeviceEntity::ALL.len() {
        let len = DeviceEntity::ALL[i].slug().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

const fn longest_label() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < DeviceEntity::ALL.len() {
        let len = DeviceEntity::ALL[i].label().len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

/// Which way a topic flows.
///
/// A command topic the firmware only publishes to, or a state topic it only
/// subscribes to, is a payload that parses cleanly and does nothing. The
/// round-trip test checks the direction as well as the address for that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicRole {
    /// The firmware publishes here; Home Assistant reads.
    Published,
    /// The firmware subscribes here; Home Assistant writes.
    ///
    /// Nothing on a subscribed topic may ever be published retained. A retained
    /// command replays on every reconnect, which is a shade that closes itself
    /// each time the broker restarts.
    Subscribed,
}

/// Every topic one shade owns.
///
/// The single source of truth described in the module docs. Adding a topic here
/// adds it to the firmware's publish/subscribe set and to the discovery payload
/// at once; there is no way to add it to only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeTopic {
    /// Current position, 0 open to 100 closed.
    Position,
    /// Current motion, as Home Assistant's cover state vocabulary.
    State,
    /// The shade's configured name, for anyone reading the broker directly.
    /// Carried by no payload key — Home Assistant takes the name from the
    /// discovery config, not from a topic.
    Name,
    /// Open, close and stop commands.
    Command,
    /// A target position to seek.
    SetPosition,
    /// Current tilt, where the shade has a tilt axis.
    TiltStatus,
    /// Tilt commands, where the shade has a tilt axis.
    TiltCommand,
}

impl ShadeTopic {
    /// Every topic, in publish order. Tilt topics are last so that the set for
    /// a non-tilt shade is a prefix of the set for a tilt-capable one.
    pub const ALL: [ShadeTopic; 7] = [
        ShadeTopic::Position,
        ShadeTopic::State,
        ShadeTopic::Name,
        ShadeTopic::Command,
        ShadeTopic::SetPosition,
        ShadeTopic::TiltStatus,
        ShadeTopic::TiltCommand,
    ];

    /// Bytes the longest relative path occupies, separators included, for the
    /// capacity proofs.
    pub const MAX_RELATIVE_LEN: usize = longest_relative();

    /// The path below the shade's base, as separate segments.
    ///
    /// Separate rather than pre-joined because a topic segment is the unit the
    /// builder accepts: anything holding a `/` would have to be pushed as one
    /// piece, and a value that may contain `/` is the shape of the bug this
    /// crate is about.
    pub const fn segments(self) -> &'static [&'static str] {
        match self {
            ShadeTopic::Position => &["position"],
            ShadeTopic::State => &["direction"],
            ShadeTopic::Name => &["name"],
            ShadeTopic::Command => &["direction", "set"],
            ShadeTopic::SetPosition => &["target", "set"],
            ShadeTopic::TiltStatus => &["tilt"],
            ShadeTopic::TiltCommand => &["tilt", "set"],
        }
    }

    /// Which way this topic flows.
    pub const fn role(self) -> TopicRole {
        match self {
            ShadeTopic::Position
            | ShadeTopic::State
            | ShadeTopic::Name
            | ShadeTopic::TiltStatus => TopicRole::Published,
            ShadeTopic::Command | ShadeTopic::SetPosition | ShadeTopic::TiltCommand => {
                TopicRole::Subscribed
            }
        }
    }

    /// The discovery-payload key that carries this topic, if any.
    pub const fn payload_key(self) -> Option<&'static str> {
        match self {
            ShadeTopic::Position => Some("position_topic"),
            ShadeTopic::State => Some("state_topic"),
            ShadeTopic::Name => None,
            ShadeTopic::Command => Some("command_topic"),
            ShadeTopic::SetPosition => Some("set_position_topic"),
            ShadeTopic::TiltStatus => Some("tilt_status_topic"),
            ShadeTopic::TiltCommand => Some("tilt_command_topic"),
        }
    }

    /// True if this topic exists only on a shade with a tilt axis.
    pub const fn needs_tilt(self) -> bool {
        matches!(self, ShadeTopic::TiltStatus | ShadeTopic::TiltCommand)
    }

    /// The topics a shade has, given whether it can tilt.
    ///
    /// A shade without a tilt axis omits the tilt topics entirely rather than
    /// advertising a control that nothing publishes and nothing acts on.
    ///
    /// `has_tilt` is the caller's judgement, not a read of the shade's
    /// configured tilt mode. `somfy-domain` currently carries tilt modes
    /// without implementing them — no command drives a tilt axis yet — so a
    /// caller that derived this from the stored mode would advertise a control
    /// that moves nothing.
    pub fn for_shade(has_tilt: bool) -> impl Iterator<Item = ShadeTopic> {
        ShadeTopic::ALL
            .into_iter()
            .filter(move |topic| has_tilt || !topic.needs_tilt())
    }
}

const fn longest_relative() -> usize {
    let mut i = 0;
    let mut max = 0;
    while i < ShadeTopic::ALL.len() {
        let segments = ShadeTopic::ALL[i].segments();
        let mut j = 0;
        let mut len = 0;
        while j < segments.len() {
            // Each segment costs its own bytes plus the separator before it.
            len += segments[j].len() + 1;
            j += 1;
        }
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
}

/// Why a payload could not be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    /// The rendered payload would exceed [`PAYLOAD_CAPACITY`].
    ///
    /// Unreachable for inputs within this crate's own limits — the assertion
    /// below proves it at compile time — so in practice this means a name
    /// longer than [`MAX_NAME_LEN`]. Reported rather than truncated: a
    /// truncated payload is invalid JSON, and Home Assistant discards invalid
    /// JSON without saying so.
    TooLong,
}

/// A `cover` discovery config, as data.
///
/// Built by `MqttConfig::cover_discovery`, which is what fills the topics in
/// from the state root. The fields are public so a caller can inspect them, but
/// the topics are [`Topic`]s, so none of them can have been hand-written.
#[derive(Debug, Clone)]
pub struct CoverDiscovery<'a> {
    /// The payload's `~`: this shade's state base, absolute and with no leading
    /// slash. Every relative topic in the payload resolves against it.
    pub base: Topic,
    /// Absolute, and under the state root. Never under the discovery prefix:
    /// `{discovery_prefix}/status` is Home Assistant's own birth and will
    /// topic, so availability published there would be overwritten by HA's own
    /// birth message and report the device available while it is offline.
    pub availability: Topic,
    /// The discovery topic's last segment before `config`.
    pub object_id: ObjectId,
    /// The identity Home Assistant remembers this entity by.
    pub unique_id: UniqueId,
    /// The shade's name, verbatim. This is where the user's own spelling
    /// survives: `Salon / Porte-fenêtre` is unusable in a topic but perfectly
    /// good here, and it is what Home Assistant displays.
    pub name: &'a str,
    /// The stable device identifier, for the payload's `device` block.
    pub device_id: &'a str,
    /// Whether to carry the tilt topics.
    pub has_tilt: bool,
}

impl CoverDiscovery<'_> {
    /// Render the JSON Home Assistant reads.
    ///
    /// Every topic-bearing field comes from [`ShadeTopic`], so the payload
    /// cannot name a topic the firmware does not act on.
    ///
    /// Deliberately absent: the state and command vocabularies
    /// (`payload_open`, `state_opening` and friends). Home Assistant's defaults
    /// apply until the task that publishes those states chooses them, and
    /// choosing them here would fix a vocabulary nothing yet publishes.
    ///
    /// On failure `out` is left **empty**, never holding a partial payload. A
    /// half-written config is truncated JSON; Home Assistant discards JSON it
    /// cannot parse without logging anything an operator would find, so a
    /// caller that publishes the buffer anyway would produce an entity that
    /// never appears and no explanation of why.
    pub fn render(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        out.clear();
        match self.write_into(out) {
            Ok(()) => Ok(()),
            Err(e) => {
                out.clear();
                Err(e)
            }
        }
    }

    fn write_into(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        if self.name.len() > MAX_NAME_LEN {
            return Err(PayloadError::TooLong);
        }
        write(out, "{")?;

        write(out, "\"~\":")?;
        write_json_string(out, self.base.as_str())?;

        write(out, ",\"availability_topic\":")?;
        write_json_string(out, self.availability.as_str())?;

        write_object_id(out, self.device_id, &self.object_id)?;

        write(out, ",\"unique_id\":")?;
        write_json_string(out, self.unique_id.as_str())?;

        write(out, ",\"name\":")?;
        write_json_string(out, self.name)?;

        // Home Assistant's cover defaults are 100 open and 0 closed; this
        // project's positions run the other way, 0 fully open to 100 fully
        // closed. Stating both ends explicitly is what stops every shade
        // reporting itself inverted.
        write(out, ",\"position_open\":0,\"position_closed\":100")?;

        write_device_block(out, self.device_id)?;

        for topic in ShadeTopic::for_shade(self.has_tilt) {
            let Some(key) = topic.payload_key() else {
                continue;
            };
            write(out, ",\"")?;
            write(out, key)?;
            write(out, "\":\"~")?;
            for segment in topic.segments() {
                write(out, "/")?;
                write(out, segment)?;
            }
            write(out, "\"")?;
        }

        write(out, "}")
    }
}

/// A device-level diagnostic's discovery config, as data.
///
/// Built by `MqttConfig::diagnostic_discovery`, which is what fills the topics
/// in from the state root. The shape is deliberately the same as
/// [`CoverDiscovery`]'s — one `~`, one absolute availability topic, and every
/// other topic relative — because the two are read by the same round-trip check
/// and a second shape would need a second one.
#[derive(Debug, Clone)]
pub struct DiagnosticDiscovery<'a> {
    /// The payload's `~`: the device's own state base, absolute and with no
    /// leading slash.
    pub base: Topic,
    /// Absolute, and under the state root. See [`CoverDiscovery::availability`]
    /// for why it can never be under the discovery prefix.
    pub availability: Topic,
    /// The discovery topic's last segment before `config`.
    pub object_id: ObjectId,
    /// The identity Home Assistant remembers this entity by.
    pub unique_id: UniqueId,
    /// The stable device identifier, for the payload's `device` block.
    pub device_id: &'a str,
    /// Which fact this entity reports.
    pub entity: DeviceEntity,
}

impl DiagnosticDiscovery<'_> {
    /// Render the JSON Home Assistant reads.
    ///
    /// `entity_category: diagnostic` is not decoration: without it every one of
    /// these lands on the device's primary dashboard card beside the covers,
    /// which is clutter R7 does not ask for and an operator would have to undo
    /// by hand for each entity.
    ///
    /// Absent attributes are **omitted**, never written as `null`. Home
    /// Assistant treats an explicit `null` as a set value in several places, so
    /// a `"unit_of_measurement": null` on a sensor that has no unit is a
    /// difference worth not discovering later.
    ///
    /// On failure `out` is left empty, for the same reason
    /// [`CoverDiscovery::render`] leaves it empty.
    pub fn render(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        out.clear();
        match self.write_into(out) {
            Ok(()) => Ok(()),
            Err(e) => {
                out.clear();
                Err(e)
            }
        }
    }

    fn write_into(&self, out: &mut String<PAYLOAD_CAPACITY>) -> Result<(), PayloadError> {
        write(out, "{")?;

        write(out, "\"~\":")?;
        write_json_string(out, self.base.as_str())?;

        write(out, ",\"availability_topic\":")?;
        write_json_string(out, self.availability.as_str())?;

        write_object_id(out, self.device_id, &self.object_id)?;

        write(out, ",\"unique_id\":")?;
        write_json_string(out, self.unique_id.as_str())?;

        write(out, ",\"name\":")?;
        write_json_string(out, self.entity.label())?;

        write(out, ",\"entity_category\":\"diagnostic\"")?;

        for (key, value) in [
            ("device_class", self.entity.device_class()),
            ("unit_of_measurement", self.entity.unit()),
            ("state_class", self.entity.state_class()),
        ] {
            let Some(value) = value else {
                continue;
            };
            write(out, ",\"")?;
            write(out, key)?;
            write(out, "\":")?;
            write_json_string(out, value)?;
        }

        write_device_block(out, self.device_id)?;

        // Relative to `~`, exactly as a cover's topics are, so the round-trip
        // check resolves both the same way.
        write(out, ",\"state_topic\":\"~/")?;
        write(out, self.entity.slug())?;
        write(out, "\"")?;

        write(out, "}")
    }
}

/// The payload's `object_id`, which is **not** the same thing as the discovery
/// topic's segment of that name.
///
/// The topic segment is an address and nothing reads it: Home Assistant accepts
/// it and ignores it. The payload key is used *"instead of `name` for automatic
/// generation of `entity_id`"* — so it is what decides whether this device's
/// uptime sensor is called `sensor.uptime` or something a second controller
/// cannot collide with.
///
/// **It is therefore device-scoped, and the bare [`ObjectId`] is not enough.**
/// A slug alone claims `sensor.uptime` on the whole installation, so the second
/// somfy-rs board on an estate gets `sensor.uptime_2` — which is exactly the
/// "two sensors called Uptime and no way to tell them apart" case
/// [`write_device_block`] exists to prevent, reappearing one layer down where
/// the device block does not reach and where automations and dashboard cards
/// actually point. Prefixing with the device id makes the entity id as stable
/// and as unique as the `unique_id` beside it.
///
/// Both halves are `[a-zA-Z0-9_-]` by construction — a validated `DeviceId` and
/// an [`ObjectId`] built from literals — so the joined value is a legal
/// entity-id suffix without any sanitising.
fn write_object_id(
    out: &mut String<PAYLOAD_CAPACITY>,
    device_id: &str,
    object_id: &ObjectId,
) -> Result<(), PayloadError> {
    write(out, ",\"object_id\":\"")?;
    write_json_escaped(out, device_id)?;
    write(out, "_")?;
    write_json_escaped(out, object_id.as_str())?;
    write(out, "\"")
}

/// The `device` block both payloads carry, so that every entity this controller
/// publishes groups under one device in Home Assistant.
///
/// Without it the diagnostics appear as loose entities with no device to belong
/// to, which on an estate with two controllers is two sensors called "Uptime"
/// and no way to tell them apart. Every field is a fact already held: the
/// identifier is the same stable `device_id` every `unique_id` is built from,
/// so it survives a reboot, either namespace changing, and a firmware update.
///
/// Deliberately not here: `model` and `sw_version`. Both are knowable — the
/// chip is a compile-time constant and the version is `CARGO_PKG_VERSION` — and
/// neither has a consumer until there is a firmware-update entity to compare
/// against, which needs the OTA path Plan 6 brings.
fn write_device_block(
    out: &mut String<PAYLOAD_CAPACITY>,
    device_id: &str,
) -> Result<(), PayloadError> {
    write(out, ",\"device\":{\"identifiers\":[")?;
    write_json_string(out, device_id)?;
    write(out, "],\"name\":\"")?;
    // Both halves go through the escaper, including the literal. Neither can
    // produce an escape today — `DEVICE_NAME_PREFIX` is a constant and the
    // device id is `[a-zA-Z0-9_-]` by validation — but "this input happens to
    // be safe" is the reasoning that produces an unparseable payload the first
    // time an input changes, and a JSON string built from two pieces is exactly
    // where that reasoning gets applied to only one of them.
    write_json_escaped(out, DEVICE_NAME_PREFIX)?;
    write_json_escaped(out, device_id)?;
    write(out, "\",\"manufacturer\":")?;
    write_json_string(out, MANUFACTURER)?;
    write(out, "}")
}

fn write(out: &mut String<PAYLOAD_CAPACITY>, text: &str) -> Result<(), PayloadError> {
    out.push_str(text).map_err(|_| PayloadError::TooLong)
}

fn push(out: &mut String<PAYLOAD_CAPACITY>, ch: char) -> Result<(), PayloadError> {
    out.push(ch).map_err(|_| PayloadError::TooLong)
}

/// Write a JSON string literal, escaping what JSON requires.
///
/// A shade name is arbitrary user text. An unescaped quote or backslash makes
/// the whole payload unparseable, and Home Assistant discards a payload it
/// cannot parse without logging anything an operator would find — the entity
/// simply never appears.
fn write_json_string(out: &mut String<PAYLOAD_CAPACITY>, value: &str) -> Result<(), PayloadError> {
    push(out, '"')?;
    write_json_escaped(out, value)?;
    push(out, '"')
}

/// The escaping half of [`write_json_string`], without the surrounding quotes,
/// for the one place a JSON string is built from two pieces.
fn write_json_escaped(out: &mut String<PAYLOAD_CAPACITY>, value: &str) -> Result<(), PayloadError> {
    for ch in value.chars() {
        match ch {
            '"' => write(out, "\\\"")?,
            '\\' => write(out, "\\\\")?,
            '\n' => write(out, "\\n")?,
            '\r' => write(out, "\\r")?,
            '\t' => write(out, "\\t")?,
            '\u{08}' => write(out, "\\b")?,
            '\u{0c}' => write(out, "\\f")?,
            // The remaining control characters have no short escape and must
            // not appear raw.
            c if (c as u32) < 0x20 => {
                let code = c as u32;
                write(out, "\\u00")?;
                push(out, hex_digit((code >> 4) as u8))?;
                push(out, hex_digit((code & 0xF) as u8))?;
            }
            c => push(out, c)?,
        }
    }
    Ok(())
}

const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + nibble - 10) as char,
    }
}

/// `,"device":{"identifiers":["<id>"],"name":"<prefix><id>","manufacturer":"<m>"}`
/// at its widest.
///
/// The device id appears twice and is counted at one byte per byte both times,
/// unlike a shade name. That is not an optimism: a shade name is arbitrary user
/// text, while `DeviceId::new` refuses anything outside `[a-zA-Z0-9_-]` —
/// `validate::check_token` is the gate — and no character in that set has a
/// JSON escape. It still goes through the escaper, because "this input happens
/// to be safe" is the reasoning that produces an unparseable payload the first
/// time the input changes; the escaper simply cannot expand it.
const WORST_DEVICE_BLOCK_LEN: usize =
    // ,"device":{"identifiers":[
    26
    // "<id>"
    + 2 + crate::ident::MAX_DEVICE_ID_LEN
    // ],"name":"
    + 10
    // <prefix><id>
    + DEVICE_NAME_PREFIX.len() + crate::ident::MAX_DEVICE_ID_LEN
    // ","manufacturer":
    + 17
    // "<manufacturer>"
    + 2 + MANUFACTURER.len()
    // }
    + 1;

/// The part of a payload every discovery config carries, at its widest.
const WORST_COMMON_LEN: usize = 1
    // "~":"<base>",  — the shade base is the longer of the two bases.
    + 6 + config::WORST_SHADE_BASE_LEN + 2
    // "availability_topic":"<topic>",
    + 22 + config::WORST_AVAILABILITY_LEN + 2
    // "object_id":"<device_id>_<id>",  — see `write_object_id` for why the
    // payload key carries the device id and the topic segment does not.
    + 13 + crate::ident::MAX_DEVICE_ID_LEN + 1 + crate::ident::MAX_OBJECT_ID_LEN + 2
    // "unique_id":"<id>",
    + 13 + crate::ident::MAX_UNIQUE_ID_LEN + 2
    + WORST_DEVICE_BLOCK_LEN
    + 1;

/// The `cover` payload budget, proven.
///
/// Every term is an upper bound taken from this crate's own limits. The
/// constant is deliberately loose — it costs nothing to over-reserve and the
/// point is that the arithmetic is checkable, not that it is tight.
const WORST_COVER_PAYLOAD_LEN: usize = WORST_COMMON_LEN
    // "name":"<name>", with every byte escaped to six.
    + 8 + MAX_NAME_LEN * 6 + 2
    // "position_open":0,"position_closed":100
    + 40
    // ,"<key>":"~<relative>" for every topic, keys bounded by the longest.
    + ShadeTopic::ALL.len() * (6 + 18 + 1 + ShadeTopic::MAX_RELATIVE_LEN);

/// The diagnostic payload budget, proven the same way.
///
/// The three optional attributes are counted as present at their widest even
/// though no entity carries all three at full length, because the point of the
/// bound is that it cannot be exceeded rather than that it is reached.
const WORST_DIAGNOSTIC_PAYLOAD_LEN: usize = WORST_COMMON_LEN
    // "name":"<label>", — a firmware literal, so escaping cannot expand it.
    + 8 + DeviceEntity::MAX_LABEL_LEN + 2
    // "entity_category":"diagnostic",
    + 32
    // "device_class":"…", "unit_of_measurement":"…", "state_class":"…"
    + 3 * (24 + 32)
    // "state_topic":"~/<slug>"
    + 16 + 2 + DeviceEntity::MAX_SLUG_LEN + 1;

const _: () = assert!(
    PAYLOAD_CAPACITY >= WORST_COVER_PAYLOAD_LEN,
    "PAYLOAD_CAPACITY is too small for the longest cover payload this crate can build",
);
const _: () = assert!(
    PAYLOAD_CAPACITY >= WORST_DIAGNOSTIC_PAYLOAD_LEN,
    "PAYLOAD_CAPACITY is too small for the longest diagnostic payload this crate can build",
);
