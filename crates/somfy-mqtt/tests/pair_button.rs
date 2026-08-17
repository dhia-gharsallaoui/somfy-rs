//! The per-shade pairing button: a stateless action that puts one `Prog` frame
//! on the air.
//!
//! It is a `button` rather than anything with state because it *is* stateless —
//! there is nothing to report back. RTS is one-way, so the controller never
//! learns whether the motor accepted the pairing; the person standing at the
//! shade sees it jog and that is the whole of the feedback loop.
//!
//! What these check is that adding it did not create the two failures this
//! crate exists to prevent: an entity whose payload names a topic the firmware
//! does not act on, and an entity that can be announced but not removed.

use std::collections::BTreeMap;

use serde_json::Value;
use somfy_domain::ShadeId;
use somfy_mqtt::{
    Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, Payload, PublishedTopic,
    Retention, ShadeTopic, StateRoot, Step, SubscribedTopic, TopicRole, PAYLOAD_CAPACITY,
    SHADE_COMPONENTS,
};

fn config() -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new("homeassistant").unwrap(),
        StateRoot::new("somfyrs").unwrap(),
        NodeId::new("somfyrs").unwrap(),
        DeviceId::new("a1b2c3d4").unwrap(),
    )
    .unwrap()
}

fn render_button(cfg: &MqttConfig, shade: ShadeId, name: &str) -> Value {
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.button_discovery(shade, name)
        .render(&mut buf)
        .expect("payload fits");
    serde_json::from_str(&buf).expect("rendered payload is valid JSON")
}

// ---------------------------------------------------------------------------
// The topic
// ---------------------------------------------------------------------------

/// The pairing topic is one the firmware **subscribes** to, so it can never be
/// published retained. A retained `PRESS` would replay on every reconnect —
/// which is a controller that transmits `Prog` at a motor every time the broker
/// restarts.
#[test]
fn the_pair_topic_is_subscribed_and_can_never_be_published_retained() {
    assert_eq!(ShadeTopic::Pair.role(), TopicRole::Subscribed);
    assert!(PublishedTopic::of(ShadeTopic::Pair).is_none());
    assert!(SubscribedTopic::of(ShadeTopic::Pair).is_some());
}

/// The pairing topic exists on every shade, tilt or not: pairing is not a tilt
/// feature.
#[test]
fn every_shade_has_a_pair_topic() {
    for has_tilt in [false, true] {
        assert!(
            ShadeTopic::for_shade(has_tilt).any(|t| t == ShadeTopic::Pair),
            "has_tilt={has_tilt}",
        );
    }
}

/// No two topics may claim the same discovery-payload key. The cover renderer
/// walks every keyed topic in order, so a second topic carrying
/// `command_topic` would emit that key twice in one JSON object — which parses,
/// silently keeps one of them, and leaves the other control pointing nowhere.
/// [`ShadeTopic::Pair`] therefore carries no key of its own; the button
/// renderer names it directly.
#[test]
fn no_two_shade_topics_claim_one_payload_key() {
    let mut seen: Vec<&str> = Vec::new();
    for topic in ShadeTopic::ALL {
        let Some(key) = topic.payload_key() else {
            continue;
        };
        assert!(!seen.contains(&key), "{key} is claimed twice");
        seen.push(key);
    }
    assert_eq!(ShadeTopic::Pair.payload_key(), None);
}

// ---------------------------------------------------------------------------
// The payload
// ---------------------------------------------------------------------------

/// The round trip that matters: the topic the payload names, after `~`
/// expansion, is the topic the firmware subscribes to. This is the check whose
/// absence left every entity in the field permanently unavailable.
#[test]
fn the_button_payload_names_the_topic_the_firmware_subscribes_to() {
    let cfg = config();
    let shade = ShadeId(3);
    let payload = render_button(&cfg, shade, "Lounge");
    let object = payload.as_object().unwrap();

    let base = object.get("~").and_then(Value::as_str).unwrap();
    let command = object.get("command_topic").and_then(Value::as_str).unwrap();
    let absolute = command
        .strip_prefix('~')
        .map(|rest| format!("{base}{rest}"))
        .unwrap_or_else(|| command.to_owned());

    let firmware: BTreeMap<String, TopicRole> = cfg
        .shade_topics(shade, true)
        .map(|(topic, absolute)| (absolute.as_str().to_owned(), topic.role()))
        .collect();

    assert_eq!(
        firmware.get(&absolute),
        Some(&TopicRole::Subscribed),
        "{absolute:?} is not a topic the firmware subscribes to",
    );
    assert_eq!(absolute, cfg.shade_topic(shade, ShadeTopic::Pair).as_str());
}

