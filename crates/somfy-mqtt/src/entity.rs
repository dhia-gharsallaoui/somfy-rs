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
pub const PAYLOAD_CAPACITY: usize = 1024;

/// Longest shade name the payload budget assumes.
///
/// Matches the capacity of `somfy_domain::ShadeConfig::name`, which is where
/// the name comes from. A caller passing something longer gets
/// [`PayloadError::TooLong`] rather than a truncated payload.
pub const MAX_NAME_LEN: usize = 32;

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
    /// (`payload_open`, `state_opening` and friends) and the device block. Home
    /// Assistant's defaults apply until the task that publishes those states
    /// chooses them, and choosing them here would fix a vocabulary nothing yet
    /// publishes.
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

        write(out, ",\"object_id\":")?;
        write_json_string(out, self.object_id.as_str())?;

        write(out, ",\"unique_id\":")?;
        write_json_string(out, self.unique_id.as_str())?;

        write(out, ",\"name\":")?;
        write_json_string(out, self.name)?;

        // Home Assistant's cover defaults are 100 open and 0 closed; this
        // project's positions run the other way, 0 fully open to 100 fully
        // closed. Stating both ends explicitly is what stops every shade
        // reporting itself inverted.
        write(out, ",\"position_open\":0,\"position_closed\":100")?;

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
    push(out, '"')
}

const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + nibble - 10) as char,
    }
}

/// The payload budget, proven.
///
/// Every term is an upper bound taken from this crate's own limits. The
/// constant is deliberately loose — it costs nothing to over-reserve and the
/// point is that the arithmetic is checkable, not that it is tight.
const WORST_PAYLOAD_LEN: usize = 1
    // "~":"<base>",
    + 6 + config::WORST_SHADE_BASE_LEN + 2
    // "availability_topic":"<topic>",
    + 22 + config::WORST_AVAILABILITY_LEN + 2
    // "object_id":"<id>",
    + 13 + crate::ident::MAX_OBJECT_ID_LEN + 2
    // "unique_id":"<id>",
    + 13 + crate::ident::MAX_UNIQUE_ID_LEN + 2
    // "name":"<name>", with every byte escaped to six.
    + 8 + MAX_NAME_LEN * 6 + 2
    // "position_open":0,"position_closed":100
    + 40
    // ,"<key>":"~<relative>" for every topic, keys bounded by the longest.
    + ShadeTopic::ALL.len() * (6 + 18 + 1 + ShadeTopic::MAX_RELATIVE_LEN)
    + 1;

const _: () = assert!(
    PAYLOAD_CAPACITY >= WORST_PAYLOAD_LEN,
    "PAYLOAD_CAPACITY is too small for the longest payload this crate can build",
);
