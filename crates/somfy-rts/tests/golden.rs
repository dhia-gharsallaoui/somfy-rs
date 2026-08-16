//! Golden capture tests: decode real (and one synthetic) pulse streams in the
//! edge-to-edge form a physical receiver actually produces.
//!
//! The three `up`/`down`/`my` tests run against real captures taken from a
//! physical Somfy wall remote on 2026-08-15 (see `tests/fixtures/README.md`).
//! They pin the engine's timing constants, Manchester polarity, sync-count
//! detection, checksum and XOR de-obfuscation against genuine hardware output.
//! The remaining tests validate the loader itself — parsing, level
//! reconstruction, glitch filtering, and end-to-end decode — against a
//! checked-in synthetic fixture, so the loader stays covered even if the real
//! captures are ever swapped out.

mod support;

use heapless::Vec as HVec;
use somfy_rts::{
    decode56, encode56, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder,
};
use support::{load_fixture, parse_pulses};

/// Where this crate's own captures live. `somfy-rmt` replays the same files
/// from its own tests and passes its own path here.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn load(name: &str) -> std::vec::Vec<Pulse> {
    load_fixture(FIXTURES, name)
}

fn decode_capture(name: &str) -> Frame {
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in load(name) {
        if let Some(fr) = rx.push(p) {
            got = Some(fr);
        }
    }
    let fr = got.unwrap_or_else(|| panic!("no frame decoded from {name}"));
    decode56(fr.bytes.as_slice().try_into().unwrap()).unwrap()
}

#[test]
fn golden_up_capture_decodes_as_up() {
    assert_eq!(decode_capture("up_56bit_1.pulses").command, Command::Up);
}

#[test]
fn golden_down_capture_decodes_as_down() {
    assert_eq!(decode_capture("down_56bit_1.pulses").command, Command::Down);
}

#[test]
fn golden_my_capture_decodes_as_my() {
    assert_eq!(decode_capture("my_56bit_1.pulses").command, Command::My);
}

/// Reproduce the checked-in synthetic fixture in memory: render an `Up` repeat
/// frame and merge adjacent same-level half-symbols into the edge-to-edge form
/// a `CHANGE`-interrupt receiver produces (see `tests/fixtures/README.md`).
fn synthetic_up_pulses() -> std::vec::Vec<Pulse> {
    let f = Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 0x000A,
        address: 0x00C0DE,
    };
    let mut raw: HVec<Pulse, 320> = HVec::new();
    render_pulses(&encode56(&f).unwrap(), FrameKind::Repeat, &mut raw);
    let mut merged: HVec<Pulse, 320> = HVec::new();
    merge_pulses(&raw, &mut merged);
    merged.iter().copied().collect()
}

/// End-to-end without hardware: a durations-only file (with comments, a blank
/// line, and an injected glitch) flows through the loader, the RX decoder, and
/// `decode56` back to the original command.
#[test]
fn loader_decodes_synthetic_capture_end_to_end() {
    let frame = decode_capture("synthetic_up_56bit.pulses");
    assert_eq!(frame.command, Command::Up);
    assert_eq!(frame.key, 0xA7);
    assert_eq!(frame.rolling_code, 0x000A);
    assert_eq!(frame.address, 0x00C0DE);
}

/// The loaded pulses must equal the in-memory merged form: this proves level
/// reconstruction from HIGH is correct and the injected sub-448µs glitch was
/// dropped (neither kept nor merged into a neighbour).
#[test]
fn loader_reconstructs_levels_and_drops_glitch() {
    let loaded = load("synthetic_up_56bit.pulses");
    let expected = synthetic_up_pulses();
    assert_eq!(loaded.len(), expected.len(), "glitch line must be dropped");
    assert_eq!(loaded, expected);
    assert!(loaded[0].high, "first reconstructed pulse must be HIGH");
}

/// Both file formats parse identically, glitches are filtered from each, and
/// the explicit-level parser honours the written level instead of
/// reconstructing it.
#[test]
fn parse_pulses_handles_both_formats_and_filters_glitches() {
    let expected = std::vec![
        Pulse {
            high: true,
            micros: 640,
        },
        Pulse {
            high: false,
            micros: 1280,
        },
        Pulse {
            high: true,
            micros: 640,
        },
    ];
    // Durations-only: levels alternate from HIGH; the 200µs line is a glitch.
    let durations_only = "# comment\n\n640\n200\n1280\n640\n";
    assert_eq!(parse_pulses(durations_only), expected);
    // Level+duration: levels honoured verbatim; 200µs line still dropped.
    let with_levels = "1,640\n0,200\n0,1280\n1,640\n";
    assert_eq!(parse_pulses(with_levels), expected);
}
