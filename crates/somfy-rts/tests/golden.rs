//! Golden capture tests: decode pulse streams in the edge-to-edge form a
//! physical receiver actually produces.
//!
//! The three `up`/`down`/`my` tests run against the **anonymised** wall-remote
//! captures: a real remote's measured timing carrying a payload this project
//! substituted, because the original payload was that remote's own address.
//! `tests/fixtures/README.md` records exactly which numbers are measured and
//! which are not, and it is the file to read before trusting anything here.
//!
//! **What they still pin, and it is most of it.** The timing constants, the
//! Manchester polarity, the level reconstruction, the hardware-sync counting
//! and the ±25% tolerance windows are all exercised against durations a real
//! transmitter produced — none of which this crate's renderer would emit,
//! because it emits nominal ones.
//!
//! **What they no longer pin.** The bits are this project's encoder's, so a
//! capture can no longer show that our checksum and de-obfuscation agree with
//! Somfy's. The bytes were frozen when the file was written, so these tests do
//! still catch a change to `deobfuscate`, to `checksum`, or to the bit order —
//! all three were confirmed by breaking them — but that is regression cover
//! over the *decode* path, not interoperability evidence.
//!
//! And be precise about the edge of that: nothing here calls `encode56`, so a
//! change to `obfuscate` **alone** passes every test in this file. It is caught
//! by `loader_reconstructs_levels_and_drops_glitch` below, which renders the
//! synthetic fixture in memory and compares — which is why that test is not
//! merely loader coverage and should not be deleted as redundant.
//!
//! The remaining tests validate the loader itself — parsing, level
//! reconstruction, glitch filtering, and end-to-end decode — against a
//! checked-in synthetic fixture.

mod support;

use heapless::Vec as HVec;
use somfy_rts::{
    decode56, encode56, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder,
    TIMINGS,
};
use support::{load_fixture, parse_pulses};

/// Where this crate's own captures live. `somfy-rmt` replays the same files
/// from its own tests and passes its own path here.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// Every anonymised capture decodes to this address. It is this project's
/// bring-up value and names no remote — see `tests/fixtures/README.md`.
const SYNTHETIC_ADDRESS: u32 = 0x00C0DE;

/// A first frame carries two hardware syncs, which an edge-driven receiver sees
/// as four segments. Asserted rather than assumed because the count is what
/// [`RxDecoder`] selects the frame length from, and because the *anonymisation*
/// copied these four segments across verbatim: if they ever stopped being four,
/// the file would no longer be the capture it claims to be.
const HW_SYNC_SEGMENTS: usize = 4;

fn load(name: &str) -> std::vec::Vec<Pulse> {
    load_fixture(FIXTURES, name)
}

/// Segments in the hardware-sync duration family, counted the way the decoder
/// counts them: every half-pulse of either level.
fn hw_sync_segments(pulses: &[Pulse]) -> usize {
    let tolerance = TIMINGS::HW_SYNC_HALF / 4;
    pulses
        .iter()
        .filter(|p| {
            p.micros >= TIMINGS::HW_SYNC_HALF - tolerance
                && p.micros <= TIMINGS::HW_SYNC_HALF + tolerance
        })
        .count()
}

/// Decode any fixture, asserting the two things every 56-bit file must show:
/// the length the sync pattern selected, and a checksum that verifies.
fn decode_fixture(name: &str) -> Frame {
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in load(name) {
        if let Some(fr) = rx.push(p) {
            got = Some(fr);
        }
    }
    let raw = got.unwrap_or_else(|| panic!("no frame decoded from {name}"));
    assert_eq!(raw.bit_length, 56, "{name}: detected frame length");
    assert_eq!(raw.bytes.len(), 7, "{name}: 56 bits is seven bytes");

    // `decode56` returns `BadChecksum` rather than a frame when the nibble
    // checksum over the de-obfuscated bytes is non-zero, so this is the
    // checksum assertion — spelled out because it is easy to read `unwrap` as
    // ceremony.
    decode56(raw.bytes.as_slice().try_into().unwrap())
        .unwrap_or_else(|error| panic!("{name}: checksum or command invalid: {error:?}"))
}

/// Decode one of the three anonymised wall-remote captures, additionally
/// asserting what makes it a *capture*: the first-frame sync structure that was
/// copied across from the original, and the substituted address.
///
/// One helper rather than three tests' worth of repetition, because every one of
/// these is a property of all three files and a capture that lost any of them
/// has stopped being useful whichever button it recorded.
fn decode_capture(name: &str) -> Frame {
    assert_eq!(
        hw_sync_segments(&load(name)),
        HW_SYNC_SEGMENTS,
        "{name}: a first frame's hardware syncs"
    );
    let frame = decode_fixture(name);
    assert_eq!(
        frame.address, SYNTHETIC_ADDRESS,
        "{name}: substituted address"
    );
    frame
}

#[test]
fn golden_up_capture_decodes_as_up() {
    let frame = decode_capture("anonymised_up_56bit_1.pulses");
    assert_eq!(frame.command, Command::Up);
    assert_eq!(frame.rolling_code, 1);
}

#[test]
fn golden_down_capture_decodes_as_down() {
    let frame = decode_capture("anonymised_down_56bit_1.pulses");
    assert_eq!(frame.command, Command::Down);
    assert_eq!(frame.rolling_code, 3);
}

#[test]
fn golden_my_capture_decodes_as_my() {
    let frame = decode_capture("anonymised_my_56bit_1.pulses");
    assert_eq!(frame.command, Command::My);
    assert_eq!(frame.rolling_code, 2);
}

/// The key byte is the one payload byte the anonymisation kept, and the three
/// files should still show what the remote did with it: increment it once per
/// press, in the order the buttons were pressed.
///
/// Worth an assertion because it is the one *cross-file* fact that survived, and
/// because it contradicts this crate's own [`somfy_rts::RollingCode`], which
/// derives the key from the rolling code's low nibble. A real remote's two
/// counters ran in lockstep at a constant offset instead — see
/// `docs/provenance.md`.
#[test]
fn the_key_byte_increments_once_per_press_across_the_three_captures() {
    let up = decode_capture("anonymised_up_56bit_1.pulses").key;
    let my = decode_capture("anonymised_my_56bit_1.pulses").key;
    let down = decode_capture("anonymised_down_56bit_1.pulses").key;

    assert_eq!(my, up + 1, "the second press");
    assert_eq!(down, my + 1, "the third press");
    assert_eq!(up & 0xF0, 0xA0, "the high nibble a remote sends");
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
    let frame = decode_fixture("synthetic_up_56bit.pulses");
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
