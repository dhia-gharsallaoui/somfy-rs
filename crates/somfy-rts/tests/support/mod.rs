//! Loader for the `.pulses` golden captures, shared by every test that replays
//! them.
//!
//! It lives beside the fixtures it parses and is pulled into
//! `crates/somfy-rmt/tests/replay.rs` with `#[path]`. That cross-crate include
//! is deliberate: a second copy of these reconstruction rules could drift from
//! this one, and a loader that quietly disagrees with the one that produced the
//! pinned expectations is worse than having no second test at all. The
//! alternative — exporting the parser from the `somfy-rts` library — would put
//! `std` file parsing into a `no_std` crate's public API for no runtime purpose.

// Each including test binary uses a different subset of this module.
#![allow(dead_code)]

use somfy_rts::Pulse;

/// Glitch threshold: `HALF_SYMBOL * 0.7 = 640 * 0.7 = 448`. Sub-threshold
/// pulses are logged by the capture ISR but never advance its edge clock, so
/// the loader must drop them outright rather than merge their duration into
/// the next pulse.
pub const GLITCH_MIN_US: u32 = 448;

/// Parse a `.pulses` file body into levelled [`Pulse`]s.
///
/// Accepts both supported formats (see `fixtures/README.md`):
/// - `<duration_us>` per line — levels reconstructed by alternation from HIGH.
/// - `<level 0|1>,<duration_us>` per line — levels taken verbatim.
///
/// Blank lines and `#` comments are skipped. Sub-448µs entries are dropped as
/// glitches *before* level reconstruction, so the surviving edges keep their
/// strict HIGH/LOW alternation.
pub fn parse_pulses(contents: &str) -> std::vec::Vec<Pulse> {
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

/// Read and parse one capture out of `dir`.
///
/// The directory is passed in rather than derived from `CARGO_MANIFEST_DIR`
/// because the including crate is not always the one that owns the fixtures.
pub fn load_fixture(dir: &str, name: &str) -> std::vec::Vec<Pulse> {
    let path = format!("{dir}/{name}");
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    parse_pulses(&contents)
}
