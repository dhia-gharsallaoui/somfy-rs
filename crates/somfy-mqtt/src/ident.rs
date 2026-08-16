//! The identifiers that become topic segments, and the sanitiser that stands
//! between a human-chosen name and a topic.
//!
//! Two different disciplines apply here, and the difference is deliberate:
//!
//! - **Configuration is validated.** [`NodeId`] and [`DeviceId`] come from an
//!   operator who is choosing an MQTT identifier, so a value outside
//!   `[a-zA-Z0-9_-]` is a mistake to report, not one to paper over.
//! - **A shade name is sanitised.** A shade is called `Salon / Porte-fenêtre`
//!   because that is what the room is called. Refusing it would be refusing the
//!   user's own language, so it is transformed on the way into a topic — and
//!   the name itself survives untouched in the discovery payload's `name`
//!   field, which is where Home Assistant reads it from anyway.

use crate::entity::Component;
use crate::error::{ConfigError, Field};
use crate::validate::check_token;
use heapless::String;
use somfy_domain::ShadeId;

/// Bytes a `node_id` may occupy.
pub const MAX_NODE_ID_LEN: usize = 32;

/// Bytes a `device_id` may occupy.
pub const MAX_DEVICE_ID_LEN: usize = 32;

/// Bytes of sanitised shade name an object id may carry, before the id suffix.
///
/// Truncating here is safe in a way truncating configuration is not: the object
/// id is a label this crate derives, not an address the operator typed, and the
/// id suffix keeps it unique however short the name part gets.
pub const MAX_NAME_PART_LEN: usize = 48;

/// Bytes an object id may occupy: a name part, a separator, and up to three
/// digits of [`ShadeId`].
pub const MAX_OBJECT_ID_LEN: usize = MAX_NAME_PART_LEN + 1 + 3;

/// Bytes a unique id may occupy: a device id, a component name, up to three
/// digits of [`ShadeId`], and two separators.
pub const MAX_UNIQUE_ID_LEN: usize = MAX_DEVICE_ID_LEN + 1 + Component::MAX_LEN + 1 + 3;

/// Substituted for a name that sanitises to nothing — a name written entirely
/// in a non-Latin script, or one that is empty.
const EMPTY_NAME_FALLBACK: &str = "shade";

/// The optional device segment of a discovery topic.
///
/// Home Assistant accepts it and then ignores it: it does not influence the
/// entity id and nothing subscribes by it. It earns its place by making the
/// device's own configs findable with `mosquitto_sub -t 'homeassistant/+/somfyrs/#'`
/// on a broker shared with other integrations.
///
/// It goes **after** the component, never before. `homeassistant/somfyrs/cover/1/config`
/// is silently ignored by Home Assistant; `homeassistant/cover/somfyrs/1/config`
/// creates the entity. That ordering is the difference between an integration
/// that works and one that does nothing at all, with no error on either side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeId(String<MAX_NODE_ID_LEN>);

impl NodeId {
    /// Validate and store a node id, or say which rule it broke.
    pub fn new(value: &str) -> Result<NodeId, ConfigError> {
        check_token(value, Field::NodeId, MAX_NODE_ID_LEN)?;
        Ok(NodeId(store(value)))
    }

    /// The single topic segment this becomes.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The stable identifier every `unique_id` is built from.
///
/// "Stable" is the requirement, and it is stronger than it looks: it must
/// survive a reboot, a configuration change, and a firmware update. An entity
/// whose `unique_id` changes is a *new* entity to Home Assistant — the old one
/// stays behind as an orphan, and every automation, dashboard card and history
/// graph that referred to it is now pointing at something that will never
/// update again.
///
/// The intended source is something the hardware carries, such as a MAC-derived
/// string. It is deliberately not derived from anything a user edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId(String<MAX_DEVICE_ID_LEN>);

impl DeviceId {
    /// Validate and store a device id, or say which rule it broke.
    pub fn new(value: &str) -> Result<DeviceId, ConfigError> {
        check_token(value, Field::DeviceId, MAX_DEVICE_ID_LEN)?;
        Ok(DeviceId(store(value)))
    }

