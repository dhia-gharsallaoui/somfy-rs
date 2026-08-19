//! TypeScript binding generation + wire-contract drift gate.
//!
//! `cargo test -p somfy-api --features ts` regenerates the UI types under
//! `ui/src/api/generated/` (ts-rs also emits one hidden `export_bindings_*`
//! test per `#[ts(export)]` type; this integration test re-exports explicitly so
//! it is self-contained and does not depend on cross-binary test ordering).
//!
//! Beyond "the files exist", the assertions below **pin the wire contract**.
//! `CommandDto` and `WsEvent` do NOT use serde's derive tagging — their wire
//! form is produced by hand-rolled `Serialize`/`Deserialize` impls over flat
//! helper structs (serde internal tagging needs `alloc`, which the firmware
//! forbids). ts-rs cannot see any serde tag attributes on those enums, so their
//! TS shape is driven purely by explicit `#[ts(tag = ...)]` overrides. If the
//! Rust wire format and the generated TS ever drift apart, these substring pins
//! fail here — not silently in the browser.
#![cfg(feature = "ts")]

use std::path::{Path, PathBuf};

use ts_rs::TS;

/// Absolute path to the committed bindings directory.
///
/// `CARGO_MANIFEST_DIR` is `<workspace>/crates/somfy-api`, so `../../ui/...`
/// resolves to `<workspace>/ui/src/api/generated`. (Note: this differs from the
/// `#[ts(export_to)]` value, which ts-rs resolves relative to the *source file*
/// directory `crates/somfy-api/src`, hence its extra `../`.)
fn generated_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/api/generated")
}

fn read(name: &str) -> String {
    let path = generated_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing generated binding {}: {e}", path.display()))
}

