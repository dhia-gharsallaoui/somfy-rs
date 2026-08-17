//! Acceptance criterion 1 — topic construction is a pure function, table-tested,
//! including the three field-observed failure combinations.
//!
//! Each row states a `(state_root, discovery_prefix)` pair and what this crate
//! must do with it. The only two permitted outcomes are a **valid** topic or a
//! **refused** config. The observed failure was that every bad combination was
//! accepted and produced a topic nobody reads, so "accepted and wrong" is the
//! one outcome no row is allowed to have.

use somfy_domain::ShadeId;
use somfy_mqtt::{
    Component, ConfigError, DeviceId, DiscoveryPrefix, Field, MqttConfig, NodeId, ObjectId,
    ShadeTopic, StateRoot,
};

/// What a `(state_root, discovery_prefix)` pair must produce.
enum Outcome {
    /// The config is refused at the point of entry, with this error.
    Refused(ConfigError),
    /// The config is accepted, and yields exactly these topics.
    Accepted {
        discovery: &'static str,
        base: &'static str,
        availability: &'static str,
    },
}

struct Row {
    what: &'static str,
    state_root: &'static str,
    discovery_prefix: &'static str,
    outcome: Outcome,
}

/// `node_id` and `device_id` are held constant across the table so the rows
/// differ in exactly the axis under test.
const NODE: &str = "somfyrs";
const DEVICE: &str = "a1b2c3d4";

fn table() -> [Row; 8] {
    [
        // ------------------------------------------------------------------
        // The three combinations observed to fail in the field. Every one of
        // them was accepted there; here each is either refused or produces a
        // topic Home Assistant acts on.
        // ------------------------------------------------------------------
        Row {
            what: "field failure 1: state root prepended to the discovery topic",
            state_root: "mydevice",
            discovery_prefix: "homeassistant",
            // Observed: `mydevice/homeassistant/cover/1/config` — ignored,
            // because it is not under Home Assistant's prefix at all. The two
            // namespaces are independent, so the discovery topic must start at
            // the prefix and the state must stay under the root.
            outcome: Outcome::Accepted {
                discovery: "homeassistant/cover/somfyrs/shade_1/config",
                base: "mydevice/shades/1",
                availability: "mydevice/status",
            },
        },
        Row {
            what: "field failure 2: empty discovery prefix yields an empty segment",
            state_root: "homeassistant",
            discovery_prefix: "",
            // Observed: `homeassistant//cover/1/config` — ignored. An empty
            // segment is not a configuration outcome, so the config is refused.
            outcome: Outcome::Refused(ConfigError::Empty(Field::DiscoveryPrefix)),
        },
        Row {
            what: "field failure 3: empty state root yields leading-slash payload topics",
            state_root: "",
            discovery_prefix: "homeassistant",
            // Observed: discovered, but the payload said `"~": "/shades/1"`
            // while the device published to `shades/1`. Different topics, so
            // every entity was permanently `unavailable`.
            outcome: Outcome::Refused(ConfigError::Empty(Field::StateRoot)),
        },
        // ------------------------------------------------------------------
        // The near miss neither R3 nor R4 names, reached by fixing the empty
        // prefix of failure 2 while leaving its state root alone. Both values
        // are individually valid, so nothing short of a cross-field check
        // refuses it — and it puts availability on `homeassistant/status`,
        // which is Home Assistant's own birth and will topic. HA's birth
        // message would then mark the device available while it is offline.
        // ------------------------------------------------------------------
        Row {
            what: "state root equal to the discovery prefix",
            state_root: "homeassistant",
            discovery_prefix: "homeassistant",
            outcome: Outcome::Refused(ConfigError::Overlap(Field::StateRoot)),
        },
        Row {
            what: "state root nested inside the discovery prefix",
            state_root: "homeassistant/somfyrs",
            discovery_prefix: "homeassistant",
            outcome: Outcome::Refused(ConfigError::Overlap(Field::StateRoot)),
        },
        // ------------------------------------------------------------------
        // Configurations that must work.
        // ------------------------------------------------------------------
        Row {
            what: "the shipped default",
            state_root: "somfyrs",
            discovery_prefix: "homeassistant",
            outcome: Outcome::Accepted {
                discovery: "homeassistant/cover/somfyrs/shade_1/config",
                base: "somfyrs/shades/1",
                availability: "somfyrs/status",
            },
        },
        Row {
            what: "a multi-segment state root stays multi-segment",
            state_root: "home/blinds",
            discovery_prefix: "homeassistant",
            outcome: Outcome::Accepted {
                discovery: "homeassistant/cover/somfyrs/shade_1/config",
                base: "home/blinds/shades/1",
                availability: "home/blinds/status",
            },
        },
        Row {
            what: "a non-default discovery prefix moves only the discovery topic",
            state_root: "somfyrs",
            discovery_prefix: "ha/discovery",
            outcome: Outcome::Accepted {
                discovery: "ha/discovery/cover/somfyrs/shade_1/config",
                base: "somfyrs/shades/1",
                availability: "somfyrs/status",
            },
        },
    ]
}

