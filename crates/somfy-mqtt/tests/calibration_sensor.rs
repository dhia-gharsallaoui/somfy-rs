//! The per-shade calibration sensor: where one shade's travel times came from,
//! said in the place the consequence lands.
//!
//! Home Assistant shows a cover's position. That position is
//! `elapsed / travel_time`, and on 2026-08-17 three shades were found carrying
//! travel times nobody had ever chosen — so a request for 25% open moved a shade
//! about 1% while the cover entity reported 25% the whole way. R7 of the
//! position-accuracy requirements is a MUST and asks for the uncalibrated state
//! to be surfaced "wherever the UI shows a shade's timings"; Home Assistant
//! shows a number computed from them, and until this entity existed it said
//! nothing at all.
//!
//! What these check is the pair of failures this crate exists to prevent — an
//! entity whose payload names a topic the firmware does not act on, and an
//! entity that can be announced and not removed — plus the one that is specific
//! to a third per-shade entity: that its identity is genuinely free.

use std::collections::BTreeMap;

use serde_json::Value;
use somfy_domain::ShadeId;
use somfy_mqtt::{
    CalibrationState, Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, Pairing,
    Payload, PublishedTopic, Retention, ShadeTopic, StateRoot, Step, SubscribedTopic, TopicRole,
    MAX_STATE_LEN, PAYLOAD_CAPACITY, SHADE_COMPONENTS,
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

fn render(cfg: &MqttConfig, shade: ShadeId, name: &str) -> Value {
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.calibration_discovery(shade, name)
        .render(&mut buf)
        .expect("payload fits");
    serde_json::from_str(&buf).expect("rendered payload is valid JSON")
}

// ---------------------------------------------------------------------------
// The topic
// ---------------------------------------------------------------------------

/// A reading, so the firmware publishes it and Home Assistant never writes it.
///
/// The direction is worth pinning rather than assuming: a subscribed topic here
/// would be a state Home Assistant could set, which is a device that reports its
/// travel times as measured because somebody typed the word.
#[test]
fn the_calibration_topic_is_published_and_never_subscribed() {
    assert_eq!(ShadeTopic::Calibration.role(), TopicRole::Published);
    assert!(PublishedTopic::of(ShadeTopic::Calibration).is_some());
    assert!(SubscribedTopic::of(ShadeTopic::Calibration).is_none());
}

/// It exists on every shade, tilt or not — the travel times it reports on are
/// the two the position estimate is computed from, which every shade has.
#[test]
fn every_shade_has_a_calibration_topic() {
    for has_tilt in [false, true] {
        assert!(
            ShadeTopic::for_shade(has_tilt).any(|t| t == ShadeTopic::Calibration),
            "has_tilt={has_tilt}",
        );
    }
}

/// It claims no discovery-payload key.
///
/// `ShadeTopic::State` already claims `state_topic` in the cover payload, and a
/// second topic claiming it would write the key twice into one JSON object —
/// which parses, keeps one of them, and leaves the other entity pointing
/// nowhere. The sensor names the topic itself instead.
#[test]
fn the_calibration_topic_claims_no_payload_key() {
    assert_eq!(ShadeTopic::Calibration.payload_key(), None);
    assert_eq!(ShadeTopic::State.payload_key(), Some("state_topic"));
}

// ---------------------------------------------------------------------------
// The payload
// ---------------------------------------------------------------------------

/// The round trip: the topic the payload names, after `~` expansion, is the
/// topic the firmware publishes to.
#[test]
fn the_payload_names_the_topic_the_firmware_publishes_to() {
    let cfg = config();
    let shade = ShadeId(3);
    let payload = render(&cfg, shade, "Lounge");
    let object = payload.as_object().unwrap();

    let base = object.get("~").and_then(Value::as_str).unwrap();
    let state = object.get("state_topic").and_then(Value::as_str).unwrap();
    let absolute = state
        .strip_prefix('~')
        .map(|rest| format!("{base}{rest}"))
        .unwrap_or_else(|| state.to_owned());

    let firmware: BTreeMap<String, TopicRole> = cfg
        .shade_topics(shade, true)
        .map(|(topic, absolute)| (absolute.as_str().to_owned(), topic.role()))
        .collect();

    assert_eq!(
        firmware.get(&absolute),
        Some(&TopicRole::Published),
        "{absolute:?} is not a topic the firmware publishes to",
    );
    assert_eq!(
        absolute,
        cfg.shade_topic(shade, ShadeTopic::Calibration).as_str()
    );
}

/// **The identity question, which is why this is a sensor and not a button.**
///
/// An entity's identity here is `(device, component, shade id)`, so a shade may
/// own one entity of each component and no more — which is why the vent command
/// rides the cover's command topic instead of becoming a second button. `sensor`
/// was the component a shade did not already own, and this asserts that all
/// three identities are genuinely distinct rather than merely intended to be.
#[test]
fn all_three_of_a_shades_entities_have_distinct_identities() {
    let cfg = config();
    let shade = ShadeId(3);

    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(shade, "Lounge", false)
        .render(&mut buf)
        .unwrap();
    let cover: Value = serde_json::from_str(&buf).unwrap();
    cfg.button_discovery(shade, "Lounge")
        .render(&mut buf)
        .unwrap();
    let button: Value = serde_json::from_str(&buf).unwrap();
    let sensor = render(&cfg, shade, "Lounge");

    let uniques = [
        &cover["unique_id"],
        &button["unique_id"],
        &sensor["unique_id"],
    ];
    for (i, a) in uniques.iter().enumerate() {
        for b in uniques.iter().skip(i + 1) {
            assert_ne!(a, b, "two of a shade's entities share a unique_id");
        }
    }

    // The names differ too, because Home Assistant shows an entity name beside
    // the *device* name — three entities called "Lounge" would be three rows a
    // person cannot tell apart.
    let names = [&cover["name"], &button["name"], &sensor["name"]];
    for (i, a) in names.iter().enumerate() {
        for b in names.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }

    // The discovery topics differ by component, so the object id may — and does
    // — stay the shade's own on all three.
    assert_ne!(
        cfg.discovery_topic(Component::Sensor, &ObjectId::for_shade(shade))
            .as_str(),
        cfg.discovery_topic(Component::Cover, &ObjectId::for_shade(shade))
            .as_str(),
    );
    assert_ne!(
        cfg.discovery_topic(Component::Sensor, &ObjectId::for_shade(shade))
            .as_str(),
        cfg.discovery_topic(Component::Button, &ObjectId::for_shade(shade))
            .as_str(),
    );
}

/// A per-shade sensor's identity cannot collide with a device-level one however
/// either set grows: a shade's suffix is decimal digits and a `DeviceEntity`'s
/// is a slug beginning with a letter. Checked across every shade id rather than
/// left as an observation about two naming conventions.
#[test]
fn no_shade_sensor_can_collide_with_a_device_sensor() {
    let cfg = config();
    let device: Vec<String> = somfy_mqtt::DeviceEntity::ALL
        .into_iter()
        .map(|entity| {
            cfg.discovery_topic(Component::Sensor, &ObjectId::for_device(entity))
                .as_str()
                .to_owned()
        })
        .collect();

    for id in 0..=u8::MAX {
        let topic = cfg
            .discovery_topic(Component::Sensor, &ObjectId::for_shade(ShadeId(id)))
            .as_str()
            .to_owned();
        assert!(
            !device.contains(&topic),
            "shade {id}'s calibration sensor lands on a device diagnostic's topic",
        );
    }
}

/// `diagnostic`, so it files with the controller's own readings rather than
/// standing on the room card. And **no** `device_class`, `state_class`,
/// `unit_of_measurement` or `options`: a `sensor` carrying `device_class: enum`
/// must also carry `options` and must not carry `state_class`, and a payload
/// that gets that combination wrong is discarded whole — the entity never
/// appears, with no message anywhere. The plain shape is the deliberate one.
#[test]
fn the_payload_is_the_plain_diagnostic_shape() {
    let payload = render(&config(), ShadeId(3), "Lounge");
    let object = payload.as_object().unwrap();

    assert_eq!(object.get("entity_category").unwrap(), "diagnostic");
    for absent in [
        "device_class",
        "state_class",
        "unit_of_measurement",
        "options",
        "command_topic",
    ] {
        assert!(
            !object.contains_key(absent),
            "the calibration sensor should carry no {absent}",
        );
    }
}

/// A name is arbitrary user text and the suffix is a literal, so both go through
/// the escaper. A payload that stops parsing is an entity that never appears.
#[test]
fn a_hostile_name_still_renders_parseable_json() {
    let payload = render(&config(), ShadeId(7), "Salon \"quote\" / \\slash\\\n");
    assert_eq!(
        payload["name"],
        Value::String("Salon \"quote\" / \\slash\\\n calibration".to_owned()),
    );
}

/// Over-long names are refused rather than truncated, and the buffer is left
/// empty rather than half-written — a truncated payload is invalid JSON, which
/// Home Assistant discards without saying so.
#[test]
fn an_over_long_name_is_refused_and_leaves_nothing_behind() {
    let cfg = config();
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    let long = "x".repeat(somfy_mqtt::MAX_NAME_LEN + 1);
    assert!(cfg
        .calibration_discovery(ShadeId(1), &long)
        .render(&mut buf)
        .is_err());
    assert!(buf.is_empty());
}

// ---------------------------------------------------------------------------
// The state vocabulary
// ---------------------------------------------------------------------------

/// The three strings are distinct, non-empty, and short enough that Home
/// Assistant stores them.
///
/// The length bound is not cosmetic: a `sensor` with no `device_class` runs its
/// state through `check_state_too_long`, which replaces an over-long value with
/// `unknown` — so an over-long state does not truncate, it **loses the message**
/// and reports the entity as broken.
#[test]
fn every_calibration_state_is_distinct_and_fits() {
    let mut seen: Vec<&str> = Vec::new();
    for state in CalibrationState::ALL {
        let text = state.as_str();
        assert!(!text.is_empty(), "{state:?} has no text");
        assert!(
            text.len() <= MAX_STATE_LEN,
            "{state:?} is too long to store"
        );
        assert!(!seen.contains(&text), "{text} is used twice");
        seen.push(text);
    }
    assert_eq!(seen.len(), 3);
    assert_eq!(CalibrationState::MAX_LEN, "entered by hand".len());
}

/// **The worst of the two travel times wins**, exhaustively.
///
/// Half a calibration is what produces a shade that is right on the way up and a
/// tenth out on the way down — the two are never mirrored, because closing is
/// gravity-assisted — so anything short of both measured has to say so.
#[test]
fn the_state_is_the_worst_of_the_two_travel_times() {
    use somfy_domain::CalibrationSource as Source;
    let cases = [
        (
            Source::Measured,
            Source::Measured,
            CalibrationState::Measured,
        ),
        (
            Source::Measured,
            Source::OperatorSupplied,
            CalibrationState::EnteredByHand,
        ),
        (
            Source::OperatorSupplied,
            Source::Measured,
            CalibrationState::EnteredByHand,
        ),
        (
            Source::OperatorSupplied,
            Source::OperatorSupplied,
            CalibrationState::EnteredByHand,
        ),
        (
            Source::Measured,
            Source::FactoryDefault,
            CalibrationState::NotCalibrated,
        ),
        (
            Source::FactoryDefault,
            Source::Measured,
            CalibrationState::NotCalibrated,
        ),
        (
            Source::FactoryDefault,
            Source::OperatorSupplied,
            CalibrationState::NotCalibrated,
        ),
        (
            Source::FactoryDefault,
            Source::FactoryDefault,
            CalibrationState::NotCalibrated,
        ),
    ];
    for (up, down, expected) in cases {
        assert_eq!(
            CalibrationState::of(up, down),
            expected,
            "up={up:?} down={down:?}",
        );
    }
}

// ---------------------------------------------------------------------------
// The lifecycle
// ---------------------------------------------------------------------------

/// The sensor joins both halves at once, because both read `SHADE_COMPONENTS`.
///
/// This is the property that makes R5 hold by construction rather than by care:
/// a component that can be announced and not removed is 49 retained topics
/// deleted by hand, which is what happened before the two arrays became one.
#[test]
fn announcing_the_sensor_makes_it_removable() {
    let cfg = config();
    let shade = ShadeId(3);
    assert!(SHADE_COMPONENTS.contains(&Component::Sensor));

    let topic = cfg
        .discovery_topic(Component::Sensor, &ObjectId::for_shade(shade))
        .as_str()
        .to_owned();

    let announced = cfg
        .announce_shade(shade, false, Pairing::Offered)
        .any(|step| match step {
            Step::Send(publish) => {
                publish.topic().as_str() == topic
                    && matches!(
                        publish.payload(),
                        Payload::Discovery {
                            component: Component::Sensor,
                            ..
                        }
                    )
            }
            Step::Listen(_) => false,
        });
    assert!(announced, "the announcement never published the sensor");

    let retired = cfg.retire_shade(shade).any(|step| match step {
        Step::Send(publish) => {
            publish.topic().as_str() == topic
                && publish.retention() == Retention::Retained
                && matches!(publish.payload(), Payload::Nothing)
        }
        Step::Listen(_) => false,
    });
    assert!(retired, "the retirement never cleared the sensor's config");
}

/// The sensor is offered on every shade, including one whose address this
/// controller did not allocate.
///
/// The pairing button is withheld there because pairing an imported address does
/// nothing. That argument does not reach this entity: an imported shade's travel
/// times came across in the backup, they are the ones most likely to be the
/// reference firmware's untouched defaults, and its position is exactly as wrong
/// as anyone else's.
#[test]
fn an_imported_shade_still_gets_a_calibration_sensor() {
    for pairing in [Pairing::Offered, Pairing::Withheld] {
        let components: Vec<Component> = pairing.components().collect();
        assert!(
            components.contains(&Component::Sensor),
            "{pairing:?} withheld the calibration sensor",
        );
        assert!(components.contains(&Component::Cover));
    }
    assert!(!Pairing::Withheld
        .components()
        .any(|c| c == Component::Button));
}

/// The state topic is retained, and cleared by the retirement.
///
/// Retained because it is a reading Home Assistant must have on reconnect
/// without waiting for a shade to be edited; cleared because a retained reading
/// for a shade that no longer exists is the orphan R5 is about.
#[test]
fn the_state_is_retained_and_cleared_with_the_shade() {
    let cfg = config();
    let shade = ShadeId(3);
    let topic = cfg.shade_topic(shade, ShadeTopic::Calibration);

    let published = PublishedTopic::of(ShadeTopic::Calibration).expect("published");
    let state = cfg.state(
        shade,
        published,
        CalibrationState::Measured.as_str().as_bytes(),
    );
    assert_eq!(state.topic().as_str(), topic.as_str());
    assert!(state.is_retained());

    let cleared = cfg.retire_shade(shade).any(|step| match step {
        Step::Send(publish) => {
            publish.topic().as_str() == topic.as_str()
                && publish.retention() == Retention::Retained
                && matches!(publish.payload(), Payload::Nothing)
        }
        Step::Listen(_) => false,
    });
    assert!(cleared, "the retained calibration state was left behind");
}
