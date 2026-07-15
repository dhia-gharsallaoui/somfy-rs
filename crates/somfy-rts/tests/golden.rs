//! Golden capture tests: decode real (and one synthetic) pulse streams the way
//! the C++ reference firmware records them.
//!
//! The three `up`/`down`/`my` tests are `#[ignore]` until the author supplies
//! real device captures (see `tests/fixtures/README.md`); the suite stays green
//! without hardware. The remaining tests are always run and validate the loader
//! itself — parsing, level reconstruction, glitch filtering, and end-to-end
//! decode — against a checked-in synthetic fixture.

use heapless::Vec as HVec;
use somfy_rts::{decode56, encode56, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder};

/// Glitch threshold. C++ `bitMin = SYMBOL * TOLERANCE_MIN = 640 * 0.7 = 448`
/// (`Somfy.cpp:4238`); the ISR logs shorter segments into `rx.pulses[]` but
/// never advances `last_time` for them (`Somfy.cpp:4388-4395`), so the loader
/// drops them without merging their duration into the next pulse.
const GLITCH_MIN_US: u32 = 448;

/// Parse a `.pulses` file body into levelled [`Pulse`]s.
///
/// Accepts both supported formats (see `tests/fixtures/README.md`):
/// - `<duration_us>` per line — levels reconstructed by alternation from HIGH.
/// - `<level 0|1>,<duration_us>` per line — levels taken verbatim.
///
/// Blank lines and `#` comments are skipped. Sub-448µs entries are dropped as
/// glitches *before* level reconstruction, so the surviving edges keep their
/// strict HIGH/LOW alternation.
fn parse_pulses(contents: &str) -> std::vec::Vec<Pulse> {
    // Pass 1: parse and drop glitches, remembering an explicit level if given.
    let mut kept: std::vec::Vec<(Option<bool>, u32)> = std::vec::Vec::new();
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (level, micros) = match line.split_once(',') {
            Some((lvl, us)) => (
                Some(lvl.trim() == "1"),
                us.trim().parse::<u32>().expect("parse duration"),
            ),
            None => (None, line.parse::<u32>().expect("parse duration")),
        };
        if micros < GLITCH_MIN_US {
            continue;
        }
        kept.push((level, micros));
    }
    // Pass 2: fill missing levels by alternation from HIGH (first pulse HIGH).
    let mut phase_high = true;
    kept.into_iter()
        .map(|(level, micros)| {
            let high = level.unwrap_or(phase_high);
            phase_high = !phase_high;
            Pulse { high, micros }
        })
        .collect()
}

fn load(name: &str) -> std::vec::Vec<Pulse> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    parse_pulses(&contents)
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
#[ignore = "requires real-device captures — see tests/fixtures/README.md"]
fn golden_up_capture_decodes_as_up() {
    assert_eq!(decode_capture("up_56bit_1.pulses").command, Command::Up);
}

#[test]
#[ignore = "requires real-device captures — see tests/fixtures/README.md"]
fn golden_down_capture_decodes_as_down() {
    assert_eq!(decode_capture("down_56bit_1.pulses").command, Command::Down);
}

#[test]
#[ignore = "requires real-device captures — see tests/fixtures/README.md"]
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
    render_pulses(&encode56(&f), FrameKind::Repeat, &mut raw);
    let mut merged: std::vec::Vec<Pulse> = std::vec::Vec::new();
    for p in &raw {
        if let Some(last) = merged.last_mut() {
            if last.high == p.high {
                last.micros += p.micros;
                continue;
            }
        }
        merged.push(*p);
    }
    merged
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
