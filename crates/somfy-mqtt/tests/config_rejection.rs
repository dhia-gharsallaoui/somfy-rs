//! Acceptance criterion 4 — each invalid input in R3 returns a typed error
//! naming the field.
//!
//! The failure this guards against is not "a bad topic was built". It is that
//! every bad combination was **accepted**, published to an address nobody
//! reads, and looked like it had worked. So the assertion here is that
//! construction fails, and that the failure says which field was wrong.

use somfy_mqtt::{ConfigError, DeviceId, DiscoveryPrefix, Field, MqttConfig, NodeId, StateRoot};

/// The constructors under test, each paired with the field its errors must
/// name. Running one table over all four is deliberate: a validator that is
/// right for `discovery_prefix` and forgotten for `state_root` is precisely the
/// asymmetry that produced the field failure.
type Ctor = fn(&str) -> Result<(), ConfigError>;

fn roots() -> [(Field, Ctor); 2] {
    [
        (Field::DiscoveryPrefix, |s| {
            DiscoveryPrefix::new(s).map(|_| ())
        }),
        (Field::StateRoot, |s| StateRoot::new(s).map(|_| ())),
    ]
}

fn tokens() -> [(Field, Ctor); 2] {
    [
        (Field::NodeId, |s| NodeId::new(s).map(|_| ())),
        (Field::DeviceId, |s| DeviceId::new(s).map(|_| ())),
    ]
}

#[test]
fn empty_is_rejected() {
    for (field, ctor) in roots().into_iter().chain(tokens()) {
        assert_eq!(ctor(""), Err(ConfigError::Empty(field)), "{field:?}");
    }
}

#[test]
fn wildcards_are_rejected() {
    for (field, ctor) in roots() {
        for (value, ch) in [
            ("some/#", '#'),
            ("#", '#'),
            ("a/#/b", '#'),
            ("some/+", '+'),
            ("+", '+'),
            ("a/+/b", '+'),
        ] {
            assert_eq!(
                ctor(value),
                Err(ConfigError::Wildcard(field, ch)),
                "{field:?} {value:?}",
            );
        }
    }
}

#[test]
fn leading_and_trailing_slashes_are_rejected() {
    for (field, ctor) in roots() {
        assert_eq!(ctor("/somfyrs"), Err(ConfigError::LeadingSlash(field)));
        assert_eq!(ctor("/"), Err(ConfigError::LeadingSlash(field)));
        assert_eq!(ctor("somfyrs/"), Err(ConfigError::TrailingSlash(field)));
        // Both faults at once. The trailing slash is reported because it is the
        // more specific description of what the operator typed; either answer
        // refuses the value, which is what R3 requires.
        assert_eq!(ctor("a//"), Err(ConfigError::TrailingSlash(field)));
    }
}

#[test]
fn empty_segments_are_rejected() {
    for (field, ctor) in roots() {
        assert_eq!(ctor("a//b"), Err(ConfigError::EmptySegment(field)));
        assert_eq!(ctor("a///b"), Err(ConfigError::EmptySegment(field)));
        assert_eq!(ctor("a//b//c"), Err(ConfigError::EmptySegment(field)));
    }
}

/// A topic segment that is not `[a-zA-Z0-9_-]` cannot appear in a topic this
/// crate builds, so it is refused at the point of entry rather than silently
/// rewritten. Rewriting is what "degrades" means, and R3 forbids it.
#[test]
fn characters_outside_the_topic_class_are_rejected() {
    for (field, ctor) in roots() {
        for (value, ch) in [
            ("somfy rs", ' '),
            ("café", 'é'),
            ("a\u{0}b", '\u{0}'),
            ("a\nb", '\n'),
            ("a.b", '.'),
            ("$SYS", '$'),
        ] {
            assert_eq!(
                ctor(value),
                Err(ConfigError::IllegalCharacter(field, ch)),
                "{field:?} {value:?}",
            );
        }
    }
}

/// `node_id` and `device_id` are single topic segments, so a slash is not a
/// separator there — it is an illegal character.
#[test]
fn tokens_reject_slashes_outright() {
    for (field, ctor) in tokens() {
        assert_eq!(
            ctor("a/b"),
            Err(ConfigError::IllegalCharacter(field, '/')),
            "{field:?}",
        );
        assert_eq!(ctor("#"), Err(ConfigError::Wildcard(field, '#')));
        assert_eq!(ctor("+"), Err(ConfigError::Wildcard(field, '+')));
    }
}

