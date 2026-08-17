//! R7 — the entity set beyond covers, and the rule that decides what is in it.
//!
//! ## The rule
//!
//! **An entity backed by nothing is worse than an absent one.** A control that
//! never moves and a reading that never changes both present as a device fault
//! rather than as an unimplemented feature, and the requirements spec's own
//! acceptance criterion says so: *"Appearing is not working; the C++ build
//! produced three entities that were permanently `unavailable`."*
//!
//! So the set here is not chosen to reach a number. Every [`DeviceEntity`] is a
//! fact this firmware already holds and already prints at boot, and the ones it
//! does not hold are absent rather than stubbed — `docs/provenance.md` records
//! each omission with the condition for adding it.
//!
//! ## Why they are device-level and not per-shade
//!
//! None of them is about a shade. Heap use, uptime, signal strength and the
//! rolling-code store's health are properties of the controller, so they are
//! announced once rather than once per shade — which also keeps the
//! announcement `k + 3N` rather than `k·N`.

use somfy_domain::ShadeId;
use somfy_mqtt::{
    Component, DeviceEntity, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, StateRoot,
    UniqueId, PAYLOAD_CAPACITY,
};

const NODE: &str = "somfyrs";
const DEVICE: &str = "a1b2c3d4";

fn config(state_root: &str, discovery_prefix: &str) -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new(discovery_prefix).expect("valid prefix"),
        StateRoot::new(state_root).expect("valid root"),
        NodeId::new(NODE).expect("valid node id"),
        DeviceId::new(DEVICE).expect("valid device id"),
    )
    .expect("valid config")
}

fn default_config() -> MqttConfig {
    config("somfyrs", "homeassistant")
}

fn render(cfg: &MqttConfig, entity: DeviceEntity) -> serde_json::Value {
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.diagnostic_discovery(entity)
        .render(&mut buf)
        .expect("a diagnostic payload fits");
    serde_json::from_str(&buf).expect("rendered payload is valid JSON")
}

// ---------------------------------------------------------------------------
// The identifiers
// ---------------------------------------------------------------------------

/// A slug becomes both a topic segment and an `object_id`, so it must satisfy
/// R2's character class — and it must do so by construction, because there is
/// no user text here to sanitise.
#[test]
fn every_slug_is_a_single_valid_topic_segment() {
    for entity in DeviceEntity::ALL {
        let slug = entity.slug();
        assert!(!slug.is_empty(), "{entity:?} has an empty slug");
        assert!(
            slug.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "{entity:?} slug {slug:?} escaped the character class",
        );
        assert!(
            slug.len() <= DeviceEntity::MAX_SLUG_LEN,
            "{slug:?} too long"
        );
    }
}

/// Two entities sharing a slug share a topic *and* a discovery topic, and the
/// second silently overwrites the first.
#[test]
fn slugs_are_distinct() {
    let mut seen: Vec<&str> = DeviceEntity::ALL.iter().map(|e| e.slug()).collect();
    let count = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), count, "two device entities share a slug");
}

/// R4: two entities sharing a `unique_id` is a configuration Home Assistant
/// rejects outright. The dangerous collision is not between two diagnostics —
/// it is between a diagnostic and a shade, because the two identifiers are
/// built by different functions and nothing but this checks them together.
#[test]
fn device_unique_ids_collide_with_nothing_a_shade_can_produce() {
    let device = DeviceId::new(DEVICE).unwrap();
    let mut ids: Vec<String> = DeviceEntity::ALL
        .iter()
        .map(|e| UniqueId::for_device(&device, *e).as_str().to_owned())
        .collect();
    let device_count = ids.len();

    for id in 0u8..=255 {
        for component in Component::ALL {
            ids.push(
                UniqueId::for_shade(&device, component, ShadeId(id))
                    .as_str()
                    .to_owned(),
            );
        }
    }
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "a device unique_id collides with a shade's ({device_count} device entities)",
    );
}

/// R4 again: the identity must survive a rename, a change of either namespace,
/// and a firmware update. A diagnostic has no name to rename, so what is tested
/// is the namespaces — the two values an operator is most likely to change.
#[test]
fn a_device_unique_id_follows_neither_namespace() {
    for entity in DeviceEntity::ALL {
        let a = render(&default_config(), entity);
        let b = render(&config("elsewhere", "homeassistant"), entity);
        let c = render(&default_config_with_prefix("ha/discovery"), entity);
        assert_eq!(a["unique_id"], b["unique_id"], "{entity:?}");
        assert_eq!(a["unique_id"], c["unique_id"], "{entity:?}");
    }
}

