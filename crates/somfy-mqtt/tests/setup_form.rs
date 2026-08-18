//! The add-a-shade form: its identity, its lifecycle, and the flow behind it.
//!
//! Four things are checked here, and the second is the one the whole design
//! turns on:
//!
//! 1. **Identity.** A form entity cannot collide with a shade's or a
//!    diagnostic's, in either the discovery topic or the `unique_id` — checked
//!    against all 256 shade ids and every other entity rather than argued from
//!    four string literals.
//! 2. **A discard leaves nothing behind.** Every topic an open retains is a
//!    topic a close clears. This is R5 for the form, and it is checked as a
//!    property of the two plans rather than as two lists that happen to agree.
//! 3. **The payloads say what Home Assistant requires.** Each component's
//!    required keys, read out of *rendered bytes* rather than out of the struct
//!    that produced them, with the schema citations in `docs/provenance.md`.
//! 4. **The flow.** Press by press, including every refusal.

use serde_json::Value;
use somfy_domain::{ShadeId, ShadeKind};
use somfy_mqtt::{
    Ask, Component, DeviceEntity, DeviceId, DiscoveryPrefix, Effect, FormChange, MqttConfig,
    NodeId, ObjectId, Pairing, Payload, Retention, Setup, SetupEntity, SetupInput, SetupMessage,
    SetupPhase, SetupValue, StateRoot, Step, UniqueId, KIND_OPTIONS, MAX_DRAFT_NAME_LEN,
    MAX_MESSAGE_LEN, MAX_STATE_LEN, MAX_TRAVEL_MS, MIN_TRAVEL_MS, PAYLOAD_CAPACITY, PAYLOAD_PRESS,
    TRAVEL_STEP_MS,
};

const NODE: &str = "somfyrs";
const DEVICE: &str = "a1b2c3d4";

fn config() -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new("homeassistant").unwrap(),
        StateRoot::new("somfyrs").unwrap(),
        NodeId::new(NODE).unwrap(),
        DeviceId::new(DEVICE).unwrap(),
    )
    .unwrap()
}

fn render(cfg: &MqttConfig, entity: SetupEntity) -> Value {
    let mut buf: heapless::String<PAYLOAD_CAPACITY> = heapless::String::new();
    cfg.setup_discovery(entity)
        .render(&mut buf)
        .expect("a setup payload must fit");
    serde_json::from_str(&buf).expect("valid JSON")
}

/// Resolve a payload topic against the payload's own `~`, exactly as Home
/// Assistant does.
fn expand(payload: &Value, key: &str) -> Option<String> {
    let base = payload["~"].as_str().expect("every payload carries a base");
    let raw = payload.get(key)?.as_str()?;
    Some(match raw.strip_prefix('~') {
        Some(rest) => format!("{base}{rest}"),
        None => raw.to_string(),
    })
}

