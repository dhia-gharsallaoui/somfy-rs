//! **A removal can only ever reach a shade this flow created.**
//!
//! A confirmed shade — `ConfirmedByOperator`, at a controller-allocated
//! address, on a real estate — disappeared from the shade table with nobody
//! pressing anything, and the add-a-shade form was the only new thing that
//! could issue a `Remove`. This file is the regression suite for that, written
//! at the level the fault lives.
//!
//! The tests are deliberately not "does discard work". They are "is there *any*
//! way to reach a removal without having created the thing", asked
//! exhaustively over the input vocabulary rather than over a scenario somebody
//! thought of — because the fault was reached by a path nobody thought of, and
//! a scenario test only ever covers the scenarios that were imagined.
//!
//! The fix they guard is `somfy_mqtt::OwnShade`: `Ask::Abandon` carries a
//! newtype whose only constructor is private and called from exactly one place,
//! so "the form can only delete what it made" is a property of the type rather
//! than a claim about control flow.

use somfy_domain::ShadeId;
use somfy_mqtt::{
    Ask, DeviceId, DiscoveryPrefix, Effect, FormChange, MqttConfig, NodeId, Pairing, Setup,
    SetupEntity, SetupInput, SetupMessage, SetupPhase, StateRoot, Step, PAYLOAD_PRESS,
};

fn config() -> MqttConfig {
    MqttConfig::new(
        DiscoveryPrefix::new("homeassistant").unwrap(),
        StateRoot::new("somfyrs").unwrap(),
        NodeId::new("somfyrs").unwrap(),
        DeviceId::new("a1b2c3d4").unwrap(),
    )
    .unwrap()
}

/// Every input the flow accepts, except the one that creates a shade.
const HARMLESS: [SetupInput<'static>; 14] = [
    SetupInput::Begin,
    SetupInput::Send,
    SetupInput::Confirm,
    SetupInput::Discard,
    SetupInput::Done,
    SetupInput::Refused(SetupMessage::Refused),
    SetupInput::Refused(SetupMessage::RegistryFull),
    SetupInput::Name("Lounge"),
    SetupInput::Name(""),
    SetupInput::Kind("Roller"),
    SetupInput::Kind("nonsense"),
    SetupInput::TravelUp("10000"),
    SetupInput::TravelDown("10000"),
    SetupInput::TravelUp("nonsense"),
];

/// **Every ordered triple over the vocabulary, with no create anywhere in it,
/// asks for no removal.**
///
/// 2,744 sequences from a fresh flow. `Created` is excluded deliberately: it is
/// the one input that is *allowed* to make a removal possible, and the test
/// below covers what it makes possible.
#[test]
fn no_sequence_without_a_create_can_ever_ask_for_a_removal() {
    for a in HARMLESS {
        for b in HARMLESS {
            for c in HARMLESS {
                let mut setup = Setup::new();
                for input in [a, b, c] {
                    let effect = setup.apply(input);
                    assert!(
                        !matches!(effect.ask, Some(Ask::Abandon(_))),
                        "{a:?} then {b:?} then {c:?} reached a removal with nothing created",
                    );
                }
            }
        }
    }
}

/// The same, starting from a flow that has already finished a setup — so it has
/// held a real id at some point and could have kept a stale claim on it.
#[test]
fn no_sequence_after_a_finished_setup_can_ask_for_a_removal_either() {
    for ending in [SetupInput::Done, SetupInput::Discard] {
        for a in HARMLESS {
            for b in HARMLESS {
                let mut setup = Setup::new();
                drive_to_created(&mut setup, ShadeId(9));
                setup.apply(ending);
                assert_eq!(setup.phase(), SetupPhase::Idle);

                for input in [a, b] {
                    assert!(
                        !matches!(setup.apply(input).ask, Some(Ask::Abandon(_))),
                        "after {ending:?}, {a:?} then {b:?} reached a removal",
                    );
                }
            }
        }
    }
}

/// With a create, a removal is reachable — and names **only** that id.
#[test]
fn a_removal_names_the_shade_the_flow_created_and_no_other() {
    let mut setup = Setup::new();
    drive_to_created(&mut setup, ShadeId(7));

    match setup.apply(SetupInput::Discard).ask {
        Some(Ask::Abandon(own)) => assert_eq!(own.id(), ShadeId(7)),
        other => panic!("a discard after a create must remove that shade, got {other:?}"),
    }
    assert_eq!(setup.phase(), SetupPhase::Idle);
}

