//! Acceptance criterion 3 — every topic referenced in a discovery payload,
//! after `~` expansion, is a topic the firmware actually publishes to or
//! subscribes to.
//!
//! ## Why this test is shaped the way it is
//!
//! The field failure was that the payload and the publisher disagreed: the
//! payload said `"~": "/shades/1"` while the device published to `shades/1`.
//! Both halves were individually plausible; nothing compared them.
//!
//! So both halves here come from one source of truth — [`ShadeTopic`], which
//! owns each topic's segments, its role, and the payload key that carries it.
//! `MqttConfig::shade_topics` builds the absolute topics from it, and the
//! payload renderer builds the `~`-relative strings from it. Neither side can
//! be edited without moving the other.
//!
//! One source of truth alone would make this test a tautology, so it is read
//! back out of **rendered payload bytes** with a real JSON parser rather than
//! out of the struct that produced them, and
//! [`the_check_actually_catches_the_leading_slash_bug`] feeds it the observed
//! broken payload and asserts it is rejected. A check that cannot fail proves
//! nothing.

use std::collections::BTreeMap;

use serde_json::Value;
use somfy_domain::ShadeId;
use somfy_mqtt::{
    DeviceEntity, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ShadeTopic, StateRoot, TopicRole,
    PAYLOAD_CAPACITY,
};

fn config(state_root: &str) -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new("homeassistant").unwrap(),
        StateRoot::new(state_root).unwrap(),
        NodeId::new("somfyrs").unwrap(),
        DeviceId::new("a1b2c3d4").unwrap(),
    )
    .unwrap()
}

fn render(cfg: &MqttConfig, shade: ShadeId, name: &str, has_tilt: bool) -> Value {
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.cover_discovery(shade, name, has_tilt)
        .render(&mut buf)
        .expect("payload fits");
    serde_json::from_str(&buf).expect("rendered payload is valid JSON")
}

/// What the firmware will do with each absolute topic, derived from the same
/// table the payload is derived from.
fn firmware_topics(
    cfg: &MqttConfig,
    shade: ShadeId,
    has_tilt: bool,
) -> BTreeMap<String, TopicRole> {
    let mut map = BTreeMap::new();
    for (topic, absolute) in cfg.shade_topics(shade, has_tilt) {
        map.insert(absolute.as_str().to_owned(), topic.role());
    }
    map.insert(
        cfg.availability_topic().as_str().to_owned(),
        TopicRole::Published,
    );
    map
}

/// Home Assistant's `~` expansion, applied exactly as HA applies it: a value
/// beginning with `~` has that character replaced by the payload's `~`.
fn expand(base: &str, value: &str) -> String {
    match value.strip_prefix('~') {
        Some(rest) => format!("{base}{rest}"),
        None => value.to_owned(),
    }
}

/// The round trip itself, as a fallible function so a deliberately broken
/// payload can be fed to it and the failure asserted.
fn round_trip(payload: &Value, firmware: &BTreeMap<String, TopicRole>) -> Result<usize, String> {
    let object = payload.as_object().ok_or("payload is not an object")?;
    let base = object
        .get("~")
        .and_then(Value::as_str)
        .ok_or("payload has no `~`")?;

    let mut checked = 0;
    for (key, value) in object {
        if key != "~" && !key.ends_with("_topic") {
            continue;
        }
        let raw = value
            .as_str()
            .ok_or_else(|| format!("{key} is not a string"))?;
        let absolute = expand(base, raw);

        // `~` is a base, not a topic in its own right: it must be a strict
        // prefix of the topics built from it. A leading slash on either side
        // breaks that and is exactly the observed failure.
        if key == "~" {
            if !firmware
                .keys()
                .any(|t| t.starts_with(&format!("{absolute}/")))
            {
                return Err(format!(
                    "`~` = {absolute:?} is not a prefix of any firmware topic"
                ));
            }
            checked += 1;
            continue;
        }

        let role = firmware.get(&absolute).ok_or_else(|| {
            format!("{key} resolves to {absolute:?}, which the firmware neither publishes nor subscribes to")
        })?;

        // A command topic the firmware only publishes to, or a state topic it
        // only subscribes to, is a payload that type-checks and does nothing.
        let expected =
            expected_role(key).ok_or_else(|| format!("{key} has no role in the topic table"))?;
        if *role != expected {
            return Err(format!(
                "{key} resolves to {absolute:?} with role {role:?}, expected {expected:?}"
            ));
        }
        checked += 1;
    }
    Ok(checked)
}