/// The `export type ...` line only, with the doc comment ts-rs copies above it
/// stripped off.
///
/// Needed for the *negative* assertions: a field must be absent from the
/// **type**, and prose above it naturally discusses the fields it does not have
/// — a whole-file substring search would fail on the explanation of why the
/// field is missing, which is the wrong thing to be sensitive to.
fn declaration(name: &str) -> String {
    read(name)
        .lines()
        .filter(|line| line.starts_with("export type"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Regenerate every binding. `export_all` also writes transitive dependencies
/// (e.g. `WsEvent` pulls in `ShadeStateEvent`). Idempotent: re-running produces
/// byte-identical files, which is what the CI `git diff --exit-code` relies on.
///
/// The three tests in this file run in parallel and each need the bindings
/// present, but concurrent `export_all` calls would race on the same files (one
/// test truncating a file mid-read of another). A `Once` collapses them to a
/// single generation; `call_once` blocks the other threads until it completes, so
/// every test then reads fully-written files.
fn regenerate() {
    static REGEN: std::sync::Once = std::sync::Once::new();
    REGEN.call_once(|| {
        somfy_api::ShadeDto::export_all().expect("export ShadeDto");
        somfy_api::GroupDto::export_all().expect("export GroupDto");
        somfy_api::RoomDto::export_all().expect("export RoomDto");
        somfy_api::CommandDto::export_all().expect("export CommandDto");
        somfy_api::WsEvent::export_all().expect("export WsEvent");
        somfy_api::CreateShadeDto::export_all().expect("export CreateShadeDto");
        somfy_api::PatchShadeDto::export_all().expect("export PatchShadeDto");
        somfy_api::ApiErrorDto::export_all().expect("export ApiErrorDto");
        somfy_api::CalibrationStepDto::export_all().expect("export CalibrationStepDto");
        somfy_api::SettingsDto::export_all().expect("export SettingsDto");
        somfy_api::WifiUpdateDto::export_all().expect("export WifiUpdateDto");
        somfy_api::MqttUpdateDto::export_all().expect("export MqttUpdateDto");
        somfy_api::TrialDecisionDto::export_all().expect("export TrialDecisionDto");
        somfy_api::SystemDto::export_all().expect("export SystemDto");
        somfy_api::RestoreReportDto::export_all().expect("export RestoreReportDto");
    });
}

#[test]
fn calibration_source_keeps_all_three_states() {
    regenerate();
    let source = read("CalibrationSource.ts");

    // Three states, not a boolean. "Nobody chose this", "somebody measured it"
    // and "the device swept it" call for three different actions, and R9 needs
    // the last two distinguishable so a sweep can be caught disagreeing with a
    // stopwatch.
    for state in [
        r#""factoryDefault""#,
        r#""operatorSupplied""#,
        r#""measured""#,
    ] {
        assert!(
            source.contains(state),
            "CalibrationSource lost {state}:\n{source}"
        );
    }

    // Each travel time carries its own provenance: an operator may measure the
    // lift and leave tilt alone, and one flag per shade would hide that.
    let shade = declaration("ShadeDto.ts");
    for field in [
        "upTimeSource: CalibrationSource",
        "downTimeSource: CalibrationSource",
        "tiltTimeSource: CalibrationSource",
    ] {
        assert!(shade.contains(field), "ShadeDto lost {field}:\n{shade}");
    }
}

#[test]
fn patch_fields_are_optional_and_exclude_what_the_device_owns() {
    regenerate();
    let patch = declaration("PatchShadeDto.ts");

    // Optional, because absent means "leave this alone" — a PATCH whose fields
    // were required would be a PUT wearing the wrong verb, and would force a
    // client correcting one travel time to resend the whole shade.
    for field in [
        "name?: string",
        "kind?: number",
        "upTimeMs?: number",
        "downTimeMs?: number",
        "tiltTimeMs?: number",
        // R9's second half. The three compensations are settable by hand for
        // the same reason the travel times are: a sweep runs the shade end to
        // end, and a measurement with nothing to check itself against cannot be
        // caught being wrong.
        "startLagMs?: number",
        "ventBandMs?: number",
        "closeBandMs?: number",
    ] {
        assert!(
            patch.contains(field),
            "PatchShadeDto lost {field}:\n{patch}"
        );
    }

    // The address stays the device's. Editing it would break a pairing a motor
    // has already learned, and the id is the Home Assistant entity's identity.
    // `myPosition` is excluded too: the favourite lives in the motor, so
    // changing it is a transmission, not a settings edit.
    for forbidden in ["address", "id?", "myPosition", "position"] {
        assert!(
            !patch.contains(forbidden),
            "PatchShadeDto must not accept `{forbidden}`:\n{patch}"
        );
    }

    // R8's dead band arrived as **three** fields rather than one, and that is
    // the contract: the two intervals of a traverse that move nothing are at
    // opposite ends of it and are not the same interval, and the start lag is a
    // third thing again that applies to every move in either direction. A single
    // `deadBand` would have collapsed them, so its absence is asserted rather
    // than assumed.
    for absent in ["deadBand", "deadband"] {
        assert!(
            !patch.contains(absent),
            "the three compensations must stay separate on the wire:\n{patch}"
        );
    }
}

#[test]
fn address_origin_is_a_string_union_the_ui_can_switch_on() {
    regenerate();
    let origin = read("AddressOrigin.ts");

    // A two-member string union, not a number: the UI branches on it to decide
    // whether pairing is offered at all, and `0`/`1` would put the meaning in a
    // lookup table nobody maintains.
    assert!(
        origin.contains(r#""allocated""#) && origin.contains(r#""imported""#),
        "AddressOrigin must stay a camelCase string union:\n{origin}"
    );

    // And it must actually reach the shade payload, non-nullable — a UI that
    // has to treat it as possibly-absent cannot use it as a gate.
    let shade = read("ShadeDto.ts");
    assert!(
        shade.contains("addressOrigin: AddressOrigin"),
        "ShadeDto must carry a required addressOrigin:\n{shade}"
    );
}

/// The state is named for whose knowledge it is, and it reaches the payload.
///
/// Two assertions, and the first is the load-bearing one: the wire must not
/// carry a word that reads as a device measurement. `paired` is what the C++
/// reference stores — set from an HTTP request body and never observed — and it
/// is the trap, not the model. RTS is one-way, so the strongest true statement
/// available is "a person told us", and the identifier has to say so or the next
/// reader will assume the device knows.
#[test]
fn pairing_state_says_whose_knowledge_it_is() {
    regenerate();
    let state = read("PairingState.ts");

    assert!(
        state.contains(r#""awaitingConfirmation""#) && state.contains(r#""confirmedByOperator""#),
        "PairingState must stay a camelCase string union:\n{state}"
    );

    // A boolean, or a member named for the motor's state rather than the
    // operator's report, would be the reference's mistake with our spelling.
    let union = declaration("PairingState.ts");
    for forbidden in [r#""paired""#, r#""unpaired""#, "boolean"] {
        assert!(
            !union.contains(forbidden),
            "PairingState must not claim device knowledge with `{forbidden}`:\n{union}"
        );
    }

    // Required and non-nullable on the shade: the UI gates the whole
    // finish-setup flow on it, and a possibly-absent field cannot be a gate.
    let shade = declaration("ShadeDto.ts");
    assert!(
        shade.contains("pairingState: PairingState"),
        "ShadeDto must carry a required pairingState:\n{shade}"
    );

    // And it is not editable through the patch surface. A settable field would
    // be settable in the *other* direction, and "set this back to unconfirmed"
    // retires the entities of a working shade.
    let patch = declaration("PatchShadeDto.ts");
    assert!(
        !patch.contains("pairingState"),
        "PatchShadeDto must not accept pairingState:\n{patch}"
    );
}

#[test]
fn create_shade_omits_everything_the_device_owns() {
    regenerate();
    let create = declaration("CreateShadeDto.ts");

    for field in [
        "name: string",
        "kind: number",
        "tiltMode: number",
        "upTimeMs: number",
        "downTimeMs: number",
        "tiltTimeMs: number",
    ] {
        assert!(
            create.contains(field),
            "CreateShadeDto lost {field}:\n{create}"
        );
    }

    // The device allocates the address and assigns the id, and derives the
    // origin from the address. A client that could set any of the three could
    // re-create the two-controllers-one-identity clash from a form field.
    for forbidden in ["address", "id:", "position", "addressOrigin"] {
        assert!(
            !create.contains(forbidden),
            "CreateShadeDto must not accept `{forbidden}` from a client:\n{create}"
        );
    }
}

#[test]
fn api_error_is_a_code_the_ui_can_translate() {
    regenerate();
    let code = read("ApiErrorCode.ts");
    let dto = read("ApiErrorDto.ts");

    // Codes, not sentences: the device has no French. Every variant must reach
    // the union, because `ui/src/api/errors.ts` maps it through a total Record
    // and a missing member there is a `tsc` failure rather than a blank screen.
    for variant in [
        r#""nameEmpty""#,
        r#""nameTooLong""#,
        r#""invalidKind""#,
        r#""invalidTiltMode""#,
        r#""travelTimeZero""#,
        r#""invalidAddress""#,
        r#""registryFull""#,
        r#""notFound""#,
        r#""addressNotAllocated""#,
        r#""valueTooLong""#,
        r#""namespacesOverlap""#,
        r#""secretNotSet""#,
        r#""trialNotAssociated""#,
        r#""settingsUnwritable""#,
    ] {
        assert!(
            code.contains(variant),
            "ApiErrorCode lost {variant}:\n{code}"
        );
    }
    assert!(dto.contains("code: ApiErrorCode"), "{dto}");
}

#[test]
fn command_dto_matches_action_tagged_wire() {
    regenerate();
    let ts = read("CommandDto.ts");

    // Unit actions carry only the tag: `{"action":"up"}`.
    assert!(
        ts.contains(r#"{ "action": "up" }"#),
        "CommandDto lost the bare action-tagged unit variant:\n{ts}"
    );
    assert!(ts.contains(r#"{ "action": "stepDown" }"#), "{ts}");

    // `goTo` REQUIRES a numeric position (a missing one is a hard error in the
    // manual deserializer), so the TS field must be non-nullable `number`.
    assert!(
        ts.contains(r#"{ "action": "goTo", position: number, }"#),
        "CommandDto goTo must require a numeric position:\n{ts}"
    );

    // `setMy` position is optional/clearable, so it must be `number | null`.
    assert!(
        ts.contains(r#"{ "action": "setMy", position: number | null, }"#),
        "CommandDto setMy must allow a nullable position:\n{ts}"
    );

    // `vent` carries **no** position, and that is the contract rather than an
    // omission: what it aims at is the shade's own measured slat-separation
    // band, so a caller has nothing to name. A `position` here would be a
    // second way to say where the shade should stop, free to disagree with the
    // one the command exists to depend on.
    assert!(
        ts.contains(r#"{ "action": "vent" }"#),
        "CommandDto vent must be a bare action:\n{ts}"
    );
}

/// The calibration conversation, on the wire.
#[test]
fn calibration_step_matches_step_tagged_wire() {
    regenerate();
    let ts = read("CalibrationStepDto.ts");

    // `begin` REQUIRES a leg: guessing a direction would drive a shade the
    // wrong way across its whole range.
    assert!(
        ts.contains(r#"{ "step": "begin", leg: CalibrationLegDto, }"#),
        "begin must require a leg:\n{ts}"
    );
    for bare in [r#"{ "step": "finish" }"#, r#"{ "step": "cancel" }"#] {
        assert!(ts.contains(bare), "CalibrationStepDto lost {bare}:\n{ts}");
    }

    // Three steps and no more. `mark` was a fourth until 2026-08-19, and this
    // is what keeps its removal from the *wire* honest: a union that still
    // carried it would let the mock and a stale tab go on describing a
    // measurement the device no longer takes.
    assert!(
        !ts.contains(r#""step": "mark""#),
        "the mark step is gone from the domain; it must be gone from the wire:\n{ts}"
    );

    // The two directions are separate values, because they are measured
    // separately and never mirrored — 30 s up against 27 s down on the estate
    // that produced the requirement.
    let leg = read("CalibrationLegDto.ts");
    for value in [r#""up""#, r#""down""#] {
        assert!(
            leg.contains(value),
            "CalibrationLegDto lost {value}:\n{leg}"
        );
    }
}

#[test]
fn ws_event_matches_ev_tagged_wire() {
    regenerate();
    let ws = read("WsEvent.ts");
    let ev = read("ShadeStateEvent.ts");

    // Flat, internally-tagged on `ev`: `{"ev":"shadeState", ...payload}`.
    assert!(
        ws.contains(r#"{ "ev": "shadeState" }"#),
        "WsEvent lost its `ev` discriminant:\n{ws}"
    );
    // The payload fields are merged in via the newtype variant's inner struct.
    assert!(
        ws.contains("& ShadeStateEvent"),
        "WsEvent must inline the ShadeStateEvent payload alongside the tag:\n{ws}"
    );

    // The payload keeps camelCase field names on the wire.
    assert!(ev.contains("tiltPosition: number"), "{ev}");
    assert!(ev.contains("direction: number"), "{ev}");
}

#[test]
fn entities_use_camelcase_and_heapless_overrides() {
    regenerate();
    let shade = read("ShadeDto.ts");
    let group = read("GroupDto.ts");
    let room = read("RoomDto.ts");

    // camelCase (serde rename_all) + heapless String override -> `string`.
    assert!(shade.contains("name: string"), "{shade}");
    assert!(shade.contains("tiltPosition: number"), "{shade}");
    assert!(shade.contains("myPosition: number | null"), "{shade}");
    // Numeric discriminants (reused from deployed devices' wire values) stay
    // numbers, not string unions.
    assert!(shade.contains("kind: number"), "{shade}");
    assert!(shade.contains("tiltMode: number"), "{shade}");

    // heapless `Vec<u8, 32>` override -> `number[]`, field renamed to camelCase.
    assert!(group.contains("shadeIds: number[]"), "{group}");
    assert!(room.contains("shadeIds: number[]"), "{room}");
}

#[test]
fn no_settings_type_the_device_sends_has_a_field_a_secret_could_go_in() {
    regenerate();

    // The type-level half of `tests/settings.rs`'s byte-level check. That one
    // proves this build does not send a secret; this one proves the *shape*
    // cannot, so a field added later is caught in the generated contract the UI
    // compiles against rather than only in a serialisation assertion.
    for name in [
        "SettingsDto.ts",
        "WifiSettingsDto.ts",
        "MqttSettingsDto.ts",
        "WifiTrialDto.ts",
    ] {
        let ts = declaration(name);
        for forbidden in ["psk", "password", "passphrase", "secret"] {
            assert!(
                !ts.to_lowercase().contains(forbidden),
                "{name} has a `{forbidden}` field; nothing the device sends may:\n{ts}"
            );
        }
    }

    // What replaces them.
    let wifi = read("WifiSettingsDto.ts");
    assert!(wifi.contains("pskSet: boolean"), "{wifi}");
    let mqtt = read("MqttSettingsDto.ts");
    assert!(mqtt.contains("passwordSet: boolean"), "{mqtt}");
}

#[test]
fn a_secret_update_is_tagged_so_the_ui_must_say_which_of_the_three_it_means() {
    regenerate();
    let ts = read("SecretDto.ts");
    assert!(ts.contains(r#"{ "secret": "keep" }"#), "{ts}");
    assert!(ts.contains(r#"{ "secret": "clear" }"#), "{ts}");
    assert!(
        ts.contains(r#"{ "secret": "set", value: string, }"#),
        "{ts}"
    );
}

#[test]
fn a_settings_rejection_carries_an_optional_field_the_form_can_highlight() {
    regenerate();
    let dto = read("ApiErrorDto.ts");
    assert!(dto.contains("field?: SettingsFieldDto"), "{dto}");

    let field = read("SettingsFieldDto.ts");
    for variant in [
        r#""ssid""#,
        r#""psk""#,
        r#""brokerAddress""#,
        r#""brokerPort""#,
        r#""brokerUsername""#,
        r#""brokerPassword""#,
        r#""discoveryPrefix""#,
        r#""stateRoot""#,
    ] {
        assert!(
            field.contains(variant),
            "SettingsFieldDto lost {variant}:\n{field}"
        );
    }
}

#[test]
fn the_three_settings_halves_are_nullable_because_none_is_a_value() {
    regenerate();
    let ts = read("SettingsDto.ts");
    // Not `ts(optional)`: a device with no broker must say so, and an absent
    // key would be indistinguishable from a firmware that did not know about
    // brokers.
    assert!(ts.contains("wifi: WifiSettingsDto | null"), "{ts}");
    assert!(ts.contains("mqtt: MqttSettingsDto | null"), "{ts}");
    assert!(ts.contains("wifiTrial: WifiTrialDto | null"), "{ts}");
}

#[test]
fn both_endings_of_a_trial_are_one_tagged_body() {
    regenerate();
    let ts = read("TrialDecisionDto.ts");
    assert!(ts.contains(r#"{ "decision": "confirm" }"#), "{ts}");
    assert!(ts.contains(r#"{ "decision": "cancel" }"#), "{ts}");
}