    /// The identifier, for use inside a `unique_id`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The last segment before `config` in a discovery topic.
///
/// Derived from the shade's name so the topic is legible to whoever is watching
/// the broker, with the shade id appended so it is unique and non-empty
/// whatever the name is.
///
/// **Consequence worth knowing:** renaming a shade changes its object id, and
/// therefore its discovery topic. The retained config at the old topic must be
/// cleared with a zero-length retained publish, exactly as deleting a shade
/// does. The entity itself survives the rename, because `unique_id` is built
/// from the device and shade ids and not from the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectId(String<MAX_OBJECT_ID_LEN>);

impl ObjectId {
    /// Derive an object id from a shade's name and id.
    ///
    /// Infallible by construction: any name at all produces a valid single
    /// segment.
    ///
    /// ```
    /// use somfy_domain::ShadeId;
    /// use somfy_mqtt::ObjectId;
    ///
    /// assert_eq!(ObjectId::for_shade("Lounge", ShadeId(1)).as_str(), "lounge_1");
    /// assert_eq!(
    ///     ObjectId::for_shade("Salon / Porte-fenêtre", ShadeId(7)).as_str(),
    ///     "salon_porte-fen_tre_7",
    /// );
    /// // A name with nothing usable in it still yields a valid segment.
    /// assert_eq!(ObjectId::for_shade("日本語", ShadeId(2)).as_str(), "shade_2");
    /// ```
    pub fn for_shade(name: &str, id: ShadeId) -> ObjectId {
        let mut out: String<MAX_OBJECT_ID_LEN> = String::new();
        let part = sanitise(name);
        let part = if part.is_empty() {
            EMPTY_NAME_FALLBACK
        } else {
            &part
        };
        // Capacity is MAX_NAME_PART_LEN + 1 + 3 and `part` is at most
        // MAX_NAME_PART_LEN, so neither push can fail.
        let _ = out.push_str(part);
        let _ = out.push('_');
        let _ = write_u8(&mut out, id.0);
        ObjectId(out)
    }

    /// The single topic segment this becomes.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The identity Home Assistant remembers an entity by.
///
/// Built from the device id, the component, and the shade id — none of which a
/// user edits. It survives a rename, a change of `state_root`, a change of
/// `discovery_prefix`, and a firmware update, which is what R4 asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueId(String<MAX_UNIQUE_ID_LEN>);

impl UniqueId {
    /// Derive the unique id for one shade's entity of a given component.
    ///
    /// The component is part of it because one shade will own several entities
    /// once diagnostics and sensors are published alongside the cover, and two
    /// entities sharing a `unique_id` is a configuration Home Assistant rejects
    /// outright.
    pub fn for_shade(device: &DeviceId, component: Component, id: ShadeId) -> UniqueId {
        let mut out: String<MAX_UNIQUE_ID_LEN> = String::new();
        let _ = out.push_str(device.as_str());
        let _ = out.push('_');
        let _ = out.push_str(component.as_str());
        let _ = out.push('_');
        let _ = write_u8(&mut out, id.0);
        UniqueId(out)
    }

    /// The identifier, for the discovery payload.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reduce an arbitrary name to the topic character class.
///
/// Characters in `[a-zA-Z0-9_-]` are kept, lowercased. Every run of anything
/// else — spaces, slashes, accented letters, emoji, control characters —
/// collapses to a single `_`, and leading and trailing separators are dropped.
/// Lowercasing is normalisation, not sanitisation: MQTT topics are
/// case-sensitive, so `Lounge` and `lounge` would otherwise be two different
/// addresses for the same shade depending on how the name was typed.
///
/// The result may be empty; callers must substitute something.
fn sanitise(name: &str) -> String<MAX_NAME_PART_LEN> {
    let mut out: String<MAX_NAME_PART_LEN> = String::new();
    let mut pending_separator = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            if pending_separator && !out.is_empty() && out.push('_').is_err() {
                break;
            }
            pending_separator = false;
            if out.push(ch.to_ascii_lowercase()).is_err() {
                break;
            }
        } else {
            pending_separator = true;
        }
    }
    // Truncation can strand a separator at the end; a kept character never can,
    // because a separator is only ever written immediately before one.
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Store an already-validated token. The length was checked by the validator,
/// so the push cannot fail.
fn store<const N: usize>(value: &str) -> String<N> {
    let mut out = String::new();
    let _ = out.push_str(value);
    out
}

/// Write a `u8` as decimal without pulling in `core::fmt`'s machinery for what
/// is at most three digits.
fn write_u8<const N: usize>(out: &mut String<N>, value: u8) -> Result<(), ()> {
    if value >= 100 {
        out.push((b'0' + value / 100) as char)?;
    }
    if value >= 10 {
        out.push((b'0' + (value / 10) % 10) as char)?;
    }
    out.push((b'0' + value % 10) as char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_collapses_runs_and_trims_edges() {
        assert_eq!(sanitise("Lounge").as_str(), "lounge");
        assert_eq!(sanitise("  Lounge  ").as_str(), "lounge");
        assert_eq!(
            sanitise("Salon / Porte-fenêtre").as_str(),
            "salon_porte-fen_tre"
        );
        assert_eq!(sanitise("///").as_str(), "");
        assert_eq!(sanitise("").as_str(), "");
        assert_eq!(sanitise("日本語").as_str(), "");
        assert_eq!(sanitise("a\u{0}b").as_str(), "a_b");
    }

    #[test]
    fn sanitise_truncates_without_stranding_a_separator() {
        let long = "é".repeat(200);
        assert_eq!(sanitise(&long).as_str(), "");

        let alternating = "a-".repeat(200);
        let out = sanitise(&alternating);
        assert!(out.len() <= MAX_NAME_PART_LEN);
        assert!(!out.ends_with('_'));
    }

    #[test]
    fn write_u8_covers_every_width() {
        for value in [0u8, 7, 10, 99, 100, 255] {
            let mut out: String<8> = String::new();
            write_u8(&mut out, value).unwrap();
            assert_eq!(out.as_str(), value.to_string());
        }
    }
}
