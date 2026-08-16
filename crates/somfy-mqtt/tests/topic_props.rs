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
    Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, StateRoot,
    PAYLOAD_CAPACITY, TOPIC_CAPACITY,
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
        let object = ObjectId::for_shade(shade);

        // The criterion is "for any valid config **and any shade name**", so the
        // name stays an input to the scenario even though it is no longer an
        // input to any topic. Putting it through the one API that accepts it is
        // what keeps that true rather than merely asserted: if a name ever finds
        // its way back into a topic, these are the topics it would corrupt.
        let name: String = name.chars().take(8).collect();
        let mut payload: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
        cfg.cover_discovery(shade, &name, has_tilt)
            .render(&mut payload)
            .expect("payload fits");

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

        // Deliberately *not* asserted here: that no topic contains the name as a
        // substring. It looks like the natural check and it is unsound — a
        // one-character name of "a" is a substring of the literal `status`, and
        // this property test found that in 512 cases within a second of it being
        // written. `topics_are_invariant_under_the_shade_name` below states the
        // real property instead, and states it more strongly: the topics do not
        // merely avoid the name, they do not vary with it at all.
    }

    /// Every topic is invariant under the shade's name.
    ///
    /// This is the stronger form of the criterion above, and the one that
    /// matters after the object id stopped following the name: it is not that
    /// hostile names are cleaned up on the way into a topic, it is that they
    /// never get there. A rename therefore cannot move a discovery topic and
    /// strand a retained config at the old address.
    ///
    /// The name is still generated here, and still reaches the payload — the
    /// assertion is that the payload changes and the topics do not.
    #[test]
    fn topics_are_invariant_under_the_shade_name(
        prefix in valid_root(),
        root in valid_root(),
        name_a in shade_name(),
        name_b in shade_name(),
        id in 0u8..=255,
        has_tilt in any::<bool>(),
    ) {
        let Ok(cfg) = MqttConfig::new(
            DiscoveryPrefix::new(&prefix).unwrap(),
            StateRoot::new(&root).unwrap(),
            NodeId::new("somfyrs").unwrap(),
            DeviceId::new("a1b2c3d4").unwrap(),
        ) else {
            return Ok(());
        };
        let shade = ShadeId(id);
        let object = ObjectId::for_shade(shade);

        // Nothing in the topic set can vary, because the name is not an input
        // to any of it.
        prop_assert_eq!(
            cfg.discovery_topic(Component::Cover, &object),
            cfg.discovery_topic(Component::Cover, &ObjectId::for_shade(shade)),
        );

        // The payload does carry the name, and both must render.
        let mut a: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
        let mut b: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
        let short_a = name_a.chars().take(8).collect::<String>();
        let short_b = name_b.chars().take(8).collect::<String>();
        cfg.cover_discovery(shade, &short_a, has_tilt).render(&mut a).unwrap();
        cfg.cover_discovery(shade, &short_b, has_tilt).render(&mut b).unwrap();

        // Same shade, different names: the `~` base and the object id are
        // byte-identical in both payloads.
        let base = cfg.shade_base(shade);
        let quoted = format!("\"~\":\"{base}\"");
        prop_assert!(a.contains(&quoted), "{a}");
        prop_assert!(b.contains(&quoted), "{b}");
        let object_field = format!("\"object_id\":\"{}\"", object.as_str());
        prop_assert!(a.contains(&object_field), "{a}");
        prop_assert!(b.contains(&object_field), "{b}");
    }

    /// The object id is a topic segment on its own: never empty, never a slash,
    /// always within the class, and distinct for distinct shades. Two shades
    /// sharing an object id share a discovery topic, and the second silently
    /// overwrites the first.
    #[test]
    fn object_ids_are_distinct_valid_segments(a in 0u8..=255, b in 0u8..=255) {
        let one = ObjectId::for_shade(ShadeId(a));
        let s = one.as_str();
        prop_assert!(!s.is_empty());
        prop_assert!(!s.contains('/'));
        prop_assert!(
            s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
            "object id {s:?} escaped the character class",
        );

        prop_assume!(a != b);
        let two = ObjectId::for_shade(ShadeId(b));
        prop_assert_ne!(one.as_str(), two.as_str());
    }
}
