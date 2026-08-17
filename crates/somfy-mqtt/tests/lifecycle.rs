//! R5 and R6 — the lifecycle, as data rather than as a sequence of calls
//! somebody remembers to make in the right order.
//!
//! Every rule in R5 is a statement about *which* message goes to *which* topic
//! with *which* retention, and every one of them is pure. So the firmware's
//! broker session does not decide any of it: it executes a plan built here, and
//! these tests are assertions about the plan.
//!
//! The three that matter most, and the failure each one is about:
//!
//! - **Retained where it must be.** A discovery config that is not retained
//!   vanishes when the broker restarts, and the entities with it — the device
//!   has to be power-cycled to get them back.
//! - **Cleared when it must be.** Removing an entity means a zero-length
//!   retained publish to its config topic. Without it the estate accumulates
//!   orphans that can only be deleted by hand; cleaning up after the
//!   experiments behind the requirements spec took 49 of them.
//! - **Never retained where it must not be.** A retained command replays on
//!   every reconnect, which is a shade that closes itself each time the broker
//!   restarts.

use somfy_domain::ShadeId;
use somfy_mqtt::{
    reconfigure, Component, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, ObjectId, Payload,
    PublishedTopic, Retention, ShadeTopic, StateRoot, Step, SubscribedTopic, OFFLINE, ONLINE,
    SHADE_COMPONENTS,
};

const NODE: &str = "somfyrs";
const DEVICE: &str = "a1b2c3d4";

fn config(prefix: &str, root: &str) -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new(prefix).expect("valid prefix"),
        StateRoot::new(root).expect("valid root"),
        NodeId::new(NODE).expect("valid node id"),
        DeviceId::new(DEVICE).expect("valid device id"),
    )
    .expect("valid config")
}

fn default_config() -> MqttConfig {
    config("homeassistant", "somfyrs")
}

