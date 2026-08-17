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
//! 2. **No topic has an empty segment, and no user text reaches one.** Every
//!    segment is a firmware literal, an operator identifier validated to
//!    `[a-zA-Z0-9_-]`, or a value built from a literal and a shade id. A shade
//!    named `Salon / Porte-fenêtre` cannot produce a topic segment because its
//!    name is not an input to any topic — it reaches Home Assistant through the
//!    payload's `name` field, which is where the display name comes from.
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
//! let object = ObjectId::for_shade(shade);
//!
//! // Discovery lives under the prefix; state lives under the root; the two
//! // never meet except through the payload's `~`.
//! assert_eq!(
//!     config.discovery_topic(Component::Cover, &object).as_str(),
//!     "homeassistant/cover/somfyrs/shade_1/config",
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
//! ## The lifecycle is here too, and it is also data
//!
//! Which message goes to which topic with which retention is pure, so R5 and
//! R6 live in [`lifecycle`](self#reexports) rather than with the transport: a
//! [`Step`] is a value, and `tests/lifecycle.rs` asserts the rules without a
//! broker. Two of them are carried by the types rather than by a note:
//!
//! - [`Publish`] holds its [`Retention`] privately and has no constructor that
//!   takes one, so "was this meant to be retained?" is not a question anyone
//!   answers at a call site.
//! - A topic whose [`TopicRole`] is [`TopicRole::Subscribed`] cannot become a
//!   [`PublishedTopic`], and [`MqttConfig::state`] — the one retained per-shade
//!   publish — takes nothing else. A retained command replays on every
//!   reconnect, which is a shade that closes itself each time the broker
//!   restarts.
//!
//! Deleting a shade clears every retained topic it owns with a zero-length
//! retained publish; [`MqttConfig::retire_shade`] is that plan, and it is
//! derived from the same [`SHADE_COMPONENTS`] the announcement is, so an entity
//! cannot be announced without also being removable. Renaming a shade needs no
//! plan at all: neither [`ObjectId`] nor [`UniqueId`] follows the name, so a
//! rename is a payload change and the topic stays where it was.
//!
//! ## The entity set, and what decides its contents (R7)
//!
//! A cover per shade, and [`DeviceEntity::ALL`] for the controller itself —
//! uptime, Wi-Fi signal, free heap, peak heap use, and the number of damaged
//! slots in the rolling-code region. All five are marked
//! `entity_category: diagnostic` so they do not clutter the device's main card,
//! and all five carry the same `device` block the cover does, so Home Assistant
//! groups every entity under one controller.
//!
//! The set is not chosen to reach a number. **An entity backed by nothing is
//! worse than an absent one**, because it reads as a device fault rather than
//! as an unimplemented feature — which is the failure the requirements spec's
//! own acceptance criterion names. Every entity here is a value the firmware
//! already holds; what it does not hold is absent rather than stubbed, and
//! `docs/provenance.md` records each omission with the condition for adding it.
//!
//! ## What is deliberately not here
//!
//! - **Any network code.** A socket, a client, a reconnect schedule and a
//!   buffer belong with the transport; what is here is the decision it
//!   executes.
//! - **The state and command vocabularies.** [`CoverDiscovery::render`] emits
//!   the topics and lets Home Assistant's defaults stand, because fixing a
//!   vocabulary before anything publishes it would be guessing.
//! - **Components other than `cover` and `sensor`.** [`Component`] carries the
//!   full set the firmware could emit; a payload is built only for the two that
//!   have something to report.

#![cfg_attr(not(test), no_std)]

mod config;
mod entity;
mod error;
mod ident;
mod lifecycle;
mod topic;
mod validate;

pub use config::MqttConfig;
pub use entity::{
    Component, CoverDiscovery, DeviceEntity, DiagnosticDiscovery, PayloadError, ShadeTopic,
    TopicRole, MAX_NAME_LEN, PAYLOAD_CAPACITY,
};
pub use error::{ConfigError, Field};
pub use ident::{
    DeviceId, NodeId, ObjectId, UniqueId, LONGEST_HA_COMPONENT_NAME, MAX_COMPONENT_HEADROOM,
    MAX_DEVICE_ID_LEN, MAX_NODE_ID_LEN, MAX_OBJECT_ID_LEN, MAX_SHADE_ID_DIGITS, MAX_UNIQUE_ID_LEN,
};
pub use lifecycle::{
    reconfigure, Listen, Payload, Publish, PublishedTopic, Retention, Step, SubscribedTopic,
    OFFLINE, ONLINE, SHADE_COMPONENTS,
};
pub use topic::{
    namespaces_overlap, DiscoveryPrefix, StateRoot, Topic, MAX_DISCOVERY_PREFIX_LEN,
    MAX_STATE_ROOT_LEN, TOPIC_CAPACITY,
};
