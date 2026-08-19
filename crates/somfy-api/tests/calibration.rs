//! The wire form of `POST /api/v1/shades/{id}/calibrate`.
//!
//! # Why this file exists
//!
//! Every other DTO on this API has been driven by a real device. This one had
//! not: the guided travel-time measurement was built, exported to TypeScript,
//! modelled in the mock server and rendered by the web UI — and the **Rust**
//! parser at the far end had never seen a byte. `tests/ts_export.rs` checks the
//! TypeScript this enum generates, which is a statement about `ts-rs`, not about
//! [`CalibrationStepDto`]'s hand-written [`serde::Deserialize`].
//!
//! That matters more here than it would elsewhere, because the impl is
//! hand-written rather than derived. It exists to keep the firmware
//! allocator-free — serde's own internally-tagged deriving buffers the map into
//! a `Content` value first — so the flat [`CalibrationStepDto`] wire shape is
//! reassembled by code somebody typed, including the `missing_field` arm that is
//! the whole difference between a refused request and a shade driven the wrong
//! way across its range.
//!
//! # Both drivers, deliberately
//!
//! `serde_json` is what the rest of this crate's tests use and it is the
//! stricter reader. `serde-json-core` is what the **device** uses: picoserve's
//! `Json` extractor is built on it, so it is the only parser whose opinion
//! decides whether the owner's first calibration run reaches the domain at all.
//! The two are not interchangeable — `serde-json-core` implements no
//! `deserialize_any` — so a body is asserted against both, from one table.
//!
//! The literals below are the **exact** bytes `ui/src/api/client.ts`'s
//! `calibrateShade` puts on the wire, `JSON.stringify` of the generated
//! `CalibrationStepDto` union. They are pasted rather than constructed so that a
//! change on either side has to be made twice, on purpose.

use somfy_api::{CalibrationLegDto, CalibrationStepDto};
use somfy_domain::CalibrationLeg;

/// Parse with both drivers and assert they agree with each other and with
/// `expected`.
///
/// Returning nothing rather than the value: every caller here is asserting an
/// equality, and a helper that handed one back would invite a test that checked
/// only one of the two readers.
#[track_caller]
fn both(json: &str, expected: CalibrationStepDto) {
    let via_serde_json: CalibrationStepDto =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("serde_json refused {json}: {e}"));
    let (via_json_core, _consumed) =
        serde_json_core::from_slice::<CalibrationStepDto>(json.as_bytes()).unwrap_or_else(|e| {
            panic!("serde-json-core — the device's own driver — refused {json}: {e}")
        });

    assert_eq!(via_serde_json, expected, "serde_json, for {json}");
    assert_eq!(
        via_json_core, expected,
        "serde-json-core (the device's driver), for {json}"
    );
}

/// Assert both drivers refuse.
///
/// Both, because a body only one of them rejects is a body the device accepts
/// and the host tests call impossible — which is the failure mode this file was
/// written to make impossible.
#[track_caller]
fn neither(json: &str) {
    assert!(
        serde_json::from_str::<CalibrationStepDto>(json).is_err(),
        "serde_json accepted {json}"
    );
    assert!(
        serde_json_core::from_slice::<CalibrationStepDto>(json.as_bytes()).is_err(),
        "serde-json-core accepted {json}"
    );
}

/// The four bodies the web UI can send, byte for byte.
#[test]
fn every_step_the_ui_sends_parses_on_both_drivers() {
    let cases = [
        (
            r#"{"step":"begin","leg":"up"}"#,
            CalibrationStepDto::Begin {
                leg: CalibrationLegDto::Up,
            },
        ),
        (
            r#"{"step":"begin","leg":"down"}"#,
            CalibrationStepDto::Begin {
                leg: CalibrationLegDto::Down,
            },
        ),
        (r#"{"step":"finish"}"#, CalibrationStepDto::Finish),
        (r#"{"step":"cancel"}"#, CalibrationStepDto::Cancel),
    ];
    for (json, expected) in cases {
        both(json, expected);
    }
}

/// A `begin` with no direction is refused rather than defaulted.
///
/// This is the single most consequential arm in the parser. A default would pick
/// a direction for a request that named none, and the shade would run its whole
/// range the wrong way — the one outcome the endpoint's own docs say guessing
/// would cause.
#[test]
fn begin_without_a_leg_is_refused() {
    neither(r#"{"step":"begin"}"#);
    // Present but null is the same absence, and is the shape a client that
    // serialises `undefined` fields would produce.
    neither(r#"{"step":"begin","leg":null}"#);
}

/// Unknown tags and unknown values, both of which a mistyped client produces.
///
/// **`mark` is in this list deliberately, and it is the only entry that was once
/// real.** It was a step until 2026-08-19, so a browser tab left open across the
/// update, or a script somebody wrote against the old shape, can still send one
/// — and what it must get back is a refusal. Accepting it and ignoring it would
/// let a caller believe it had recorded a slat figure this device never stored,
/// which is the failure mode the whole panel exists to prevent.
#[test]
fn unknown_steps_legs_and_marks_are_refused() {
    neither(r#"{"step":"measure"}"#);
    neither(r#"{"step":"begin","leg":"sideways"}"#);
    neither(r#"{"step":"mark","mark":"motionBegan"}"#);
    neither(r#"{"step":"mark","mark":"curtainMoved"}"#);
    neither(r#"{"step":"mark"}"#);
    // The tag is camelCase on the wire; the Rust spelling is not accepted.
    neither(r#"{"step":"Begin","leg":"up"}"#);
}

/// A body with no `step` at all — the shape a `POST` with an empty object makes.
#[test]
fn a_body_with_no_step_is_refused() {
    neither("{}");
}

/// The fields the other steps carry are ignored where they do not apply rather
/// than making the body malformed.
///
/// Deliberate, and it is the choice `CommandDto` already makes for a `vent` that
/// offers a position: the step tag decides what is read, so a client that sends
/// one struct with every optional field set still names exactly one step. A
/// stricter reading would turn a harmless client into a 400 the operator cannot
/// act on.
#[test]
fn a_field_belonging_to_another_step_is_ignored() {
    both(
        r#"{"step":"finish","leg":"up"}"#,
        CalibrationStepDto::Finish,
    );
    both(
        r#"{"step":"cancel","leg":"down"}"#,
        CalibrationStepDto::Cancel,
    );
}

/// Field order is not significant, which a hand-written flat reader could
/// plausibly get wrong: the tag can arrive after the value it selects.
#[test]
fn the_tag_may_arrive_after_the_field_it_selects() {
    both(
        r#"{"leg":"down","step":"begin"}"#,
        CalibrationStepDto::Begin {
            leg: CalibrationLegDto::Down,
        },
    );
}

/// The lowering onto the domain, exhaustively.
///
/// Two lines, and they decide which way a motor runs across its whole range — a
/// transposition here would be invisible on the wire and visible only at a
/// window.
#[test]
fn legs_lower_onto_the_domain_without_transposing() {
    assert_eq!(CalibrationLegDto::Up.to_domain(), CalibrationLeg::Up);
    assert_eq!(CalibrationLegDto::Down.to_domain(), CalibrationLeg::Down);
}
