//! # somfy-mqtt
//!
//! MQTT topic construction, configuration validation, and Home Assistant
//! discovery payloads. Pure data: no network, no hardware, no clock.
//!
//! ## What went wrong, and what this crate is for
//!
//! Deployed firmware publishes MQTT correctly — 49 retained
//! state topics, every one of them right — and its discovery *payload* is
//! correct too: every topic inside it resolves. Only the address the config is
//! **sent to** is wrong, and every possible configuration fails, in three
//! mutually exclusive ways:
//!
//! | state root | discovery prefix | published to | result in Home Assistant |
//! |---|---|---|---|
//! | `mydevice` | `homeassistant` | `mydevice/homeassistant/cover/1/config` | ignored — not under HA's prefix |
//! | `homeassistant` | *(empty)* | `homeassistant//cover/1/config` | ignored — empty segment |
//! | *(empty)* | `homeassistant` | `homeassistant/cover/1/config` | discovered, but the payload says `"~": "/shades/1"` while the device publishes to `shades/1` — entities permanently `unavailable` |
//!
//! Three causes: the state root was prepended to the discovery topic; an empty
//! root produced leading-slash topics in the payload that disagreed with where
//! the publisher wrote; and empty segments were not collapsed. Every bad
//! combination was accepted, and every one looked like it had worked.
//!
//! ## The four things this crate guarantees
//!
//! 1. **The two namespaces cannot be joined.** [`DiscoveryPrefix`] and
//!    [`StateRoot`] hold their text privately and expose no way to read it, and
//!    the builder that turns a root into a topic can be seeded exactly once.
//!    Concatenating them is not a mistake to avoid; it is a program that cannot
//!    be written. See [`DiscoveryPrefix`] for the compile-fail proofs.
//! 2. **No topic has an empty segment.** Every segment comes from a firmware
//!    literal, a validated identifier, or a sanitised name, and none of those
//!    can be empty.
//! 3. **Bad configuration is refused, never repaired.** [`ConfigError`] names
//!    the [`Field`] that was wrong. There is no variant meaning "accepted with
//!    adjustments", because a silently adjusted address is indistinguishable
//!    from a broken device. That includes the fault that belongs to a *pair* of
//!    values rather than to either one: two individually valid roots that name
//!    the same namespace put availability on Home Assistant's own birth topic,
//!    so [`MqttConfig::new`] refuses them — see [`ConfigError::Overlap`].
//! 4. **The payload and the publisher cannot drift apart.** Both are derived
//!    from [`ShadeTopic`], which owns each topic's segments, its direction, and
//!    the payload key that carries it.
//!
//! ## Home Assistant's discovery contract, as verified
//!
//! ```text
//! <discovery_prefix>/<component>/[<node_id>/]<object_id>/config
//! ```
//!
//! Confirmed by publishing both shapes to a live broker and watching which one
//! Home Assistant acted on:
//!
//! ```text
//! homeassistant/mydevice/cover/1/config   -> ignored
//! homeassistant/cover/mydevice/1/config   -> entity created, live position
//! ```
//!
//! The component **must** be the segment immediately after the prefix. The node
//! id is optional and unused by Home Assistant. Home Assistant supports exactly
//! one discovery prefix, and it is global to the installation — a device that
//! forces it to be moved taxes every other MQTT device on that network for as
//! long as it is installed.
//!
//! ## Example
//!
//! ```
//! use somfy_domain::ShadeId;
//! use somfy_mqtt::{
//!     Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, ShadeTopic, StateRoot,
//! };
//!
//! let config = MqttConfig::new(
//!     DiscoveryPrefix::new("homeassistant")?,
//!     StateRoot::new("somfyrs")?,
//!     NodeId::new("somfyrs")?,
//!     DeviceId::new("a1b2c3d4")?,
//! )?;
//! let shade = ShadeId(1);
//! let object = ObjectId::for_shade("Salon / Porte-fenêtre", shade);
//!
//! // Discovery lives under the prefix; state lives under the root; the two
//! // never meet except through the payload's `~`.
//! assert_eq!(
//!     config.discovery_topic(Component::Cover, &object).as_str(),
//!     "homeassistant/cover/somfyrs/salon_porte-fen_tre_1/config",
//! );
//! assert_eq!(config.shade_base(shade).as_str(), "somfyrs/shades/1");
//! assert_eq!(
//!     config.shade_topic(shade, ShadeTopic::Position).as_str(),
//!     "somfyrs/shades/1/position",
//! );
//! assert_eq!(config.availability_topic().as_str(), "somfyrs/status");
//! # Ok::<(), somfy_mqtt::ConfigError>(())
//! ```
//!
//! ## What is deliberately not here
//!
//! - **Any network code.** A broker connection, retention flags and the
//!   lifecycle rules that go with them belong with the transport.
//! - **The state and command vocabularies.** [`CoverDiscovery::render`] emits
//!   the topics and lets Home Assistant's defaults stand, because fixing a
//!   vocabulary before anything publishes it would be guessing.
//! - **Components other than `cover`.** [`Component`] carries the full set the
//!   firmware will emit, but only the cover payload is built so far.
//!
//! Two obligations follow for whoever adds the transport, and neither is
//! visible from the types:
//!
//! - Topics whose [`TopicRole`] is [`TopicRole::Subscribed`] must never be
//!   published retained. A retained command replays on every reconnect, which
//!   is a shade that closes itself each time the broker restarts.
//! - Renaming a shade changes its [`ObjectId`] and therefore its discovery
//!   topic. The retained config at the old topic must be cleared with a
//!   zero-length retained publish, exactly as deleting a shade must. The entity
//!   itself survives, because [`UniqueId`] is not derived from the name.

#![cfg_attr(not(test), no_std)]

mod config;
mod entity;
mod error;
mod ident;
mod topic;
mod validate;

pub use config::MqttConfig;
pub use entity::{
    Component, CoverDiscovery, PayloadError, ShadeTopic, TopicRole, MAX_NAME_LEN, PAYLOAD_CAPACITY,
};
pub use error::{ConfigError, Field};
pub use ident::{
    DeviceId, NodeId, ObjectId, UniqueId, MAX_DEVICE_ID_LEN, MAX_NAME_PART_LEN, MAX_NODE_ID_LEN,
    MAX_OBJECT_ID_LEN, MAX_UNIQUE_ID_LEN,
};
pub use topic::{
    DiscoveryPrefix, StateRoot, Topic, MAX_DISCOVERY_PREFIX_LEN, MAX_STATE_ROOT_LEN, TOPIC_CAPACITY,
};
