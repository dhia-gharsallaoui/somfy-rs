//! The capacity claims, checked against real maximal inputs.
//!
//! `config.rs` and `entity.rs` assert at compile time that the buffers are big
//! enough, which is what lets topic construction be infallible and lets the
//! builder panic rather than truncate. Those assertions are arithmetic, and
//! arithmetic can be wrong in a direction that only shows up as a truncated
//! topic on a device in someone's house. So the widest input this crate's own
//! limits permit is built here and measured.

use somfy_domain::ShadeId;
use somfy_mqtt::{
    Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, ShadeTopic, StateRoot,
    MAX_DEVICE_ID_LEN, MAX_DISCOVERY_PREFIX_LEN, MAX_NAME_LEN, MAX_NAME_PART_LEN, MAX_NODE_ID_LEN,
    MAX_OBJECT_ID_LEN, MAX_STATE_ROOT_LEN, MAX_UNIQUE_ID_LEN, PAYLOAD_CAPACITY, TOPIC_CAPACITY,
};

/// The widest shade id, and therefore the most digits.
const WIDEST_SHADE: ShadeId = ShadeId(255);

fn maximal_config() -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new(&"p".repeat(MAX_DISCOVERY_PREFIX_LEN)).unwrap(),
        StateRoot::new(&"r".repeat(MAX_STATE_ROOT_LEN)).unwrap(),
        NodeId::new(&"n".repeat(MAX_NODE_ID_LEN)).unwrap(),
        DeviceId::new(&"d".repeat(MAX_DEVICE_ID_LEN)).unwrap(),
    )
    .unwrap()
}

#[test]
fn the_widest_topics_fit_with_room_to_spare() {
    let cfg = maximal_config();
    // A name long enough to fill the object id's name part completely.
    let object = ObjectId::for_shade(&"o".repeat(MAX_NAME_PART_LEN * 2), WIDEST_SHADE);
    assert_eq!(object.as_str().len(), MAX_OBJECT_ID_LEN);

    let mut widest = 0;
    for component in Component::ALL {
        let topic = cfg.discovery_topic(component, &object);
        widest = widest.max(topic.len());
    }
    for (_, topic) in cfg.shade_topics(WIDEST_SHADE, true) {
        widest = widest.max(topic.len());
    }
    widest = widest.max(cfg.availability_topic().len());
    widest = widest.max(cfg.shade_base(WIDEST_SHADE).len());

    assert!(
        widest <= TOPIC_CAPACITY,
        "widest topic is {widest} bytes, capacity is {TOPIC_CAPACITY}",
    );
}

#[test]
fn the_widest_payload_fits() {
    let cfg = maximal_config();
    // Control characters escape to six bytes each, which is the worst a name
    // can do to the payload's length.
    let name = "\u{1}".repeat(MAX_NAME_LEN);
    assert_eq!(name.len(), MAX_NAME_LEN);

    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(WIDEST_SHADE, &name, true)
        .render(&mut buf)
        .expect("the widest payload must fit");

    assert!(
        buf.len() <= PAYLOAD_CAPACITY,
        "widest payload is {} bytes, capacity is {PAYLOAD_CAPACITY}",
        buf.len(),
    );
    // It must still be valid JSON at the extreme, not merely short enough.
    let parsed: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(parsed["name"].as_str(), Some(name.as_str()));
}

/// A name longer than the payload budget is refused, not truncated. A truncated
/// payload is invalid JSON, and Home Assistant discards invalid JSON silently —
/// the entity simply never appears, with nothing anywhere to explain why.
#[test]
fn an_over_long_name_is_refused_rather_than_truncated() {
    let cfg = maximal_config();
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    let result = cfg
        .cover_discovery(WIDEST_SHADE, &"x".repeat(MAX_NAME_LEN + 1), true)
        .render(&mut buf);

    assert_eq!(result, Err(somfy_mqtt::PayloadError::TooLong));
    // And the buffer is left empty rather than holding a truncated payload a
    // careless caller could publish.
    assert!(buf.is_empty(), "a failed render left {buf:?} behind");
}

/// A buffer that already held something is not appended to, and a failed render
/// does not leave the previous payload in place to be republished.
#[test]
fn render_never_leaves_a_stale_or_partial_payload() {
    let cfg = maximal_config();
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();

    cfg.cover_discovery(WIDEST_SHADE, "Lounge", true)
        .render(&mut buf)
        .unwrap();
    let good = buf.as_str().to_owned();
    assert!(good.starts_with('{') && good.ends_with('}'));

    // Reusing the same buffer replaces rather than appends.
    cfg.cover_discovery(ShadeId(2), "Kitchen", false)
        .render(&mut buf)
        .unwrap();
    assert!(buf.starts_with('{') && buf.ends_with('}'));
    assert_eq!(buf.matches('{').count(), 1);

    // A failing render clears what the previous success left behind.
    let result = cfg
        .cover_discovery(ShadeId(3), &"x".repeat(MAX_NAME_LEN + 1), false)
        .render(&mut buf);
    assert_eq!(result, Err(somfy_mqtt::PayloadError::TooLong));
    assert!(buf.is_empty());
}

/// The identifier bounds hold at their widest, which is what the topic capacity
/// proof depends on.
#[test]
fn identifiers_stay_within_their_declared_bounds() {
    let device = DeviceId::new(&"d".repeat(MAX_DEVICE_ID_LEN)).unwrap();
    for component in Component::ALL {
        let unique = somfy_mqtt::UniqueId::for_shade(&device, component, WIDEST_SHADE);
        assert!(
            unique.as_str().len() <= MAX_UNIQUE_ID_LEN,
            "{} is {} bytes, bound is {MAX_UNIQUE_ID_LEN}",
            unique.as_str(),
            unique.as_str().len(),
        );
    }

    for name in ["", &"z".repeat(1000), "日本語", "Salon / Porte-fenêtre"] {
        let object = ObjectId::for_shade(name, WIDEST_SHADE);
        assert!(object.as_str().len() <= MAX_OBJECT_ID_LEN);
        assert!(!object.as_str().is_empty());
    }
}

/// Every relative path the payload can carry is covered by the length the
/// capacity proof assumes for it.
#[test]
fn no_shade_topic_relative_path_exceeds_the_assumed_maximum() {
    for topic in ShadeTopic::ALL {
        let joined: usize = topic.segments().iter().map(|s| s.len() + 1).sum();
        assert!(
            joined <= ShadeTopic::MAX_RELATIVE_LEN,
            "{topic:?} joins to {joined} bytes, assumed maximum is {}",
            ShadeTopic::MAX_RELATIVE_LEN,
        );
    }
}
