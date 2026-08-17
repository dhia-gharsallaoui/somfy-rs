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
    Component, DeviceEntity, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, ShadeTopic,
    StateRoot, MAX_DEVICE_ID_LEN, MAX_DISCOVERY_PREFIX_LEN, MAX_NAME_LEN, MAX_NODE_ID_LEN,
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
    // Two shapes of object id, and the constant is deliberately wider than
    // either: `DeviceEntity::MAX_SLUG_LEN` folds over a hand-maintained array,
    // so sizing to it exactly would look proven and would not be. The actual
    // widest is pinned, and the headroom is what is left over.
    let object = ObjectId::for_shade(WIDEST_SHADE);
    assert_eq!(object.as_str(), "shade_255");
    let widest_object = DeviceEntity::ALL
        .iter()
        .map(|e| ObjectId::for_device(*e).as_str().len())
        .chain(core::iter::once(object.as_str().len()))
        .max()
        .unwrap();
    assert_eq!(
        widest_object, 16,
        "`rollcode_damaged` is the widest object id"
    );
    assert!(
        widest_object < MAX_OBJECT_ID_LEN,
        "the object-id budget has no headroom left",
    );

    let mut widest = 0;
    for component in Component::ALL {
        for object in DeviceEntity::ALL
            .iter()
            .map(|e| ObjectId::for_device(*e))
            .chain(core::iter::once(ObjectId::for_shade(WIDEST_SHADE)))
        {
            widest = widest.max(cfg.discovery_topic(component, &object).len());
        }
    }
    for (_, topic) in cfg.shade_topics(WIDEST_SHADE, true) {
        widest = widest.max(topic.len());
    }
    for (_, topic) in cfg.device_topics() {
        widest = widest.max(topic.len());
    }
    widest = widest.max(cfg.availability_topic().len());
    widest = widest.max(cfg.shade_base(WIDEST_SHADE).len());
    widest = widest.max(cfg.device_base().len());

    // Pinned rather than merely bounded. `Topic` wraps a `String<TOPIC_CAPACITY>`,
    // so "it is under the capacity" is a type invariant that no test can
    // observe failing — an overrun panics inside the builder long before the
    // assertion runs. The number is the thing worth watching: if a change moves
    // it, the capacity budget deserves a fresh look rather than a silent slide.
    assert_eq!(
        widest, 135,
        "widest topic moved; re-check the budget against TOPIC_CAPACITY = {TOPIC_CAPACITY}",
    );
    assert!(widest < TOPIC_CAPACITY);
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

    // Pinned for the same reason as the widest topic: the bound is a type
    // invariant, the number is the signal. Adding a payload field moves it, and
    // that is exactly when the budget wants re-checking.
    assert_eq!(
        buf.len(),
        864,
        "widest payload moved; re-check the budget against PAYLOAD_CAPACITY = {PAYLOAD_CAPACITY}",
    );
    // It must still be valid JSON at the extreme, not merely short enough.
    let parsed: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(parsed["name"].as_str(), Some(name.as_str()));
}

/// The same measurement for the entities R7 adds. Their payloads have no user
/// text in them at all — every string is a firmware literal or a validated
/// identifier — so the widest is reached at the widest *configuration* rather
/// than at the widest input.
#[test]
fn the_widest_diagnostic_payload_fits() {
    let cfg = maximal_config();
    let mut widest = 0;
    for entity in DeviceEntity::ALL {
        let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
        cfg.diagnostic_discovery(entity)
            .render(&mut buf)
            .expect("the widest diagnostic payload must fit");
        let parsed: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
        assert_eq!(parsed["name"].as_str(), Some(entity.label()));
        widest = widest.max(buf.len());
    }
    assert_eq!(
        widest, 610,
        "widest diagnostic payload moved; re-check the budget against \
         PAYLOAD_CAPACITY = {PAYLOAD_CAPACITY}",
    );
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
    // One payload, not two concatenated. Compared against a freshly rendered
    // one rather than by counting braces, because a payload legitimately
    // contains a nested object — the `device` block — and a brace count would
    // have to be updated every time the shape changes rather than checking
    // the thing that matters.
    let mut fresh: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(ShadeId(2), "Kitchen", false)
        .render(&mut fresh)
        .unwrap();
    assert_eq!(buf, fresh);

    // A failing render clears what the previous success left behind.
    let result = cfg
        .cover_discovery(ShadeId(3), &"x".repeat(MAX_NAME_LEN + 1), false)
        .render(&mut buf);
    assert_eq!(result, Err(somfy_mqtt::PayloadError::TooLong));
    assert!(buf.is_empty());
}

/// Identifiers keep their *content* at the widest inputs.
///
/// Asserting only that they fit their own bound would be a tautology — they are
/// `heapless::String`s of exactly that capacity — and worse, it would be blind
/// to the failure that matters. An identifier that overruns is not too long; it
/// is silently *short*, missing whichever piece did not fit, and two shades
/// whose ids fell off share one identity. So the assertions are on what the
/// identifier still contains.
#[test]
fn identifiers_keep_every_part_at_the_widest_inputs() {
    let device = DeviceId::new(&"d".repeat(MAX_DEVICE_ID_LEN)).unwrap();
    for component in Component::ALL {
        let unique = somfy_mqtt::UniqueId::for_shade(&device, component, WIDEST_SHADE);
        let got = unique.as_str();
        assert!(got.starts_with(device.as_str()), "{got} lost the device id");
        assert!(
            got.contains(component.as_str()),
            "{got} lost the component {component:?}",
        );
        assert!(got.ends_with("_255"), "{got} lost the shade id");
        assert!(got.len() <= MAX_UNIQUE_ID_LEN);
    }

    // Distinct components must still yield distinct identifiers — the property
    // truncation destroys first.
    let ids: Vec<String> = Component::ALL
        .iter()
        .map(|c| {
            somfy_mqtt::UniqueId::for_shade(&device, *c, WIDEST_SHADE)
                .as_str()
                .to_owned()
        })
        .collect();
    let mut unique_ids = ids.clone();
    unique_ids.sort();
    unique_ids.dedup();
    assert_eq!(
        unique_ids.len(),
        ids.len(),
        "two components share a unique_id: {ids:?}"
    );

    for id in [0u8, 9, 10, 99, 100, 255] {
        let object = ObjectId::for_shade(ShadeId(id));
        assert!(
            object.as_str().ends_with(&id.to_string()),
            "{object:?} lost the shade id"
        );
        assert!(object.as_str().len() <= MAX_OBJECT_ID_LEN);
    }
}

/// The relative paths, pinned by value.
///
/// Comparing the joined lengths against `MAX_RELATIVE_LEN` would restate the
/// definition — that constant *is* the maximum of this expression over this
/// array — so the exact figure is pinned instead, and it is the figure the
/// topic capacity proof spends.
#[test]
fn the_longest_relative_path_is_pinned() {
    assert_eq!(ShadeTopic::MAX_RELATIVE_LEN, 14);
    let longest = ShadeTopic::ALL
        .iter()
        .map(|t| t.segments().iter().map(|s| s.len() + 1).sum::<usize>())
        .max()
        .unwrap();
    assert_eq!(longest, 14, "`/direction/set` is the longest relative path");
}