fn config(state_root: &str, discovery_prefix: &str) -> Result<MqttConfig, ConfigError> {
    MqttConfig::new(
        DiscoveryPrefix::new(discovery_prefix)?,
        StateRoot::new(state_root)?,
        NodeId::new(NODE)?,
        DeviceId::new(DEVICE)?,
    )
}

#[test]
fn topic_construction_table() {
    for row in table() {
        let built = config(row.state_root, row.discovery_prefix);
        match row.outcome {
            Outcome::Refused(expected) => {
                let err = built.expect_err(row.what);
                assert_eq!(err, expected, "{}", row.what);
            }
            Outcome::Accepted {
                discovery,
                base,
                availability,
            } => {
                let cfg = built.unwrap_or_else(|e| panic!("{}: refused with {e:?}", row.what));
                let object = ObjectId::for_shade(ShadeId(1));
                assert_eq!(
                    cfg.discovery_topic(Component::Cover, &object).as_str(),
                    discovery,
                    "{}",
                    row.what
                );
                assert_eq!(cfg.shade_base(ShadeId(1)).as_str(), base, "{}", row.what);
                assert_eq!(
                    cfg.availability_topic().as_str(),
                    availability,
                    "{}",
                    row.what
                );
            }
        }
    }
}

/// No accepted row may produce a topic with an empty segment, whatever else it
/// gets wrong. This is the assertion the observed `homeassistant//cover/…`
/// would have tripped.
#[test]
fn no_accepted_row_ever_emits_an_empty_segment() {
    for row in table() {
        let Ok(cfg) = config(row.state_root, row.discovery_prefix) else {
            continue;
        };
        let object = ObjectId::for_shade(ShadeId(1));
        let mut topics = alloc_topics(&cfg, &object);
        topics.push(cfg.availability_topic().as_str().to_owned());
        for topic in topics {
            assert!(
                !topic.contains("//"),
                "{}: {topic:?} has an empty segment",
                row.what
            );
            assert!(
                !topic.starts_with('/'),
                "{}: {topic:?} has a leading slash",
                row.what
            );
            assert!(
                !topic.ends_with('/'),
                "{}: {topic:?} has a trailing slash",
                row.what
            );
        }
    }
}

fn alloc_topics(cfg: &MqttConfig, object: &ObjectId) -> Vec<String> {
    let mut out = vec![
        cfg.discovery_topic(Component::Cover, object)
            .as_str()
            .to_owned(),
        cfg.shade_base(ShadeId(1)).as_str().to_owned(),
    ];
    for (_, topic) in cfg.shade_topics(ShadeId(1), true) {
        out.push(topic.as_str().to_owned());
    }
    out
}

/// The contract verified against a live Home Assistant: the component segment
/// must come **immediately** after the discovery prefix.
///
/// `homeassistant/mydevice/cover/1/config` was ignored;
/// `homeassistant/cover/mydevice/1/config` created the entity. A `node_id`
/// placed before the component is the single-character difference between an
/// integration that works and one that silently does nothing.
#[test]
fn component_is_the_segment_immediately_after_the_prefix() {
    let cfg = config("somfyrs", "ha/discovery").unwrap();
    let object = ObjectId::for_shade(ShadeId(1));
    let topic = cfg.discovery_topic(Component::Cover, &object);
    let segments: Vec<&str> = topic.as_str().split('/').collect();

    assert_eq!(
        segments,
        ["ha", "discovery", "cover", "somfyrs", "shade_1", "config"]
    );
    // The prefix is two segments here, so "immediately after" is positional,
    // not a fixed index: the component follows the whole prefix.
    let after_prefix = segments
        .iter()
        .position(|s| *s == "discovery")
        .expect("prefix present")
        + 1;
    assert_eq!(segments[after_prefix], "cover");
    assert_eq!(*segments.last().unwrap(), "config");
}