/// The payload key -> role mapping, read back out of the same table that
/// produced the payload rather than restated here.
///
/// A diagnostic's `state_topic` resolves through [`ShadeTopic::State`], which
/// carries the same key. That is the right answer and not a coincidence — the
/// key names a direction, and a `state_topic` is read by Home Assistant
/// whatever kind of entity carries it — but it is stated here because the two
/// payloads are built by different renderers and nothing else says so.
fn expected_role(key: &str) -> Option<TopicRole> {
    if key == "availability_topic" {
        return Some(TopicRole::Published);
    }
    ShadeTopic::ALL
        .iter()
        .find(|t| t.payload_key() == Some(key))
        .map(|t| t.role())
}

#[test]
fn every_payload_topic_is_a_firmware_topic() {
    for (root, has_tilt) in [
        ("somfyrs", false),
        ("somfyrs", true),
        ("home/blinds", true),
        ("a", false),
    ] {
        let cfg = config(root);
        let shade = ShadeId(1);
        let payload = render(&cfg, shade, "Lounge", has_tilt);
        let firmware = firmware_topics(&cfg, shade, has_tilt);

        let checked = round_trip(&payload, &firmware)
            .unwrap_or_else(|e| panic!("root {root:?} tilt {has_tilt}: {e}"));
        // `~` plus availability plus every keyed shade topic.
        let expected = 2 + ShadeTopic::for_shade(has_tilt)
            .filter(|t| t.payload_key().is_some())
            .count();
        assert_eq!(checked, expected, "root {root:?} tilt {has_tilt}");
    }
}

/// The other direction: every topic the table says carries a payload key must
/// actually appear in the rendered payload. Omitting one leaves Home Assistant
/// without a control the firmware is publishing for.
#[test]
fn every_keyed_firmware_topic_appears_in_the_payload() {
    for has_tilt in [false, true] {
        let cfg = config("somfyrs");
        let payload = render(&cfg, ShadeId(1), "Lounge", has_tilt);
        let object = payload.as_object().unwrap();

        for topic in ShadeTopic::for_shade(has_tilt) {
            let Some(key) = topic.payload_key() else {
                continue;
            };
            assert!(
                object.contains_key(key),
                "tilt {has_tilt}: payload is missing {key} for {topic:?}",
            );
        }
    }
}

/// R8 in the payload: a non-tilt shade must not carry tilt keys at all.
#[test]
fn non_tilt_shades_carry_no_tilt_keys() {
    let cfg = config("somfyrs");
    let payload = render(&cfg, ShadeId(1), "Lounge", false);
    let object = payload.as_object().unwrap();

    assert!(!object.contains_key("tilt_status_topic"));
    assert!(!object.contains_key("tilt_command_topic"));
}

/// This is the whole point. Feed the round trip the payload shape that was
/// actually observed in the field — a `~` with a leading slash, while the
/// publisher writes without one — and assert it is rejected.
///
/// Without this, `every_payload_topic_is_a_firmware_topic` could be passing
/// because it compares a thing with itself.
#[test]
fn the_check_actually_catches_the_leading_slash_bug() {
    let cfg = config("somfyrs");
    let shade = ShadeId(1);
    let firmware = firmware_topics(&cfg, shade, true);
    let mut payload = render(&cfg, shade, "Lounge", true);

    let base = payload["~"].as_str().unwrap().to_owned();
    payload["~"] = Value::String(format!("/{base}"));

    let err = round_trip(&payload, &firmware)
        .expect_err("a leading-slash `~` must be rejected, not tolerated");
    assert!(err.contains('/'), "unhelpful error: {err}");
}

/// The second observed shape: a payload whose availability topic sits under the
/// discovery prefix. `homeassistant/status` is Home Assistant's own birth and
/// will topic, so the device would be marked available by HA's own birth
/// message while it is offline.
#[test]
fn the_check_actually_catches_availability_under_the_discovery_prefix() {
    let cfg = config("somfyrs");
    let shade = ShadeId(1);
    let firmware = firmware_topics(&cfg, shade, true);
    let mut payload = render(&cfg, shade, "Lounge", true);

    payload["availability_topic"] = Value::String("homeassistant/status".to_owned());

    let err = round_trip(&payload, &firmware)
        .expect_err("availability under the discovery prefix must be rejected");
    assert!(
        err.contains("homeassistant/status"),
        "unhelpful error: {err}"
    );
}