fn default_config_with_prefix(prefix: &str) -> MqttConfig {
    config("somfyrs", prefix)
}

// ---------------------------------------------------------------------------
// The topics
// ---------------------------------------------------------------------------

/// Pinned by value, because these are the addresses a dashboard or a broker
/// subscription is written against.
#[test]
fn device_topics_are_exact() {
    let cfg = default_config();
    assert_eq!(cfg.device_base().as_str(), "somfyrs/device");
    for (entity, expected) in [
        (DeviceEntity::Uptime, "somfyrs/device/uptime"),
        (DeviceEntity::WifiSignal, "somfyrs/device/wifi_signal"),
        (DeviceEntity::HeapFree, "somfyrs/device/heap_free"),
        (DeviceEntity::HeapPeak, "somfyrs/device/heap_peak"),
        (
            DeviceEntity::RollcodeDamaged,
            "somfyrs/device/rollcode_damaged",
        ),
    ] {
        assert_eq!(cfg.device_topic(entity).as_str(), expected, "{entity:?}");
    }
}

/// The device namespace must not be reachable as a shade's, or a shade with the
/// right id would publish over a diagnostic.
#[test]
fn the_device_namespace_cannot_collide_with_a_shade_or_with_availability() {
    let cfg = default_config();
    let mut occupied: Vec<String> = vec![cfg.availability_topic().as_str().to_owned()];
    for id in 0u8..=255 {
        for (_, topic) in cfg.shade_topics(ShadeId(id), true) {
            occupied.push(topic.as_str().to_owned());
        }
        occupied.push(cfg.shade_base(ShadeId(id)).as_str().to_owned());
    }
    for entity in DeviceEntity::ALL {
        let topic = cfg.device_topic(entity).as_str().to_owned();
        assert!(
            !occupied.contains(&topic),
            "{entity:?} publishes to {topic}, which something else owns",
        );
    }
}

/// The contract verified against a live Home Assistant, restated for the
/// entities that are not covers: the component is the segment immediately after
/// the prefix, whatever the component is.
#[test]
fn a_diagnostic_discovery_topic_puts_its_component_immediately_after_the_prefix() {
    let cfg = config("somfyrs", "ha/discovery");
    for entity in DeviceEntity::ALL {
        let object = ObjectId::for_device(entity);
        let topic = cfg.discovery_topic(entity.component(), &object);
        let segments: Vec<&str> = topic.as_str().split('/').collect();
        assert_eq!(
            segments,
            [
                "ha",
                "discovery",
                entity.component().as_str(),
                NODE,
                entity.slug(),
                "config",
            ],
            "{entity:?}",
        );
    }
}