/// One step, flattened to something a test can compare and print.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Flat {
    Send {
        topic: String,
        retained: bool,
        payload: FlatPayload,
    },
    Listen {
        topic: String,
        retained_replay: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FlatPayload {
    Bytes(Vec<u8>),
    Discovery(u8, &'static str),
    Nothing,
}

fn flatten<'a>(steps: impl Iterator<Item = Step<'a>>) -> Vec<Flat> {
    steps
        .map(|step| match step {
            Step::Send(publish) => Flat::Send {
                topic: publish.topic().as_str().to_string(),
                retained: publish.retention() == Retention::Retained,
                payload: match publish.payload() {
                    Payload::Bytes(bytes) => FlatPayload::Bytes(bytes.to_vec()),
                    Payload::Discovery { shade, component } => {
                        FlatPayload::Discovery(shade.0, component.as_str())
                    }
                    Payload::Nothing => FlatPayload::Nothing,
                },
            },
            Step::Listen(listen) => Flat::Listen {
                topic: listen.topic().as_str().to_string(),
                retained_replay: listen.retained_replay(),
            },
        })
        .collect()
}

/// Every topic a step addresses, in order, whichever kind it is.
fn topics(steps: &[Flat]) -> Vec<String> {
    steps
        .iter()
        .map(|step| match step {
            Flat::Send { topic, .. } | Flat::Listen { topic, .. } => topic.clone(),
        })
        .collect()
}

/// Every topic a step *publishes* to, in order.
fn published(steps: &[Flat]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|step| match step {
            Flat::Send { topic, .. } => Some(topic.clone()),
            Flat::Listen { .. } => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// R5 — the will and the availability topic
// ---------------------------------------------------------------------------

/// The will is registered in CONNECT, so it is the only message the broker can
/// send on this device's behalf once the device is no longer there to send
/// anything. It has to be retained: a subscriber that connects after the device
/// died must still learn that it is dead.
#[test]
fn the_will_is_offline_retained_at_the_availability_topic() {
    let config = default_config();
    let will = config.will();
    assert_eq!(will.topic().as_str(), "somfyrs/status");
    assert_eq!(will.retention(), Retention::Retained);
    assert_eq!(will.payload(), Payload::Bytes(OFFLINE));
    assert_eq!(OFFLINE, b"offline");
}

/// And `online` is the first thing said on a session that has just been
/// established, for the symmetric reason: a subscriber connecting later has to
/// find the device marked available without waiting for it to say anything
/// else.
#[test]
fn online_is_the_first_message_of_an_announcement_and_it_is_retained() {
    let config = default_config();
    let steps = flatten(config.announce(&[ShadeId(1), ShadeId(2)], false));
    assert_eq!(
        steps.first(),
        Some(&Flat::Send {
            topic: "somfyrs/status".to_string(),
            retained: true,
            payload: FlatPayload::Bytes(ONLINE.to_vec()),
        }),
    );
    assert_eq!(ONLINE, b"online");
}

// ---------------------------------------------------------------------------
// R5 — discovery configs are retained
// ---------------------------------------------------------------------------

/// The whole point of retaining them: a broker restart or a Home Assistant
/// restart re-populates the entities without anybody touching the device.
#[test]
fn every_discovery_config_is_published_retained() {
    let config = default_config();
    let steps = flatten(config.announce(&[ShadeId(1), ShadeId(7)], false));

    let configs: Vec<&Flat> = steps
        .iter()
        .filter(|step| matches!(step, Flat::Send { topic, .. } if topic.ends_with("/config")))
        .collect();
    assert_eq!(
        configs.len(),
        2 * SHADE_COMPONENTS.len(),
        "one config per shade per component: {steps:#?}",
    );
    for step in configs {
        let Flat::Send {
            retained, payload, ..
        } = step
        else {
            unreachable!("filtered to sends")
        };
        assert!(*retained, "a discovery config must be retained: {step:?}");
        assert!(
            matches!(payload, FlatPayload::Discovery(..)),
            "a discovery config carries a rendered payload: {step:?}",
        );
    }

    assert!(topics(&steps).contains(&"homeassistant/cover/somfyrs/shade_1/config".to_string()));
    assert!(topics(&steps).contains(&"homeassistant/cover/somfyrs/shade_7/config".to_string()));
}

// ---------------------------------------------------------------------------
// R5 — removal is a zero-length retained publish
// ---------------------------------------------------------------------------

/// The rule stated literally. A retained message is deleted from a broker by
/// publishing a zero-length payload to the same topic **with the retain flag
/// set**; a zero-length publish without it deletes nothing and a non-empty one
/// leaves Home Assistant trying to parse it.
#[test]
fn removing_a_shade_publishes_a_zero_length_retained_payload_to_its_config_topic() {
    let config = default_config();
    let steps = flatten(config.retire_shade(ShadeId(3)));

    let tombstone = steps
        .iter()
        .find(|step| matches!(step, Flat::Send { topic, .. } if topic.ends_with("/config")))
        .expect("a retirement clears the discovery config");
    assert_eq!(
        tombstone,
        &Flat::Send {
            topic: "homeassistant/cover/somfyrs/shade_3/config".to_string(),
            retained: true,
            payload: FlatPayload::Nothing,
        },
    );
}

/// Every retained topic a shade owns is cleared, not just its config. The
/// evidence behind the requirements is 49 retained topics deleted by hand, and
/// most of them were state topics: `.../shades/2/position` outlives the entity
/// exactly as `.../cover/shade_2/config` does.
#[test]
fn retiring_a_shade_clears_every_topic_the_firmware_ever_retains_for_it() {
    let config = default_config();
    let steps = flatten(config.retire_shade(ShadeId(2)));

    let mut expected: Vec<String> = SHADE_COMPONENTS
        .iter()
        .map(|component| {
            config
                .discovery_topic(*component, &ObjectId::for_shade(ShadeId(2)))
                .as_str()
                .to_string()
        })
        .collect();
    // Deliberately *with* tilt: see the test below for why retirement never
    // asks whether the shade had one.
    expected.extend(PublishedTopic::for_shade(true).map(|topic| {
        config
            .shade_topic(ShadeId(2), topic.into())
            .as_str()
            .to_string()
    }));

    let mut got = published(&steps);
    got.sort();
    expected.sort();
    assert_eq!(got, expected);

    for step in &steps {
        assert_eq!(
            step,
            &Flat::Send {
                topic: match step {
                    Flat::Send { topic, .. } | Flat::Listen { topic, .. } => topic.clone(),
                },
                retained: true,
                payload: FlatPayload::Nothing,
            },
            "every step of a retirement is a zero-length retained publish",
        );
    }
}

/// Retirement never takes `has_tilt`, and this is why: a shade announced with
/// tilt and retired without it would leave `.../tilt` and `.../tilt/set`
/// behind. Clearing a topic that was never published costs one packet and
/// removes the whole class of mistake.
#[test]
fn the_tilt_topics_are_cleared_even_for_a_shade_that_never_had_tilt() {
    let config = default_config();
    let cleared = published(&flatten(config.retire_shade(ShadeId(4))));
    assert!(
        cleared.contains(&"somfyrs/shades/4/tilt".to_string()),
        "{cleared:?}"
    );
}

/// The structural version of the two tests above, and the one that survives
/// Task 4 adding components: whatever an announcement retains, a retirement
/// clears. A component added to `SHADE_COMPONENTS` joins both sides at once.
#[test]
fn retirement_clears_every_topic_an_announcement_retains() {
    let config = default_config();
    for has_tilt in [false, true] {
        let announced: Vec<String> = flatten(config.announce(&[ShadeId(5)], has_tilt))
            .into_iter()
            .filter_map(|step| match step {
                Flat::Send {
                    topic,
                    retained: true,
                    ..
                } => Some(topic),
                _ => None,
            })
            .collect();
        // Plus everything the running firmware retains as state, which an
        // announcement cannot emit because it does not know the values.
        let state: Vec<String> = PublishedTopic::for_shade(has_tilt)
            .map(|topic| {
                config
                    .state(ShadeId(5), topic, b"0")
                    .topic()
                    .as_str()
                    .to_string()
            })
            .collect();

        let cleared = published(&flatten(config.retire(&[ShadeId(5)])));

        for topic in announced.into_iter().chain(state) {
            assert!(
                cleared.contains(&topic),
                "{topic} is retained but never cleared (has_tilt={has_tilt})",
            );
        }
    }
}

/// A device-wide retirement clears availability too. Without it the old
/// `{state_root}/status` keeps saying `online` forever, which is the same
/// orphan as a stale config and a worse one to read: it is confidently wrong.
#[test]
fn retiring_the_device_clears_its_availability_topic() {
    let config = default_config();
    let steps = flatten(config.retire(&[ShadeId(1)]));
    assert!(steps.contains(&Flat::Send {
        topic: "somfyrs/status".to_string(),
        retained: true,
        payload: FlatPayload::Nothing,
    }));
}

// ---------------------------------------------------------------------------
// R5 — changing a root deletes the old configs before publishing the new ones
// ---------------------------------------------------------------------------

/// Stated as an ordering because that is what it is: the tombstones must reach
/// the broker before the new configs. Publishing the new ones first is not
/// merely untidy — Home Assistant would create the new entities and then be
/// told to delete the old ones, and for the window in between the estate has
/// both.
#[test]
fn changing_the_state_root_clears_the_old_topics_before_publishing_the_new_ones() {
    let old = [config("homeassistant", "oldroot")];
    let new = config("homeassistant", "newroot");
    let steps = flatten(reconfigure(&old, &new, &[ShadeId(1)], false));

    let first_new = steps
        .iter()
        .position(|step| matches!(step, Flat::Send { topic, .. } if topic.starts_with("newroot/")))
        .expect("the new configuration is announced");
    let last_old = steps
        .iter()
        .rposition(|step| matches!(step, Flat::Send { topic, .. } if topic.starts_with("oldroot/")))
        .expect("the old configuration is retired");
    assert!(
        last_old < first_new,
        "every old-root topic must be cleared before any new-root topic is published: {steps:#?}",
    );

    // And the old ones are cleared, not merely mentioned.
    for step in &steps {
        if let Flat::Send { topic, payload, .. } = step {
            if topic.starts_with("oldroot/") {
                assert_eq!(payload, &FlatPayload::Nothing, "{step:?}");
            }
        }
    }
}

/// The same rule for the other root. It is a separate test because the two
/// namespaces are independent — a validator or a plan that is right for one and
/// forgotten for the other is the asymmetry behind the whole requirements spec.
#[test]
fn changing_the_discovery_prefix_clears_the_old_configs_before_publishing_the_new_ones() {
    let old = [config("oldprefix", "somfyrs")];
    let new = config("homeassistant", "somfyrs");
    let steps = flatten(reconfigure(&old, &new, &[ShadeId(1)], false));

    let old_config = "oldprefix/cover/somfyrs/shade_1/config".to_string();
    let new_config = "homeassistant/cover/somfyrs/shade_1/config".to_string();
    let all = topics(&steps);
    let old_at = all
        .iter()
        .position(|t| *t == old_config)
        .expect("old cleared");
    let new_at = all
        .iter()
        .position(|t| *t == new_config)
        .expect("new published");
    assert!(old_at < new_at, "{steps:#?}");

    assert_eq!(
        steps[old_at],
        Flat::Send {
            topic: old_config,
            retained: true,
            payload: FlatPayload::Nothing,
        },
    );
}

/// Several superseded configurations are all cleared, and the new one is
/// announced **once** — not once per old configuration.
///
/// A caller that looped over the old ones itself would republish every retained
/// config per old namespace, which is a broker's worth of traffic repeated for
/// no change. That is why the loop is inside `reconfigure` and not outside it.
#[test]
fn every_superseded_configuration_is_cleared_and_the_new_one_announced_once() {
    let superseded = [
        config("homeassistant", "firstroot"),
        config("homeassistant", "secondroot"),
    ];
    let new = default_config();
    let steps = flatten(reconfigure(&superseded, &new, &[ShadeId(1)], false));
    let sent = published(&steps);

    for root in ["firstroot", "secondroot"] {
        assert!(
            sent.iter()
                .any(|topic| topic.starts_with(&format!("{root}/"))),
            "{root} was never cleared: {steps:#?}",
        );
    }

    // The announcement itself happens once. Counted over the *announcement's*
    // topics rather than over all publishes, because a tombstone can legally
    // address the same topic — see the test below.
    let announced = flatten(new.announce(&[ShadeId(1)], false));
    for topic in published(&announced) {
        let announcements = steps
            .iter()
            .filter(|step| {
                matches!(step, Flat::Send { topic: t, payload, .. }
                    if *t == topic && *payload != FlatPayload::Nothing)
            })
            .count();
        assert_eq!(
            announcements, 1,
            "{topic} was announced {announcements} times"
        );
    }
}

/// The property that makes the churn above harmless: **wherever a tombstone and
/// a publish address the same topic, the publish is last.**
///
/// Two configurations that differ in only one of their two namespaces still
/// share the other one's topics, so retiring the old one clears an address the
/// new one is about to use. That is a momentary removal in Home Assistant and
/// not a lost entity — `unique_id` is stable, so the entity comes back as
/// itself — but only because the order is this way round. Reversed, the device
/// would announce its configuration and then delete it, and the estate would be
/// left with nothing at all.
#[test]
fn a_tombstone_never_outlives_a_publish_to_the_same_topic() {
    let superseded = [
        // Shares the discovery prefix: its config tombstone lands on the topic
        // the announcement republishes.
        config("homeassistant", "otherroot"),
        // Shares the state root: its availability and state tombstones land on
        // the topics the announcement and the state republish use.
        config("otherprefix", "somfyrs"),
    ];
    let new = default_config();
    let sent = published(&flatten(reconfigure(
        &superseded,
        &new,
        &[ShadeId(1)],
        false,
    )));
    let steps = flatten(reconfigure(&superseded, &new, &[ShadeId(1)], false));

    for topic in &sent {
        let last = steps
            .iter()
            .rposition(|step| matches!(step, Flat::Send { topic: t, .. } if t == topic))
            .expect("the topic was published");
        let Flat::Send { payload, .. } = &steps[last] else {
            unreachable!("filtered to sends")
        };
        // A topic whose *last* word is a tombstone is a topic this device has
        // deliberately removed. That is right for anything only the old
        // configuration owned, and wrong for anything the new one publishes.
        let announced = published(&flatten(new.announce(&[ShadeId(1)], false)));
        if announced.contains(topic) {
            assert_ne!(
                payload,
                &FlatPayload::Nothing,
                "{topic} is announced by the new configuration and ends as a tombstone",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R6 — commands are never retained, in either direction
// ---------------------------------------------------------------------------

/// The publish side. No plan this crate can build addresses a command topic
/// with a publish at all, retained or not: the firmware subscribes to those and
/// has nothing to say on them.
#[test]
fn no_plan_ever_publishes_to_a_command_topic() {
    let old = [config("oldprefix", "oldroot")];
    let new = default_config();
    let shades = [ShadeId(1), ShadeId(2), ShadeId(255)];

    let mut command_topics: Vec<String> = Vec::new();
    for shade in shades {
        for topic in SubscribedTopic::for_shade(true) {
            for cfg in [&old[0], &new] {
                command_topics.push(cfg.shade_topic(shade, topic.into()).as_str().to_string());
            }
        }
    }

    let plans: Vec<Vec<Flat>> = vec![
        flatten(new.announce(&shades, true)),
        flatten(new.announce(&shades, false)),
        flatten(new.retire(&shades)),
        flatten(new.retire_shade(ShadeId(1))),
        flatten(reconfigure(&old, &new, &shades, true)),
        vec![Flat::Send {
            topic: new.will().topic().as_str().to_string(),
            retained: true,
            payload: FlatPayload::Bytes(OFFLINE.to_vec()),
        }],
    ];

    for plan in &plans {
        for topic in published(plan) {
            assert!(
                !command_topics.contains(&topic),
                "{topic} is a command topic and nothing may publish to it",
            );
        }
    }
}

/// The subscribe side, which R6 names explicitly and which is the half that is
/// easy to miss. A broker that already holds a retained message on a command
/// topic — left by a previous integration, or by a `mosquitto_pub -r` during
/// debugging — replays it to every new subscriber. Suppressing that replay is
/// the only defence the subscriber has.
#[test]
fn a_subscription_never_asks_for_retained_messages() {
    let config = default_config();
    let steps = flatten(config.announce(&[ShadeId(1), ShadeId(2)], true));
    let listens: Vec<&Flat> = steps
        .iter()
        .filter(|step| matches!(step, Flat::Listen { .. }))
        .collect();
    assert!(
        !listens.is_empty(),
        "an announcement subscribes to something"
    );
    for step in listens {
        let Flat::Listen {
            retained_replay, ..
        } = step
        else {
            unreachable!("filtered to listens")
        };
        assert!(
            !retained_replay,
            "a command subscription must suppress retained replay: {step:?}",
        );
    }
}

/// And the subscriptions are exactly the command topics — no more, so nothing
/// arrives that the firmware has no handler for, and no fewer, so no command
/// silently does nothing.
#[test]
fn the_subscriptions_are_exactly_the_command_topics() {
    let config = default_config();
    for has_tilt in [false, true] {
        let steps = flatten(config.announce(&[ShadeId(9)], has_tilt));
        let mut listened: Vec<String> = steps
            .iter()
            .filter_map(|step| match step {
                Flat::Listen { topic, .. } => Some(topic.clone()),
                Flat::Send { .. } => None,
            })
            .collect();
        let mut expected: Vec<String> = SubscribedTopic::for_shade(has_tilt)
            .map(|topic| {
                config
                    .shade_topic(ShadeId(9), topic.into())
                    .as_str()
                    .to_string()
            })
            .collect();
        listened.sort();
        expected.sort();
        assert_eq!(listened, expected, "has_tilt={has_tilt}");
    }
}

// ---------------------------------------------------------------------------
// The type-level halves of the same two rules
// ---------------------------------------------------------------------------

/// A command topic cannot be turned into something the retained-state
/// constructor accepts. This is R6 made unrepresentable rather than checked:
/// `MqttConfig::state` publishes retained and takes only a [`PublishedTopic`].
#[test]
fn a_command_topic_cannot_become_a_published_topic() {
    for topic in ShadeTopic::ALL {
        let published = PublishedTopic::of(topic);
        let subscribed = SubscribedTopic::of(topic);
        assert_eq!(
            published.is_some(),
            subscribed.is_none(),
            "{topic:?} must be exactly one of the two",
        );
    }
    assert!(PublishedTopic::of(ShadeTopic::Command).is_none());
    assert!(PublishedTopic::of(ShadeTopic::SetPosition).is_none());
    assert!(PublishedTopic::of(ShadeTopic::TiltCommand).is_none());
    assert!(PublishedTopic::of(ShadeTopic::Position).is_some());
}

/// State the firmware publishes is retained, so a subscriber that connects
/// later sees the current position rather than waiting for the next change —
/// which, for a shade nobody touches, may be days.
#[test]
fn state_is_published_retained() {
    let config = default_config();
    let publish = config.state(
        ShadeId(1),
        PublishedTopic::of(ShadeTopic::Position).expect("position is published"),
        b"69",
    );
    assert_eq!(publish.topic().as_str(), "somfyrs/shades/1/position");
    assert_eq!(publish.retention(), Retention::Retained);
    assert_eq!(publish.payload(), Payload::Bytes(b"69"));
}

/// Every component this crate can name is a real Home Assistant component, and
/// the per-shade set is the one both halves of the lifecycle read.
#[test]
fn the_per_shade_component_set_is_what_both_halves_read() {
    assert!(SHADE_COMPONENTS.contains(&Component::Cover));
    for component in SHADE_COMPONENTS {
        assert!(Component::ALL.contains(&component));
    }
}