/// The topics a plan publishes something **retained and non-empty** to — that
/// is, the ones that would outlive the device if nobody cleared them.
fn retained_by(steps: impl Iterator<Item = Step<'static>>) -> Vec<String> {
    steps
        .filter_map(|step| match step {
            Step::Send(publish)
                if publish.retention() == Retention::Retained
                    && !matches!(publish.payload(), Payload::Nothing) =>
            {
                Some(publish.topic().as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

/// The topics a plan clears with a zero-length retained publish.
fn cleared_by(steps: impl Iterator<Item = Step<'static>>) -> Vec<String> {
    steps
        .filter_map(|step| match step {
            Step::Send(publish)
                if publish.retention() == Retention::Retained
                    && matches!(publish.payload(), Payload::Nothing) =>
            {
                Some(publish.topic().as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1 — identity
// ---------------------------------------------------------------------------

/// A form entity's object id cannot be any shade's or any diagnostic's.
///
/// The consequence of a collision is not cosmetic: two entities sharing a
/// discovery topic means the second silently overwrites the first's retained
/// config, and two sharing a `unique_id` is a configuration Home Assistant
/// rejects outright. This is the wall the earlier per-shade design hit, checked
/// from the device level where it is supposed not to exist.
#[test]
fn a_form_entity_cannot_collide_with_a_shade_or_a_diagnostic() {
    let mut seen: Vec<String> = Vec::new();

    for entity in SetupEntity::ALL {
        let id = ObjectId::for_setup(entity).as_str().to_string();
        assert!(
            id.starts_with("setup_"),
            "{id} does not carry the prefix that keeps it out of the other two sets",
        );
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "{id} escaped R2's character class",
        );
        assert!(!seen.contains(&id), "{id} repeated within the form");
        seen.push(id);
    }

    for id in 0u8..=255 {
        let shade = ObjectId::for_shade(ShadeId(id)).as_str().to_string();
        assert!(
            !seen.contains(&shade),
            "{shade} collides with a form entity"
        );
    }
    for entity in DeviceEntity::ALL {
        let diagnostic = ObjectId::for_device(entity).as_str().to_string();
        assert!(
            !seen.contains(&diagnostic),
            "{diagnostic} collides with a form entity",
        );
    }
}

/// The same, one layer down, where Home Assistant actually keys its entities.
#[test]
fn every_unique_id_in_the_installation_is_distinct() {
    let device = DeviceId::new(DEVICE).unwrap();
    let mut seen: Vec<String> = Vec::new();

    for entity in SetupEntity::ALL {
        seen.push(UniqueId::for_setup(&device, entity).as_str().to_string());
    }
    for entity in DeviceEntity::ALL {
        seen.push(UniqueId::for_device(&device, entity).as_str().to_string());
    }
    for id in 0u8..=255 {
        for component in Component::ALL {
            seen.push(
                UniqueId::for_shade(&device, component, ShadeId(id))
                    .as_str()
                    .to_string(),
            );
        }
    }

    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seen.len(),
        "two entities share a unique_id, which Home Assistant rejects outright",
    );
}

/// A form topic cannot address what a shade, a diagnostic or availability owns.
#[test]
fn the_setup_namespace_does_not_overlap_the_others() {
    let cfg = config();
    let mut others: Vec<String> = vec![cfg.availability_topic().as_str().to_string()];
    for id in 0u8..=255 {
        others.push(cfg.shade_base(ShadeId(id)).as_str().to_string());
        for (_, topic) in cfg.shade_topics(ShadeId(id), true) {
            others.push(topic.as_str().to_string());
        }
    }
    for (_, topic) in cfg.device_topics() {
        others.push(topic.as_str().to_string());
    }

    for (entity, state, command) in cfg.setup_topics() {
        for topic in state.iter().chain(command.iter()) {
            let text = topic.as_str();
            assert!(
                !others.iter().any(|other| other == text),
                "{entity:?}'s topic {text} collides with another namespace",
            );
            assert!(!text.contains("//"), "{text} has an empty segment");
            assert!(!text.starts_with('/'), "{text} has a leading slash");
        }
    }
}

// ---------------------------------------------------------------------------
// 2 — a discard leaves nothing behind (R5)
// ---------------------------------------------------------------------------

/// **The property the whole design rests on.** Everything an open publishes
/// retained, a close clears.
///
/// Checked against the plans rather than against `SetupEntity::FORM`, so it
/// keeps holding if one of the two halves ever stops reading that array.
#[test]
fn closing_the_form_clears_every_topic_opening_it_retains() {
    let cfg = config();
    let retained = retained_by(cfg.open_form());
    let cleared = cleared_by(cfg.close_form());

    assert!(
        !retained.is_empty(),
        "an open that retains nothing is a bug"
    );
    for topic in &retained {
        assert!(
            cleared.contains(topic),
            "opening the form retains {topic} and closing it does not clear it — \
             that is an orphaned retained config, which is exactly what R5 is about",
        );
    }
}

/// The values too, not only the configs.
///
/// The evidence behind R5 is 49 retained topics deleted by hand, and most of
/// them were state rather than discovery. A close must clear every value the
/// form *could* have published, whether or not it did.
#[test]
fn closing_the_form_clears_every_value_topic_it_could_own() {
    let cfg = config();
    let cleared = cleared_by(cfg.close_form());
    for entity in SetupEntity::FORM {
        if !entity.has_state() {
            continue;
        }
        let topic = cfg.setup_topic(entity).as_str().to_string();
        assert!(
            cleared.contains(&topic),
            "{entity:?}'s value topic {topic} survives a discard",
        );
    }
}

/// A close leaves `Add shade` alone, and a whole-configuration retirement does
/// not.
///
/// The two are different events: one ends a setup and leaves the way to start
/// another, the other abandons the namespace entirely. Conflating them would
/// either strand `Add shade` on a broker nobody publishes to again, or remove
/// the only way to start a setup every time one finished.
#[test]
fn a_close_keeps_the_way_in_and_a_retirement_does_not() {
    let cfg = config();
    let begin = cfg
        .discovery_topic(
            SetupEntity::Begin.component(),
            &ObjectId::for_setup(SetupEntity::Begin),
        )
        .as_str()
        .to_string();

    assert!(
        !cleared_by(cfg.close_form()).contains(&begin),
        "closing a setup must not remove the button that starts the next one",
    );
    assert!(
        cleared_by(cfg.retire_setup()).contains(&begin),
        "abandoning the configuration must remove the button too",
    );
}

/// And the whole-configuration retirement clears everything either half of the
/// form can put on a broker.
#[test]
fn retiring_the_configuration_clears_the_whole_form() {
    let cfg = config();
    let put_out: Vec<String> = retained_by(cfg.announce_setup())
        .into_iter()
        .chain(retained_by(cfg.open_form()))
        .chain(
            // Values are published by the firmware rather than by a plan, so
            // they are named from the table the firmware publishes from.
            SetupEntity::ALL
                .into_iter()
                .filter(|entity| entity.has_state())
                .map(|entity| cfg.setup_topic(entity).as_str().to_string()),
        )
        .collect();

    let cleared = cleared_by(cfg.retire(&[ShadeId(1)]));
    for topic in put_out {
        assert!(
            cleared.contains(&topic),
            "{topic} is published under this configuration and survives its retirement",
        );
    }
}

// ---------------------------------------------------------------------------
// R6 — the form's commands are never retained
// ---------------------------------------------------------------------------

/// A retained `PRESS` on `setup/begin/set` would start a setup on every
/// reconnect; on `setup/send/set` it would **create a shade on every
/// reconnect**, each with its own allocated address and its own rolling-code
/// seed. That is the worst instance of R6 anywhere in this crate.
#[test]
fn no_plan_ever_publishes_to_a_form_command_topic() {
    let cfg = config();
    let commands: Vec<String> = SetupEntity::ALL
        .into_iter()
        .filter(|entity| entity.accepts_command())
        .map(|entity| cfg.setup_command_topic(entity).as_str().to_string())
        .collect();

    let published: Vec<String> = [
        cfg.announce(&[ShadeId(1)], false, |_| Pairing::Offered)
            .collect::<Vec<_>>(),
        cfg.open_form().collect(),
        cfg.close_form().collect(),
        cfg.retire(&[ShadeId(1)]).collect(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|step| match step {
        Step::Send(publish) => Some(publish.topic().as_str().to_string()),
        Step::Listen(_) => None,
    })
    .collect();

    for command in commands {
        assert!(
            !published.contains(&command),
            "{command} is a command topic and something published to it",
        );
    }
}

/// And the subscriptions suppress a retained replay the broker may already
/// hold — left by an earlier integration, or by a `mosquitto_pub -r` during
/// debugging. That is the half of R6 a publisher cannot fix.
#[test]
fn every_form_subscription_refuses_a_retained_replay() {
    let cfg = config();
    let mut found = 0;
    for step in cfg.announce_setup() {
        let Step::Listen(listen) = step else {
            continue;
        };
        assert!(
            !listen.retained_replay(),
            "{} would replay a retained command on every reconnect",
            listen.topic(),
        );
        found += 1;
    }
    assert_eq!(
        found,
        SetupEntity::ALL
            .iter()
            .filter(|entity| entity.accepts_command())
            .count(),
        "every form command topic must be subscribed",
    );
}

// ---------------------------------------------------------------------------
// 3 — the payloads Home Assistant will accept
// ---------------------------------------------------------------------------

/// Every payload's topics resolve to somewhere the firmware actually acts.
///
/// Read out of *rendered bytes*, which is the point: this is the check that
/// would have caught the leading-slash bug, where the payload and the publisher
/// disagreed and nothing noticed.
#[test]
fn every_topic_a_setup_payload_names_is_one_the_device_acts_on() {
    let cfg = config();
    for entity in SetupEntity::ALL {
        let payload = render(&cfg, entity);

        assert_eq!(
            payload["~"].as_str(),
            Some(cfg.setup_base().as_str()),
            "{entity:?} does not sit under the setup base",
        );
        assert_eq!(
            payload["availability_topic"].as_str(),
            Some(cfg.availability_topic().as_str()),
            "{entity:?}'s availability must be under the state root, never the prefix",
        );

        match expand(&payload, "command_topic") {
            Some(topic) => {
                assert!(entity.accepts_command(), "{entity:?} takes no commands");
                assert_eq!(topic, cfg.setup_command_topic(entity).as_str());
            }
            None => assert!(
                !entity.accepts_command(),
                "{entity:?} takes commands and its payload names no command topic — \
                 Home Assistant requires one for every component here but `sensor`",
            ),
        }

        match expand(&payload, "state_topic") {
            Some(topic) => {
                assert!(entity.has_state(), "{entity:?} publishes no state");
                assert_eq!(topic, cfg.setup_topic(entity).as_str());
            }
            None => assert!(!entity.has_state(), "{entity:?} has a state and no topic"),
        }
    }
}

/// The keys each component requires, and the values Home Assistant constrains.
///
/// Every assertion traces to a citation in `docs/provenance.md`. The failure
/// mode without them is the quiet one: a payload that fails `vol` validation
/// produces no entity, and a key Home Assistant does not recognise is dropped
/// by `extra=vol.REMOVE_EXTRA` without a word.
#[test]
fn each_component_carries_what_its_schema_requires() {
    let cfg = config();
    for entity in SetupEntity::ALL {
        let payload = render(&cfg, entity);

        // `MQTT_ENTITY_COMMON_SCHEMA`: `entity_category` is `vol.Coerce` over
        // `config` | `diagnostic`, so a `null` would be refused and take the
        // whole payload with it.
        assert_eq!(payload["entity_category"].as_str(), Some("config"));
        assert!(payload["unique_id"].as_str().is_some());
        assert!(payload["device"]["identifiers"].is_array());

        match entity.component() {
            // `MQTT_RW_SCHEMA` requires `command_topic` and nothing else.
            Component::Button => {
                assert!(payload.get("command_topic").is_some());
                // `payload_press` is matched rather than declared.
                assert!(payload.get("payload_press").is_none());
            }
            // `text.py`: `min <= max <= 255`.
            Component::Text => {
                let min = payload["min"].as_u64().expect("text carries a min");
                let max = payload["max"].as_u64().expect("text carries a max");
                assert!(min <= max);
                assert!(max <= MAX_STATE_LEN as u64);
                assert_eq!(max, MAX_DRAFT_NAME_LEN as u64);
                assert_eq!(payload["mode"].as_str(), Some("text"));
            }
            // `number.py`: `min <= max`, `step >= 1e-3`, and the defaults are
            // 0-100 — so a millisecond range must be stated or every value is
            // refused.
            Component::Number => {
                let min = payload["min"].as_u64().expect("number carries a min");
                let max = payload["max"].as_u64().expect("number carries a max");
                let step = payload["step"].as_u64().expect("number carries a step");
                assert_eq!(min, u64::from(MIN_TRAVEL_MS));
                assert_eq!(max, u64::from(MAX_TRAVEL_MS));
                assert_eq!(step, u64::from(TRAVEL_STEP_MS));
                assert!(min <= max);
                assert!(step >= 1);
                assert_eq!(payload["mode"].as_str(), Some("box"));
                assert_eq!(payload["unit_of_measurement"].as_str(), Some("ms"));
                // Omitted on purpose: a `device_class` Home Assistant disagreed
                // with would cost the entity, and the unit alone cannot be
                // rejected.
                assert!(payload.get("device_class").is_none());
            }
            // `select.py:57`: `options` is the one required key beyond
            // `command_topic`.
            Component::Select => {
                let options = payload["options"]
                    .as_array()
                    .expect("select carries an options list");
                assert_eq!(options.len(), KIND_OPTIONS.len());
                assert!(
                    !options.is_empty(),
                    "a select with no options picks nothing"
                );
                for option in options {
                    let text = option.as_str().expect("options are strings");
                    assert!(
                        somfy_mqtt::kind_from_label(text).is_some(),
                        "{text} is offered and cannot be read back",
                    );
                    // `select.py:130` treats `none` case-insensitively as "no
                    // option", so an option spelled that way could never be
                    // selected.
                    assert!(!text.eq_ignore_ascii_case("none"));
                }
            }
            // `MQTT_RO_SCHEMA` requires `state_topic` and forbids nothing else
            // we send. No `device_class`, so the state goes through
            // `check_state_too_long` rather than a parser.
            Component::Sensor => {
                assert!(payload.get("state_topic").is_some());
                assert!(payload.get("device_class").is_none());
                assert!(payload.get("state_class").is_none());
            }
            other => panic!("{entity:?} is a {other:?}, which has no schema check here"),
        }
    }
}

/// Every instruction fits the 255 characters Home Assistant will store.
///
/// Over the limit the entity goes to `unknown` and the sentence is **lost**,
/// not shortened (`util.py:377-396`) — so the compile-time assertion in
/// `setup.rs` is the real guard and this is its visible half.
#[test]
fn every_instruction_fits_home_assistants_state_limit() {
    for message in SetupMessage::ALL {
        let text = message.as_str();
        assert!(
            text.chars().count() <= MAX_STATE_LEN,
            "{message:?} is {} characters, over the {MAX_STATE_LEN} Home Assistant stores",
            text.chars().count(),
        );
        assert!(
            text.is_ascii(),
            "{message:?} is not ASCII, so the byte budget and the character limit have parted",
        );
        assert!(!text.is_empty());
    }
    const { assert!(MAX_MESSAGE_LEN <= MAX_STATE_LEN) };
}

/// The one message that has to earn its 255 characters actually says the thing.
///
/// Pinned because it is the entire reason this form is possible where an
/// entity-name design was not: the two-minute programming window, and the fact
/// that the remote which opens it is not this controller.
#[test]
fn the_first_step_says_what_no_entity_name_could() {
    let text = SetupMessage::Drafting.as_str();
    assert!(
        text.contains("PROG"),
        "the instruction must name the button"
    );
    assert!(text.contains("2 minutes"), "and the window it opens");
    assert!(
        text.contains("existing remote"),
        "and that the remote is not this controller",
    );
    assert!(
        text.contains("measure"),
        "and that the travel times are measured rather than guessed — the fault \
         this whole form exists to prevent",
    );
}

// ---------------------------------------------------------------------------
// 4 — the flow
// ---------------------------------------------------------------------------

fn opened() -> Setup {
    let mut setup = Setup::new();
    assert_eq!(
        setup.apply(SetupInput::Begin),
        Effect {
            form: FormChange::Open,
            ask: None,
        },
    );
    setup
}

/// Nothing exists until `Add shade`, and a stray press while idle does nothing
/// at all.
///
/// The silence is deliberate: the entity that would carry an answer does not
/// exist, so anything published would be a retained value under a config
/// nothing announced.
#[test]
fn an_idle_controller_answers_only_the_button_that_starts_a_setup() {
    let mut setup = Setup::new();
    assert_eq!(setup.phase(), SetupPhase::Idle);
    for input in [
        SetupInput::Send,
        SetupInput::Confirm,
        SetupInput::Discard,
        SetupInput::Name("Lounge"),
        SetupInput::TravelUp("10000"),
        SetupInput::Done,
    ] {
        assert_eq!(
            setup.apply(input),
            Effect {
                form: FormChange::Unchanged,
                ask: None,
            },
            "{input:?} did something while idle",
        );
        assert_eq!(setup.phase(), SetupPhase::Idle);
    }
}

/// **The press-by-press sequence**, from an empty controller to a confirmed
/// shade.
#[test]
fn the_whole_sequence_from_add_shade_to_it_moved() {
    let mut setup = Setup::new();

    // Add shade -> the form appears, empty.
    assert_eq!(setup.apply(SetupInput::Begin).form, FormChange::Open);
    assert_eq!(setup.phase(), SetupPhase::Drafting);
    assert_eq!(setup.message(), SetupMessage::Drafting);
    assert_eq!(setup.value(SetupEntity::Name), SetupValue::Unset);
    assert_eq!(setup.value(SetupEntity::TravelUp), SetupValue::Unset);
    assert_eq!(setup.value(SetupEntity::TravelDown), SetupValue::Unset);
    assert_eq!(setup.value(SetupEntity::Kind), SetupValue::Text("Roller"));

    // Send pairing, too early: refused with the reason, and nothing is asked
    // of the shade table.
    assert_eq!(setup.apply(SetupInput::Send).ask, None);
    assert_eq!(setup.message(), SetupMessage::NeedsName);

    setup.apply(SetupInput::Name("Lounge"));
    assert_eq!(setup.message(), SetupMessage::Drafting);
    assert_eq!(setup.apply(SetupInput::Send).ask, None);
    assert_eq!(setup.message(), SetupMessage::NeedsTimes);

    setup.apply(SetupInput::TravelUp("14200"));
    assert_eq!(setup.apply(SetupInput::Send).ask, None);
    assert_eq!(
        setup.message(),
        SetupMessage::NeedsTimes,
        "one of the two times is not both of them",
    );

    setup.apply(SetupInput::TravelDown("13800"));
    setup.apply(SetupInput::Kind("Shutter"));
    assert_eq!(setup.draft().kind(), ShadeKind::Shutter);
    assert_eq!(setup.draft().blocker(), None);

    // Send pairing, for real: create first.
    assert_eq!(
        setup.apply(SetupInput::Send),
        Effect {
            form: FormChange::Unchanged,
            ask: Some(Ask::Create),
        },
    );

    // The shade table answers with an id, and the flow pairs at once — one
    // press, not two, because the operator is holding PROG right now.
    assert_eq!(
        setup.apply(SetupInput::Created(ShadeId(3))),
        Effect {
            form: FormChange::Unchanged,
            ask: Some(Ask::Pair(ShadeId(3))),
        },
    );
    assert_eq!(
        setup.phase(),
        SetupPhase::AwaitingReport { shade: ShadeId(3) }
    );
    assert_eq!(setup.message(), SetupMessage::AwaitingReport);

    // The window was missed. Press again; it pairs again and creates nothing.
    assert_eq!(
        setup.apply(SetupInput::Send).ask,
        Some(Ask::Pair(ShadeId(3))),
    );

    // Timed it properly this time, and the correction reaches the shade that
    // already exists.
    assert_eq!(
        setup.apply(SetupInput::TravelUp("14700")).ask,
        Some(Ask::Amend(ShadeId(3))),
    );

    // It moved.
    assert_eq!(
        setup.apply(SetupInput::Confirm),
        Effect {
            form: FormChange::Unchanged,
            ask: Some(Ask::Confirm(ShadeId(3))),
        },
    );
    assert_eq!(
        setup.apply(SetupInput::Done),
        Effect {
            form: FormChange::Close,
            ask: None,
        },
    );
    assert_eq!(setup.phase(), SetupPhase::Idle);
}

/// A discard before anything was created costs nothing and asks for nothing.
#[test]
fn discarding_a_draft_removes_the_form_and_nothing_else() {
    let mut setup = opened();
    setup.apply(SetupInput::Name("Lounge"));
    assert_eq!(
        setup.apply(SetupInput::Discard),
        Effect {
            form: FormChange::Close,
            ask: None,
        },
    );
    assert_eq!(setup.phase(), SetupPhase::Idle);
    // And the next setup starts blank rather than inheriting the abandoned one.
    setup.apply(SetupInput::Begin);
    assert_eq!(setup.value(SetupEntity::Name), SetupValue::Unset);
}

/// A discard after the shade exists takes the shade with it.
///
/// Leaving it would leave a row in `Shades awaiting setup` that nothing on this
/// surface can reach again — the half-finished artefact the whole flow is
/// shaped to make unreachable.
#[test]
fn discarding_after_creation_removes_the_shade_too() {
    let mut setup = opened();
    setup.apply(SetupInput::Name("Lounge"));
    setup.apply(SetupInput::TravelUp("10000"));
    setup.apply(SetupInput::TravelDown("10000"));
    setup.apply(SetupInput::Send);
    setup.apply(SetupInput::Created(ShadeId(5)));

    assert_eq!(
        setup.apply(SetupInput::Discard),
        Effect {
            form: FormChange::Close,
            ask: Some(Ask::Abandon(ShadeId(5))),
        },
    );
    assert_eq!(setup.phase(), SetupPhase::Idle);
}

/// A refusal from the shade table lands in the form rather than in a log.
#[test]
fn a_refusal_is_shown_where_the_operator_is() {
    let mut setup = opened();
    setup.apply(SetupInput::Name("Lounge"));
    setup.apply(SetupInput::TravelUp("10000"));
    setup.apply(SetupInput::TravelDown("10000"));
    assert_eq!(setup.apply(SetupInput::Send).ask, Some(Ask::Create));

    assert_eq!(
        setup.apply(SetupInput::Refused(SetupMessage::RegistryFull)),
        Effect {
            form: FormChange::Unchanged,
            ask: None,
        },
    );
    assert_eq!(setup.message(), SetupMessage::RegistryFull);
    assert_eq!(
        setup.phase(),
        SetupPhase::Drafting,
        "a refused creation must not leave the flow believing a shade exists",
    );
}

/// A value the form cannot use leaves the draft alone and says so.
///
/// Never coerced: a truncated name is a different shade from the one that was
/// typed, and a clamped travel time is the silently-wrong number this form
/// exists to stop.
#[test]
fn an_unusable_value_is_refused_rather_than_coerced() {
    let mut setup = opened();

    let long = "x".repeat(MAX_DRAFT_NAME_LEN + 1);
    assert_eq!(setup.apply(SetupInput::Name(&long)).ask, None);
    assert_eq!(setup.message(), SetupMessage::NameTooLong);
    assert_eq!(setup.value(SetupEntity::Name), SetupValue::Unset);

    setup.apply(SetupInput::Name("Lounge"));
    for bad in [
        "", "abc", "-5", "+10", "1e4", "10..0", "0", "180001", " 100",
    ] {
        setup.apply(SetupInput::TravelUp(bad));
        assert_eq!(
            setup.draft().up_ms(),
            None,
            "{bad:?} was accepted as a travel time",
        );
    }
    setup.apply(SetupInput::Kind("Trebuchet"));
    assert_eq!(
        setup.draft().kind(),
        ShadeKind::Roller,
        "an unknown option must leave the kind alone",
    );
}

/// Home Assistant sends `10000` for a whole number and can send `10000.5`; both
/// are accepted, and the fraction is dropped rather than the value refused.
#[test]
fn a_travel_time_is_read_the_way_home_assistant_writes_it() {
    let mut setup = opened();
    for (sent, expected) in [
        ("10000", 10_000u32),
        ("10000.0", 10_000),
        ("10000.5", 10_000),
        ("1", MIN_TRAVEL_MS),
        ("180000", MAX_TRAVEL_MS),
    ] {
        setup.apply(SetupInput::TravelDown(sent));
        assert_eq!(setup.draft().down_ms(), Some(expected), "sent {sent:?}");
    }
}

/// Only the exact `PRESS` payload acts. A lenient parse here would let a stray
/// retained message create a shade or put `Prog` on the air.
#[test]
fn a_button_acts_only_on_the_exact_press_payload() {
    let cfg = config();
    let topic = cfg.setup_command_topic(SetupEntity::Send);
    assert_eq!(
        Setup::decode(&cfg, topic.as_str(), PAYLOAD_PRESS.as_bytes()),
        Some(SetupInput::Send),
    );
    for payload in [b"press".as_slice(), b"1", b"", b"PRESS ", b"ON"] {
        assert_eq!(
            Setup::decode(&cfg, topic.as_str(), payload),
            None,
            "{payload:?} was treated as a press",
        );
    }
}

/// Every form command topic decodes to the input it belongs to, and nothing
/// else does.
#[test]
fn decoding_is_by_exact_topic_and_covers_every_control() {
    let cfg = config();
    let mut decoded = 0;
    for entity in SetupEntity::ALL {
        if !entity.accepts_command() {
            assert_eq!(
                Setup::decode(&cfg, cfg.setup_topic(entity).as_str(), b"anything"),
                None,
                "{entity:?} has no command topic and something decoded on its state topic",
            );
            continue;
        }
        let topic = cfg.setup_command_topic(entity);
        let payload: &[u8] = match entity.component() {
            Component::Button => PAYLOAD_PRESS.as_bytes(),
            Component::Number => b"10000",
            Component::Select => b"Roller",
            _ => b"Lounge",
        };
        assert!(
            Setup::decode(&cfg, topic.as_str(), payload).is_some(),
            "{entity:?} did not decode",
        );
        decoded += 1;
    }
    assert_eq!(decoded, 8, "eight of the nine entities take a command");

    // A shade's own topics are not the form's.
    assert_eq!(
        Setup::decode(
            &cfg,
            cfg.shade_topic(ShadeId(1), somfy_mqtt::ShadeTopic::Command)
                .as_str(),
            b"OPEN",
        ),
        None,
    );
}

/// `Add shade` pressed while a setup is running re-announces rather than
/// restarting: a draft already typed survives a broker that lost the configs.
#[test]
fn add_shade_during_a_setup_recovers_the_form_without_losing_the_draft() {
    let mut setup = opened();
    setup.apply(SetupInput::Name("Lounge"));
    setup.apply(SetupInput::TravelUp("12000"));

    assert_eq!(
        setup.apply(SetupInput::Begin),
        Effect {
            form: FormChange::Open,
            ask: None,
        },
    );
    assert_eq!(setup.value(SetupEntity::Name), SetupValue::Text("Lounge"));
    assert_eq!(setup.draft().up_ms(), Some(12_000));
}

/// The form never publishes a value for a button, and never a placeholder for
/// an unset number.
///
/// A retained placeholder outlives the boot that produced it and is handed to
/// every later subscriber — the confidently-wrong retained value this whole
/// integration is written around. `Unset` means publish nothing, and Home
/// Assistant shows the entity as unknown, which is what it is.
#[test]
fn a_button_has_no_value_and_an_unset_number_has_no_placeholder() {
    let setup = opened();
    for entity in SetupEntity::ALL {
        let value = setup.value(entity);
        if !entity.has_state() {
            assert_eq!(value, SetupValue::Unset, "{entity:?} is a button");
        }
    }
    assert_eq!(setup.value(SetupEntity::TravelUp), SetupValue::Unset);
    assert_eq!(setup.value(SetupEntity::TravelDown), SetupValue::Unset);
    assert!(matches!(
        setup.value(SetupEntity::NextStep),
        SetupValue::Text(_)
    ));
}