/// No topic this crate builds may contain an empty segment — the failure that
/// produced `homeassistant//cover/…` in the field.
#[test]
fn no_device_topic_has_an_empty_segment() {
    for root in ["somfyrs", "a", "home/blinds"] {
        let cfg = config(root, "homeassistant");
        for entity in DeviceEntity::ALL {
            for topic in [
                cfg.device_topic(entity).as_str().to_owned(),
                cfg.discovery_topic(entity.component(), &ObjectId::for_device(entity))
                    .as_str()
                    .to_owned(),
            ] {
                assert!(!topic.contains("//"), "{topic:?}");
                assert!(!topic.starts_with('/'), "{topic:?}");
                assert!(!topic.ends_with('/'), "{topic:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The payloads
// ---------------------------------------------------------------------------

/// `entity_category: diagnostic` is what keeps five sensors out of the main
/// dashboard. Without it every one of them lands on the device's primary card
/// beside the covers, which is the clutter R7 is not asking for.
#[test]
fn every_diagnostic_is_categorised_as_one() {
    let cfg = default_config();
    for entity in DeviceEntity::ALL {
        let payload = render(&cfg, entity);
        assert_eq!(
            payload["entity_category"].as_str(),
            Some("diagnostic"),
            "{entity:?}",
        );
    }
}

/// The fields Home Assistant needs to render a number as something other than a
/// bare integer. `device_class` and `unit_of_measurement` are what turn `-58`
/// into `-58 dBm` with the right icon and the right graph.
#[test]
fn every_diagnostic_carries_the_fields_that_make_it_readable() {
    let cfg = default_config();
    for entity in DeviceEntity::ALL {
        let payload = render(&cfg, entity);
        assert_eq!(
            payload["state_topic"].as_str(),
            Some(format!("~/{}", entity.slug()).as_str()),
            "{entity:?}",
        );
        assert_eq!(payload["name"].as_str(), Some(entity.label()), "{entity:?}");
        assert_eq!(
            payload["device_class"].as_str(),
            entity.device_class(),
            "{entity:?}",
        );
        assert_eq!(
            payload["unit_of_measurement"].as_str(),
            entity.unit(),
            "{entity:?}",
        );
        assert_eq!(
            payload["state_class"].as_str(),
            entity.state_class(),
            "{entity:?}",
        );
    }
}

/// An absent attribute is absent, not `null`. Home Assistant treats an explicit
/// `null` as a set value in several places, and a `"unit_of_measurement": null`
/// on a sensor that has no unit is a difference worth not discovering later.
#[test]
fn an_entity_without_a_unit_omits_the_key_rather_than_writing_null() {
    let cfg = default_config();
    let without: Vec<DeviceEntity> = DeviceEntity::ALL
        .into_iter()
        .filter(|e| e.unit().is_none())
        .collect();
    assert!(
        !without.is_empty(),
        "this test needs at least one unitless entity to be meaningful",
    );
    for entity in without {
        let payload = render(&cfg, entity);
        let object = payload.as_object().unwrap();
        assert!(
            !object.contains_key("unit_of_measurement"),
            "{entity:?} wrote a null unit",
        );
    }
}

/// The payload's `object_id` is device-scoped, and the discovery topic's
/// segment of the same name is not.
///
/// **They are different things and only one of them is inert.** Home Assistant
/// accepts the topic segment and ignores it, but the payload key is *"used
/// instead of `name` for automatic generation of `entity_id`"* — so a bare slug
/// there claims `sensor.uptime` on the whole installation, and the second
/// somfy-rs board on an estate gets `sensor.uptime_2`. That is the collision the
/// `device` block was added to prevent, one layer down, at the identifier
/// automations and dashboard cards actually point at.
#[test]
fn the_payload_object_id_is_device_scoped_and_the_topic_segment_is_not() {
    let cfg = default_config();
    for entity in DeviceEntity::ALL {
        // The address stays short and stable.
        assert_eq!(ObjectId::for_device(entity).as_str(), entity.slug());
        // The entity id does not collide with another controller's.
        assert_eq!(
            render(&cfg, entity)["object_id"].as_str(),
            Some(format!("{DEVICE}_{}", entity.slug()).as_str()),
            "{entity:?}",
        );
    }

    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(ShadeId(1), "Lounge", false)
        .render(&mut buf)
        .unwrap();
    let cover: serde_json::Value = serde_json::from_str(&buf).unwrap();
    assert_eq!(
        cover["object_id"].as_str(),
        Some(format!("{DEVICE}_shade_1").as_str()),
    );

    // The property, rather than the format: two controllers propose no entity
    // id in common. This is what would fail if the device id were ever dropped
    // back out of the key.
    let other = MqttConfig::new(
        DiscoveryPrefix::new("homeassistant").unwrap(),
        StateRoot::new("somfyrs").unwrap(),
        NodeId::new(NODE).unwrap(),
        DeviceId::new("0f0f0f0f").unwrap(),
    )
    .unwrap();
    for entity in DeviceEntity::ALL {
        assert_ne!(
            render(&cfg, entity)["object_id"],
            render(&other, entity)["object_id"],
            "{entity:?} proposes the same entity id on two controllers",
        );
    }
}

/// Every entity this device publishes belongs to one device in Home Assistant.
///
/// Without the `device` block the diagnostics appear as loose entities with no
/// device to group them under, which on an estate with more than one controller
/// is five sensors called "Uptime" and no way to tell which board each belongs
/// to. Every field in the block is a fact the firmware already holds — the
/// identifier is the same stable `device_id` every `unique_id` is built from.
#[test]
fn every_payload_names_the_same_device() {
    let cfg = default_config();
    let mut blocks: Vec<serde_json::Value> = DeviceEntity::ALL
        .iter()
        .map(|e| render(&cfg, *e)["device"].clone())
        .collect();

    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(ShadeId(1), "Lounge", false)
        .render(&mut buf)
        .unwrap();
    let cover: serde_json::Value = serde_json::from_str(&buf).unwrap();
    blocks.push(cover["device"].clone());

    for block in &blocks {
        assert_eq!(
            block["identifiers"].as_array().map(Vec::as_slice),
            Some([serde_json::Value::String(DEVICE.to_owned())].as_slice()),
            "{block:?}",
        );
        assert!(
            block["name"].as_str().is_some_and(|n| n.contains(DEVICE)),
            "the device name must distinguish one board from another: {block:?}",
        );
    }
    assert!(
        blocks.windows(2).all(|w| w[0] == w[1]),
        "the cover and the diagnostics must name the same device: {blocks:?}",
    );
}
