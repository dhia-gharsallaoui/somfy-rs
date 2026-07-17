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
    });
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
    // Numeric C++ discriminants stay numbers, not string unions.
    assert!(shade.contains("kind: number"), "{shade}");
    assert!(shade.contains("tiltMode: number"), "{shade}");

    // heapless `Vec<u8, 32>` override -> `number[]`, field renamed to camelCase.
    assert!(group.contains("shadeIds: number[]"), "{group}");
    assert!(room.contains("shadeIds: number[]"), "{room}");
}
