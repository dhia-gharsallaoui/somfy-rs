//! The identifiers that become topic segments.
//!
//! Two disciplines apply, and the difference is deliberate:
//!
//! - **Configuration is validated.** [`NodeId`] and [`DeviceId`] come from an
//!   operator who is choosing an MQTT identifier, so a value outside
//!   `[a-zA-Z0-9_-]` is a mistake to report, not one to paper over.
//! - **Derived identifiers are built, not transformed.** [`ObjectId`] and
//!   [`UniqueId`] are assembled from a literal and a [`ShadeId`], so they
//!   satisfy the character class by construction. Nothing a user types reaches
//!   a topic segment at all.
//!
//! ## Why a shade's name is not in its object id
//!
//! An earlier shape sanitised the name into the object id, on the reading that
//! R2's "`node_id` and `object_id` MUST be sanitised" implies the name flows in.
//! It does satisfy that requirement, but it buys a legible topic at the price of
//! a lifecycle bug: renaming a shade moves its discovery topic, so the retained
//! config at the old address has to be cleared or it becomes an orphan. That is
//! the mess the requirements complain about — an estate accumulating retained
//! entities that can only be deleted by hand.
//!
//! The requirements themselves note that `object_id` "does not influence the
//! entity_id", so a stable, id-derived value costs nothing a user can see. The
//! human-readable name still reaches Home Assistant, through the discovery
//! payload's `name` field, which is where the display name actually comes from.
//! Building the id from the shade's stable slot index instead makes a rename a
//! payload change and nothing more, and it satisfies R2's character class more
//! strongly than sanitising would: there is no user text to sanitise.

use crate::entity::Component;
use crate::error::{ConfigError, Field};
use crate::validate::check_token;
use core::fmt::Write as _;
use heapless::String;
use somfy_domain::ShadeId;

/// Bytes a `node_id` may occupy.
pub const MAX_NODE_ID_LEN: usize = 32;

/// Bytes a `device_id` may occupy.
pub const MAX_DEVICE_ID_LEN: usize = 32;

/// Digits in the widest [`ShadeId`], which is a `u8`.
pub const MAX_SHADE_ID_DIGITS: usize = 3;

/// What every shade's object id starts with.
///
/// A literal rather than a bare number so a topic read off a broker says what
/// it addresses. [`Component`] already separates a shade's cover from its
/// sensors, since it is a different segment of the discovery topic.
const OBJECT_ID_PREFIX: &str = "shade_";

/// Bytes an object id may occupy: the prefix and up to three digits of
/// [`ShadeId`].
///
/// Exact rather than padded, and safe to be exact because both terms are fixed
/// at compile time — change the prefix and this constant moves with it. That is
/// the difference from [`MAX_UNIQUE_ID_LEN`], whose bound depends on a
/// hand-maintained array and therefore carries headroom.
pub const MAX_OBJECT_ID_LEN: usize = OBJECT_ID_PREFIX.len() + MAX_SHADE_ID_DIGITS;

/// Bytes a unique id may occupy.
///
/// A device id, a component name, up to three digits of [`ShadeId`], and two
/// separators — with headroom, deliberately, rather than sized exactly to the
/// components that exist today. [`Component::MAX_LEN`] is a fold over
/// [`Component::ALL`], and nothing forces `ALL` to list every variant: the
/// exhaustive `match` in [`Component::as_str`] forces a new variant to be named
/// there, but a variant left out of `ALL` leaves `MAX_LEN` stale. Sizing this
/// exactly would turn that omission into a truncated `unique_id` — and a
/// truncated `unique_id` is not a visible fault, it is two entities silently
/// sharing an identity.
///
/// The headroom absorbs any Home Assistant component name up to
/// [`MAX_COMPONENT_HEADROOM`] bytes, which covers every name in HA's set; the
/// assertion below pins that, and [`push_u8`] panics rather than truncating if
/// it is ever exceeded anyway.
pub const MAX_UNIQUE_ID_LEN: usize = 64;