/// The button's identity is its own. Two entities sharing a `unique_id` is a
/// configuration Home Assistant rejects outright, and the shade already owns a
/// cover.
#[test]
fn the_button_does_not_share_the_covers_identity() {
    let cfg = config();
    let shade = ShadeId(3);
    let button = render_button(&cfg, shade, "Lounge");

    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(shade, "Lounge", false)
        .render(&mut buf)
        .unwrap();
    let cover: Value = serde_json::from_str(&buf).unwrap();

    assert_ne!(button["unique_id"], cover["unique_id"]);
    assert_ne!(button["name"], cover["name"]);
    // The discovery topics differ by component, so the object id may — and
    // does — stay the shade's.
    assert_ne!(
        cfg.discovery_topic(Component::Button, &ObjectId::for_shade(shade))
            .as_str(),
        cfg.discovery_topic(Component::Cover, &ObjectId::for_shade(shade))
            .as_str(),
    );
}

/// The name says which shade it pairs. An estate with thirty-two shades and
/// thirty-two buttons all called "Pair" is one where the wrong motor gets
/// programmed.
#[test]
fn the_button_name_carries_the_shades_name() {
    let payload = render_button(&config(), ShadeId(3), "Lounge");
    let name = payload["name"].as_str().unwrap();
    assert!(name.starts_with("Lounge"), "{name:?}");
    assert_ne!(name, "Lounge");
}

/// A pairing button is a configuration control, not something anyone wants on
/// the room card next to the shade's own open/close. `entity_category: config`
/// is what keeps an accidental tap out of easy reach — and an accidental tap
/// here transmits `Prog` at a real motor.
#[test]
fn the_button_is_filed_as_configuration() {
    let payload = render_button(&config(), ShadeId(3), "Lounge");
    assert_eq!(payload["entity_category"], "config");
}

/// A name at the limit still renders, and the buffer proves it rather than the
/// reader assuming it.
#[test]
fn a_name_at_the_limit_still_renders() {
    let name = "x".repeat(somfy_mqtt::MAX_NAME_LEN);
    let payload = render_button(&config(), ShadeId(255), &name);
    assert!(payload["name"].as_str().unwrap().starts_with(&name));
}

/// A name past the limit is refused rather than truncated, for the same reason
/// the cover refuses one: truncated JSON is a payload Home Assistant discards
/// without saying so.
#[test]
fn a_name_past_the_limit_is_refused_and_leaves_nothing_behind() {
    let cfg = config();
    let name = "x".repeat(somfy_mqtt::MAX_NAME_LEN + 1);
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    assert!(cfg
        .button_discovery(ShadeId(0), &name)
        .render(&mut buf)
        .is_err());
    assert!(
        buf.is_empty(),
        "a refused render must leave no partial payload"
    );
}

// ---------------------------------------------------------------------------
// The lifecycle
// ---------------------------------------------------------------------------

/// The button joins both halves at once, because both read `SHADE_COMPONENTS`.
/// An entity that can be announced and not removed is a retained orphan only an
/// MQTT client can clear.
#[test]
fn the_button_is_announced_and_retired_from_the_same_table() {
    assert!(SHADE_COMPONENTS.contains(&Component::Button));

    let cfg = config();
    let shade = ShadeId(3);
    let expected = cfg
        .discovery_topic(Component::Button, &ObjectId::for_shade(shade))
        .as_str()
        .to_owned();

    let announced = cfg.announce_shade(shade, false).any(|step| match step {
        Step::Send(publish) => {
            publish.topic().as_str() == expected
                && matches!(
                    publish.payload(),
                    Payload::Discovery {
                        component: Component::Button,
                        ..
                    }
                )
                && publish.retention() == Retention::Retained
        }
        Step::Listen(_) => false,
    });
    assert!(announced, "the button's discovery config is not announced");

    let retired = cfg.retire_shade(shade).any(|step| match step {
        Step::Send(publish) => {
            publish.topic().as_str() == expected
                && publish.payload() == Payload::Nothing
                && publish.retention() == Retention::Retained
        }
        Step::Listen(_) => false,
    });
    assert!(retired, "the button's discovery config is never cleared");
}

/// Announcing a shade subscribes to its pairing topic. A button whose presses
/// nothing is listening for is an entity that appears, works, and does nothing.
#[test]
fn announcing_a_shade_subscribes_to_its_pair_topic() {
    let cfg = config();
    let shade = ShadeId(3);
    let expected = cfg.shade_topic(shade, ShadeTopic::Pair).as_str().to_owned();

    for has_tilt in [false, true] {
        assert!(
            cfg.announce_shade(shade, has_tilt).any(|step| match step {
                Step::Listen(listen) => listen.topic().as_str() == expected,
                Step::Send(_) => false,
            }),
            "has_tilt={has_tilt}",
        );
    }
}