/// A shade name is not a topic, and cannot become one: it is not an input to
/// any topic this crate builds. `ObjectId::for_shade` takes a [`ShadeId`] and
/// nothing else, so `Salon / Porte-fenêtre` has no path to a segment at all —
/// a stronger guarantee than sanitising the name would give, and one the
/// signature enforces rather than a function body.
///
/// It also means the discovery topic does not move when a shade is renamed,
/// which is what stops every rename leaving an orphaned retained config behind.
#[test]
fn a_shade_name_cannot_produce_topic_segments() {
    let cfg = config("somfyrs", "homeassistant").unwrap();
    let topic = cfg.discovery_topic(Component::Cover, &ObjectId::for_shade(ShadeId(7)));

    assert_eq!(topic.as_str(), "homeassistant/cover/somfyrs/shade_7/config");
    assert_eq!(topic.as_str().split('/').count(), 5);

    // The name still reaches Home Assistant, in the payload where it belongs.
    let mut payload: heapless::String<{ somfy_mqtt::PAYLOAD_CAPACITY }> = heapless::String::new();
    cfg.cover_discovery(ShadeId(7), "Salon / Porte-fenêtre", false)
        .render(&mut payload)
        .unwrap();
    assert!(
        payload.contains(r#""name":"Salon / Porte-fenêtre""#),
        "{payload}"
    );
}

/// The discovery topic is invariant under the name. This is the rename
/// stability the id-derived object id buys, asserted directly.
///
/// It goes through `cover_discovery`, which is the only public path with a name
/// in scope at all. Building an `ObjectId` here and comparing it against itself
/// would pass whatever `cover_discovery` did with the name — which is precisely
/// the thing under test.
#[test]
fn renaming_a_shade_does_not_move_its_discovery_topic() {
    let cfg = config("somfyrs", "homeassistant").unwrap();
    let shade = ShadeId(4);
    let expected = cfg
        .discovery_topic(Component::Cover, &ObjectId::for_shade(shade))
        .as_str()
        .to_owned();

    let mut first_unique_id: Option<String> = None;
    for name in [
        "Lounge",
        "Sitting room",
        "Salon / Porte-fenêtre",
        "",
        "日本語",
    ] {
        let discovery = cfg.cover_discovery(shade, name, false);

        // The address the config is published to does not follow the name...
        let topic = cfg.discovery_topic(Component::Cover, &discovery.object_id);
        assert_eq!(
            topic.as_str(),
            expected,
            "renaming to {name:?} moved the discovery topic"
        );
        assert_eq!(discovery.object_id.as_str(), "shade_4");

        // ...and neither does the identity Home Assistant remembers.
        match &first_unique_id {
            None => first_unique_id = Some(discovery.unique_id.as_str().to_owned()),
            Some(id) => assert_eq!(discovery.unique_id.as_str(), id),
        }
    }
}

/// Every component this crate can emit is a literal from Home Assistant's own
/// set, chosen by the firmware. There is no path by which a user string becomes
/// the component segment.
#[test]
fn component_segments_are_literals() {
    let cfg = config("somfyrs", "homeassistant").unwrap();
    let object = ObjectId::for_shade(ShadeId(1));
    for (component, expected) in [
        (Component::Cover, "cover"),
        (Component::Sensor, "sensor"),
        (Component::BinarySensor, "binary_sensor"),
        (Component::Button, "button"),
        (Component::Switch, "switch"),
        (Component::Update, "update"),
    ] {
        let topic = cfg.discovery_topic(component, &object);
        assert_eq!(topic.as_str().split('/').nth(1), Some(expected));
    }
}

/// R4: availability lives under the state root, never under the discovery
/// prefix. `{discovery_prefix}/status` is Home Assistant's own birth/will
/// topic — publishing availability there means HA's birth message marks the
/// device available while it is offline.
#[test]
fn availability_is_under_the_state_root_not_the_discovery_prefix() {
    let cfg = config("somfyrs", "homeassistant").unwrap();
    let availability = cfg.availability_topic();

    assert_eq!(availability.as_str(), "somfyrs/status");
    assert_ne!(availability.as_str(), "homeassistant/status");
    assert!(!availability.as_str().starts_with("homeassistant"));
}

/// R8: tilt topics exist only for tilt-capable shades. A non-tilt shade must
/// omit them rather than advertise a topic nothing ever publishes.
#[test]
fn tilt_topics_are_omitted_for_non_tilt_shades() {
    let cfg = config("somfyrs", "homeassistant").unwrap();

    let plain: Vec<ShadeTopic> = cfg
        .shade_topics(ShadeId(1), false)
        .map(|(t, _)| t)
        .collect();
    assert!(!plain.contains(&ShadeTopic::TiltStatus));
    assert!(!plain.contains(&ShadeTopic::TiltCommand));

    let tilting: Vec<ShadeTopic> = cfg.shade_topics(ShadeId(1), true).map(|(t, _)| t).collect();
    assert!(tilting.contains(&ShadeTopic::TiltStatus));
    assert!(tilting.contains(&ShadeTopic::TiltCommand));
}

/// The exact per-shade topics, pinned. These are the strings the firmware
/// publishes to and subscribes to, and the strings the discovery payload
/// resolves to.
#[test]
fn shade_topics_are_exact() {
    let cfg = config("somfyrs", "homeassistant").unwrap();
    let got: Vec<(ShadeTopic, String)> = cfg
        .shade_topics(ShadeId(3), true)
        .map(|(t, topic)| (t, topic.as_str().to_owned()))
        .collect();

    assert_eq!(
        got,
        vec![
            (ShadeTopic::Position, "somfyrs/shades/3/position".to_owned()),
            (ShadeTopic::State, "somfyrs/shades/3/direction".to_owned()),
            (ShadeTopic::Name, "somfyrs/shades/3/name".to_owned()),
            (
                ShadeTopic::Command,
                "somfyrs/shades/3/direction/set".to_owned()
            ),
            (
                ShadeTopic::SetPosition,
                "somfyrs/shades/3/target/set".to_owned()
            ),
            (ShadeTopic::Pair, "somfyrs/shades/3/pair/set".to_owned()),
            (ShadeTopic::TiltStatus, "somfyrs/shades/3/tilt".to_owned()),
            (
                ShadeTopic::TiltCommand,
                "somfyrs/shades/3/tilt/set".to_owned()
            ),
        ]
    );
}