/// Bytes of component name [`MAX_UNIQUE_ID_LEN`] leaves room for.
///
/// The longest name in Home Assistant's own component set is
/// `alarm_control_panel` at 19 bytes; this is comfortably beyond it.
pub const MAX_COMPONENT_HEADROOM: usize = MAX_UNIQUE_ID_LEN - MAX_DEVICE_ID_LEN - 1 - 1 - 3;

const _: () = assert!(
    Component::MAX_LEN <= MAX_COMPONENT_HEADROOM,
    "a component name outgrew the unique-id budget",
);

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
/// Built from the shade's stable slot index, never from its name, so the
/// discovery topic does not move when a shade is renamed. See the module docs
/// for why that trade is worth making: an object id that follows the name turns
/// every rename into a retained config that has to be cleared or become an
/// orphan, and buys nothing a user can see, because `object_id` does not
/// influence the entity id Home Assistant creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectId(String<MAX_OBJECT_ID_LEN>);

impl ObjectId {
    /// Derive an object id from a shade's id.
    ///
    /// Infallible, unique per shade, never empty, and always a single segment
    /// in `[a-zA-Z0-9_-]` — all four by construction rather than by check,
    /// because the only inputs are a literal and an integer.
    ///
    /// ```
    /// use somfy_domain::ShadeId;
    /// use somfy_mqtt::ObjectId;
    ///
    /// assert_eq!(ObjectId::for_shade(ShadeId(1)).as_str(), "shade_1");
    /// assert_eq!(ObjectId::for_shade(ShadeId(255)).as_str(), "shade_255");
    /// ```
    pub fn for_shade(id: ShadeId) -> ObjectId {
        let mut out: String<MAX_OBJECT_ID_LEN> = String::new();
        push_str(&mut out, OBJECT_ID_PREFIX);
        push_u8(&mut out, id.0);
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
        push_str(&mut out, device.as_str());
        push_str(&mut out, "_");
        push_str(&mut out, component.as_str());
        push_str(&mut out, "_");
        push_u8(&mut out, id.0);
        UniqueId(out)
    }

    /// The identifier, for the discovery payload.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Store an already-validated token. The length was checked by the validator,
/// so the push cannot fail.
fn store<const N: usize>(value: &str) -> String<N> {
    let mut out = String::new();
    let _ = out.push_str(value);
    out
}

/// Append text, or panic. See [`push_u8`] for why silence is not an option.
fn push_str<const N: usize>(out: &mut String<N>, text: &str) {
    out.push_str(text)
        .expect("identifier capacity proven at compile time");
}

/// Append a `u8` as decimal, or panic.
///
/// The capacity assertions below prove every caller has room, so this cannot
/// fail. It panics rather than dropping the digits because the alternative is
/// silent: an identifier missing its shade number still looks like an
/// identifier, and two shades sharing one is a configuration Home Assistant
/// rejects outright.
fn push_u8<const N: usize>(out: &mut String<N>, value: u8) {
    write!(out, "{value}").expect("identifier capacity proven at compile time");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shade id yields a distinct, single, non-empty segment. Distinctness
    /// is the property that matters: two shades sharing an object id share a
    /// discovery topic, and the second silently overwrites the first.
    #[test]
    fn object_ids_are_distinct_valid_segments_for_every_shade_id() {
        let mut seen: Vec<heapless::String<MAX_OBJECT_ID_LEN>> = Vec::new();
        for id in 0u8..=255 {
            let object = ObjectId::for_shade(ShadeId(id));
            let text = object.as_str();
            assert!(!text.is_empty());
            assert!(
                text.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{text} escaped the character class",
            );
            assert_eq!(text, format!("shade_{id}"));
            assert!(text.len() <= MAX_OBJECT_ID_LEN);
            assert!(!seen.iter().any(|s| s.as_str() == text), "{text} repeated");
            seen.push(object.0.clone());
        }
        assert_eq!(seen.len(), 256);
    }
}