/// A payload key pointing at a topic the firmware never touches — the third way
/// the two halves can drift apart.
#[test]
fn the_check_actually_catches_a_topic_nobody_publishes() {
    let cfg = config("somfyrs");
    let shade = ShadeId(1);
    let firmware = firmware_topics(&cfg, shade, true);
    let mut payload = render(&cfg, shade, "Lounge", true);

    payload["position_topic"] = Value::String("~/pos".to_owned());

    let err = round_trip(&payload, &firmware).expect_err("an unpublished topic must be rejected");
    assert!(
        err.contains("somfyrs/shades/1/pos"),
        "unhelpful error: {err}"
    );
}

/// A user-supplied name goes into the payload as JSON text, so it must be
/// escaped. An unescaped quote or backslash makes the payload unparseable and
/// the entity never appears — with no error anywhere.
#[test]
fn hostile_names_still_render_parseable_json() {
    let cfg = config("somfyrs");
    for name in [
        r#"Salon "grand" / Porte-fenêtre"#,
        r"back\slash",
        "new\nline\ttab",
        "\u{1}control",
        "日本語 🪟",
        "",
    ] {
        let payload = render(&cfg, ShadeId(2), name, true);
        assert_eq!(
            payload["name"].as_str(),
            Some(name),
            "name {name:?} did not survive"
        );
        let firmware = firmware_topics(&cfg, ShadeId(2), true);
        round_trip(&payload, &firmware).unwrap_or_else(|e| panic!("name {name:?}: {e}"));
    }
}

// ---------------------------------------------------------------------------
// The same check for the entities R7 adds
// ---------------------------------------------------------------------------

/// The device-level half of acceptance criterion 3. A diagnostic's payload
/// names two topics — its `~` and its `state_topic` — and both must resolve to
/// somewhere the firmware actually publishes.
///
/// This is the check that would catch the R7 version of the field failure: five
/// sensors whose configs are perfect and whose `state_topic` points one segment
/// away from where the readings go, which appears in Home Assistant as five
/// entities that are permanently unknown.
#[test]
fn every_diagnostic_payload_topic_is_a_firmware_topic() {
    for root in ["somfyrs", "home/blinds", "a"] {
        let cfg = config(root);
        let firmware = device_topics(&cfg);
        for entity in DeviceEntity::ALL {
            let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
            cfg.diagnostic_discovery(entity)
                .render(&mut buf)
                .expect("payload fits");
            let payload: Value = serde_json::from_str(&buf).expect("valid JSON");

            let checked = round_trip(&payload, &firmware)
                .unwrap_or_else(|e| panic!("root {root:?} {entity:?}: {e}"));
            // `~`, availability, and the state topic.
            assert_eq!(checked, 3, "root {root:?} {entity:?}");
        }
    }
}

/// The absolute topics the firmware publishes for the device itself, derived
/// from the same table the payloads are.
fn device_topics(cfg: &MqttConfig) -> BTreeMap<String, TopicRole> {
    let mut map = BTreeMap::new();
    for (_, topic) in cfg.device_topics() {
        map.insert(topic.as_str().to_owned(), TopicRole::Published);
    }
    map.insert(
        cfg.availability_topic().as_str().to_owned(),
        TopicRole::Published,
    );
    map
}

/// The same demonstration the cover half carries: a check that cannot fail
/// proves nothing. Move a diagnostic's `state_topic` one segment and assert the
/// round trip rejects it.
#[test]
fn the_diagnostic_check_actually_catches_a_reading_nobody_publishes() {
    let cfg = config("somfyrs");
    let firmware = device_topics(&cfg);
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.diagnostic_discovery(DeviceEntity::Uptime)
        .render(&mut buf)
        .unwrap();
    let mut payload: Value = serde_json::from_str(&buf).unwrap();

    payload["state_topic"] = Value::String("~/uptime_seconds".to_owned());

    let err = round_trip(&payload, &firmware).expect_err("an unpublished reading must be rejected");
    assert!(
        err.contains("somfyrs/device/uptime_seconds"),
        "unhelpful error: {err}"
    );
}

/// R4: `unique_id` must be stable across reboots, config changes and firmware
/// updates. It therefore cannot be derived from anything a user can edit — a
/// rename must not create a second entity.
#[test]
fn unique_id_survives_a_rename_and_a_root_change() {
    let a = render(&config("somfyrs"), ShadeId(4), "Lounge", true);
    let b = render(&config("somfyrs"), ShadeId(4), "Sitting room", true);
    let c = render(&config("home/blinds"), ShadeId(4), "Lounge", true);

    assert_eq!(a["unique_id"], b["unique_id"]);
    assert_eq!(a["unique_id"], c["unique_id"]);
    // Different shades must not collide.
    let d = render(&config("somfyrs"), ShadeId(5), "Lounge", true);
    assert_ne!(a["unique_id"], d["unique_id"]);
}