/// Storage is fixed-size, so an over-long value has exactly two possible
/// treatments: truncate it, or refuse it. Truncation is a silent change of
/// address — the topic the device publishes to stops being the topic the
/// operator configured — so it is refused, and the error carries the limit.
#[test]
fn over_long_values_are_rejected_not_truncated() {
    for (field, ctor) in roots().into_iter().chain(tokens()) {
        let long = "a".repeat(1024);
        match ctor(&long) {
            Err(ConfigError::TooLong(got, limit)) => {
                assert_eq!(got, field);
                assert!(limit > 0 && limit < 1024, "{field:?} limit {limit}");
            }
            other => panic!("{field:?}: expected TooLong, got {other:?}"),
        }
    }
}

/// Every error names its field. This is the property the message-level
/// requirement reduces to, checked over the whole table rather than per case.
#[test]
fn every_error_names_its_field() {
    let bad = [
        "",
        "#",
        "+",
        "/a",
        "a/",
        "a//b",
        "a b",
        "café",
        "a\u{0}b",
        &"a".repeat(1024),
    ];
    for (field, ctor) in roots().into_iter().chain(tokens()) {
        for value in bad {
            let err = ctor(value).expect_err(&format!("{field:?} accepted {value:?}"));
            assert_eq!(err.field(), field, "{field:?} {value:?} -> {err:?}");
        }
    }
}

/// The one fault that belongs to a *pair* of values rather than to either one.
///
/// R3 lists per-field rules and R4 says availability must live under the state
/// root and never under the discovery prefix. Both are satisfied, literally, by
/// setting the two roots to the same string — and the result is availability on
/// `homeassistant/status`, which is Home Assistant's own birth and will topic.
/// HA's birth message would then mark the device available at the moment HA
/// restarts, whether or not the device is running: the precise failure R4
/// exists to prevent, reached by a route neither requirement names.
///
/// It is not a hypothetical route. It is what an operator lands on by fixing
/// the empty prefix of the second observed failure and leaving its state root
/// of `homeassistant` alone.
#[test]
fn overlapping_namespaces_are_rejected() {
    for (prefix, root) in [
        ("homeassistant", "homeassistant"),
        ("homeassistant", "homeassistant/somfyrs"),
        ("homeassistant/somfyrs", "homeassistant"),
        ("a/b", "a/b/c"),
        ("a", "a"),
    ] {
        let built = MqttConfig::new(
            DiscoveryPrefix::new(prefix).unwrap(),
            StateRoot::new(root).unwrap(),
            NodeId::new("somfyrs").unwrap(),
            DeviceId::new("a1b2c3d4").unwrap(),
        );
        assert_eq!(
            built.err(),
            Some(ConfigError::Overlap(Field::StateRoot)),
            "prefix {prefix:?} root {root:?}",
        );
    }
}

/// The overlap check compares at `/` boundaries, so namespaces that merely
/// share a textual prefix are unrelated and must still be accepted. Rejecting
/// `home` alongside `homeassistant` would be strictness for its own sake.
#[test]
fn merely_similar_namespaces_are_not_an_overlap() {
    for (prefix, root) in [
        ("homeassistant", "home"),
        ("home", "homeassistant"),
        ("homeassistant", "homeassistant2"),
        ("a/b", "a/bc"),
        ("homeassistant", "somfyrs"),
    ] {
        let built = MqttConfig::new(
            DiscoveryPrefix::new(prefix).unwrap(),
            StateRoot::new(root).unwrap(),
            NodeId::new("somfyrs").unwrap(),
            DeviceId::new("a1b2c3d4").unwrap(),
        );
        assert!(built.is_ok(), "prefix {prefix:?} root {root:?} was refused");
    }
}

/// The values that must be accepted, so the validator is not merely strict.
#[test]
fn valid_values_are_accepted() {
    for (field, ctor) in roots() {
        for value in [
            "homeassistant",
            "somfyrs",
            "a",
            "home/blinds",
            "a/b/c",
            "A-Z_0-9",
        ] {
            assert_eq!(ctor(value), Ok(()), "{field:?} rejected {value:?}");
        }
    }
    for (field, ctor) in tokens() {
        for value in ["somfyrs", "a", "A-Z_0-9", "a1b2c3d4"] {
            assert_eq!(ctor(value), Ok(()), "{field:?} rejected {value:?}");
        }
    }
}
