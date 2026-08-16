//! Acceptance criterion 2 — for any valid config and any shade name, every
//! generated topic matches `^[a-zA-Z0-9_\-]+(/[a-zA-Z0-9_\-]+)*$` and contains
//! no `//`.
//!
//! The criterion is stated as a regular expression, so the test uses that
//! expression rather than a hand-rolled equivalent. A hand-rolled checker that
//! is subtly wrong passes everything, and a property test that cannot fail is
//! worse than no property test.

use proptest::prelude::*;
use regex::Regex;
use somfy_domain::ShadeId;
use somfy_mqtt::{
    Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, StateRoot, TOPIC_CAPACITY,
};

/// Verbatim from the acceptance criterion.
const TOPIC_GRAMMAR: &str = r"^[a-zA-Z0-9_\-]+(/[a-zA-Z0-9_\-]+)*$";

/// Any string a valid `discovery_prefix` or `state_root` may take: one or more
/// segments of the permitted characters, joined by single slashes. Anything
/// outside this is a *rejected* config and is the subject of the rejection
/// test, not this one.
fn valid_root() -> impl Strategy<Value = String> {
    proptest::collection::vec("[a-zA-Z0-9_-]{1,12}", 1..4).prop_map(|segs| segs.join("/"))
}

/// Any string a valid `node_id` or `device_id` may take: a single token.
fn valid_token() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,16}".prop_map(|s| s)
}

/// Deliberately hostile shade names: Unicode, slashes, spaces, empty, control
/// characters, MQTT wildcards, and very long.
fn shade_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("Salon / Porte-fenêtre".to_owned()),
        Just("//".to_owned()),
        Just("   ".to_owned()),
        Just("#".to_owned()),
        Just("+".to_owned()),
        Just("日本語".to_owned()),
        Just("\u{0}\u{1}\u{7f}".to_owned()),
        Just("🪟".to_owned()),
        Just("a".repeat(400)),
        Just("é".repeat(200)),
        ".{0,80}",
        "[a-zA-Z0-9 /_+#.$-]{0,40}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn every_generated_topic_matches_the_grammar(
        prefix in valid_root(),
        root in valid_root(),
        node in valid_token(),
        device in valid_token(),
        name in shade_name(),
        id in 0u8..=255,
        has_tilt in any::<bool>(),
    ) {
        let grammar = Regex::new(TOPIC_GRAMMAR).unwrap();
        let cfg = match MqttConfig::new(
            DiscoveryPrefix::new(&prefix).unwrap(),
            StateRoot::new(&root).unwrap(),
            NodeId::new(&node).unwrap(),
            DeviceId::new(&device).unwrap(),
        ) {
            Ok(cfg) => cfg,
            // The generator can produce two roots that name the same namespace.
            // That pair is refused by design and belongs to the rejection test,
            // not to this one — which is itself the property under test: a
            // refused config produces no topics at all.
            Err(_) => return Ok(()),
        };
        let shade = ShadeId(id);
        let object = ObjectId::for_shade(&name, shade);

        let mut topics = vec![
            cfg.availability_topic(),
            cfg.shade_base(shade),
        ];
        for component in [
            Component::Cover,
            Component::Sensor,
            Component::BinarySensor,
            Component::Button,
            Component::Switch,
            Component::Update,
        ] {
            topics.push(cfg.discovery_topic(component, &object));
        }
        for (_, topic) in cfg.shade_topics(shade, has_tilt) {
            topics.push(topic);
        }

        for topic in &topics {
            let s = topic.as_str();
            prop_assert!(grammar.is_match(s), "topic {s:?} does not match {TOPIC_GRAMMAR}");
            prop_assert!(!s.contains("//"), "topic {s:?} contains an empty segment");
            prop_assert!(s.len() <= TOPIC_CAPACITY, "topic {s:?} overran the capacity");
        }
    }

    /// The object id is a topic segment on its own, so it must satisfy the
    /// segment grammar for every possible name — never empty, never a slash.
    #[test]
    fn object_id_is_always_a_single_valid_segment(
        name in shade_name(),
        id in 0u8..=255,
    ) {
        let object = ObjectId::for_shade(&name, ShadeId(id));
        let s = object.as_str();
        prop_assert!(!s.is_empty());
        prop_assert!(!s.contains('/'));
        prop_assert!(
            s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "object id {s:?} escaped the character class",
        );
    }

    /// Distinct shades never share an object id, whatever they are named —
    /// two entities sharing a discovery topic means the second silently
    /// overwrites the first.
    #[test]
    fn object_ids_are_unique_per_shade(
        name_a in shade_name(),
        name_b in shade_name(),
        a in 0u8..=255,
        b in 0u8..=255,
    ) {
        prop_assume!(a != b);
        let one = ObjectId::for_shade(&name_a, ShadeId(a));
        let two = ObjectId::for_shade(&name_b, ShadeId(b));
        prop_assert_ne!(one.as_str(), two.as_str());
    }
}