/// **A discard with no setup open touches nothing** — the first regression
/// asked for.
///
/// An idle flow holds no id at all, so there is nothing for a spurious press to
/// name. That is the property; "the id happens to be zero today" would not be.
#[test]
fn a_discard_with_no_setup_open_asks_for_nothing() {
    let mut setup = Setup::new();
    assert_eq!(setup.phase(), SetupPhase::Idle);
    for _ in 0..5 {
        assert_eq!(
            setup.apply(SetupInput::Discard),
            Effect {
                form: FormChange::Unchanged,
                ask: None,
            },
        );
    }
}

/// A confirmed shade stops being the flow's the instant the setup ends.
///
/// This is the shape of what happened: a shade that had been through the form
/// once, later removed by something the form issued. After a confirmation there
/// is no claim left to reach it through.
#[test]
fn a_confirmed_shade_cannot_be_reached_by_a_later_discard() {
    let mut setup = Setup::new();
    drive_to_created(&mut setup, ShadeId(3));
    assert_eq!(
        setup.apply(SetupInput::Confirm).ask,
        Some(Ask::Confirm(ShadeId(3)))
    );
    assert_eq!(setup.apply(SetupInput::Done).form, FormChange::Close);

    // A second setup, abandoned before it creates anything, must not reach the
    // shade the first one confirmed.
    setup.apply(SetupInput::Begin);
    setup.apply(SetupInput::Name("Something else"));
    assert_eq!(
        setup.apply(SetupInput::Discard).ask,
        None,
        "a fresh setup's discard reached the shade a previous one confirmed",
    );
}

/// **A reconnect touches nothing** — the second regression asked for.
///
/// Everything a session does on reconnect is a plan of publishes and
/// subscriptions. There is no path from any of them into the flow, so there is
/// none into a removal; and the flow a reconnect leaves behind still holds no
/// id.
#[test]
fn a_reconnect_publishes_and_subscribes_and_asks_for_nothing() {
    let cfg = config();
    let shades = [ShadeId(0), ShadeId(1), ShadeId(2), ShadeId(3)];

    let mut sends = 0;
    let mut listens = 0;
    for step in cfg
        .announce(&shades, false, |_| Pairing::Offered)
        .chain(cfg.close_form())
        .chain(cfg.open_form())
    {
        match step {
            Step::Send(publish) => {
                assert!(
                    publish.topic().as_str().starts_with("homeassistant/")
                        || publish.topic().as_str().starts_with("somfyrs/"),
                    "a reconnect published outside this device's namespaces: {}",
                    publish.topic(),
                );
                sends += 1;
            }
            Step::Listen(_) => listens += 1,
        }
    }
    assert!(sends > 0 && listens > 0);

    let mut setup = Setup::new();
    assert_eq!(setup.phase(), SetupPhase::Idle);
    assert_eq!(setup.apply(SetupInput::Discard).ask, None);
}

/// Nothing a broker could be holding on the form's own subscriptions decodes to
/// a discard by accident.
///
/// The retained-command hypothesis, checked rather than assumed. The eight form
/// subscriptions are the form's entire inbound surface.
#[test]
fn nothing_a_broker_could_replay_decodes_to_a_discard_by_accident() {
    let cfg = config();
    let payloads: [&[u8]; 9] = [
        b"", b"0", b"1", b"ON", b"OFF", b"press", b"None", b"online", b"PRESS ",
    ];
    for entity in SetupEntity::ALL {
        if !entity.accepts_command() {
            continue;
        }
        let topic = cfg.setup_command_topic(entity);
        for payload in payloads {
            assert!(
                !matches!(
                    Setup::decode(&cfg, topic.as_str(), payload),
                    Some(SetupInput::Discard)
                ),
                "{entity:?} + {payload:?} decoded as a discard",
            );
        }
    }
    // The one thing that does, and only on its own topic.
    assert_eq!(
        Setup::decode(
            &cfg,
            cfg.setup_command_topic(SetupEntity::Discard).as_str(),
            PAYLOAD_PRESS.as_bytes(),
        ),
        Some(SetupInput::Discard),
    );
}

/// Drive a flow from idle to a created shade with the given id.
fn drive_to_created(setup: &mut Setup, id: ShadeId) {
    setup.apply(SetupInput::Begin);
    setup.apply(SetupInput::Name("Lounge"));
    setup.apply(SetupInput::TravelUp("10000"));
    setup.apply(SetupInput::TravelDown("10000"));
    assert_eq!(setup.apply(SetupInput::Send).ask, Some(Ask::Create));
    setup.apply(SetupInput::Created(id));
    assert_eq!(setup.phase(), SetupPhase::AwaitingReport { shade: id });
}
