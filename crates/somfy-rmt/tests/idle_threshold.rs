//! Where the RMT receiver is told one transmission ends and the next begins.
//!
//! The peripheral finishes a reception when the input has held one level for
//! longer than its idle threshold. That single number decides whether a frame
//! arrives whole, and it is the one receive-side constant that cannot be
//! checked by looking at the transmitter: pick it too low and every reception
//! is cut in two at a gap that is a normal part of a real remote's frame; pick
//! it too high and consecutive frames arrive glued together in a buffer that
//! was never sized for them.
//!
//! These tests model the hardware rule on the host and run committed
//! wall-remote captures through it, so the choice is settled against what a
//! physical remote actually emits rather than against what this crate emits.
//!
//! The fixtures were anonymised on 2026-08-19 — their payload is substituted,
//! their timing is not. **Everything this file turns on survived intact**: the
//! split points are decided by the wake-up pulse and the ~17.7 ms gap after it,
//! and those two durations were copied across verbatim. See
//! `../../somfy-rts/tests/fixtures/README.md`.

// The captures live in `somfy-rts`, next to the decoder they were taken to pin,
// and are shared with its own golden tests so both crates reconstruct levels
// and drop glitches by identical rules — see that module's header.
#[path = "../../somfy-rts/tests/support/mod.rs"]
mod support;

use heapless::Vec;
use somfy_rmt::{IDLE_THRESHOLD_US, MAX_SYMBOLS};
use somfy_rts::{
    decode56, encode56, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder,
    MEASURED_MAX_INTRA_FRAME_SEGMENT_US, TIMINGS,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../somfy-rts/tests/fixtures");

/// The fixtures whose timing came off the physical wall remote. The synthetic
/// one is excluded on purpose: it is rendered from this project's own transmit
/// constants, so it can only confirm that the threshold agrees with `TIMINGS` —
/// which is the reasoning this file exists to replace.
const CAPTURED_TIMING: [&str; 3] = [
    "anonymised_up_56bit_1.pulses",
    "anonymised_down_56bit_1.pulses",
    "anonymised_my_56bit_1.pulses",
];

/// The threshold the design spec originally specified, derived from
/// `WAKEUP_LOW` rather than from a capture. Kept here only to pin what it would
/// have done.
const SPEC_THRESHOLD_US: u32 = 12_000;

/// Split a pulse stream the way an RMT receiver with this idle threshold would.
///
/// The hardware rule is level-agnostic — reception ends when *no edge* arrives
/// for longer than `idle_threshold` ticks — so a long HIGH terminates a
/// reception just as a long LOW does.
///
/// ## What this model does and does not claim
///
/// The split points are the part worth trusting. Two details around them are
/// modelled from the documented behaviour rather than measured, and no assertion
/// in this file should be read as evidence for either:
///
/// - **The terminating segment is dropped**, on the grounds that the receiver
///   stops partway through it and writes an end marker instead of recording it.
///   If the peripheral does record it, every burst gains a trailing segment
///   longer than the threshold — which `RxDecoder` would discard in
///   `WaitingSync` anyway, so the decode assertions hold either way.
/// - **The next burst begins at the very next pulse.** On real hardware there is
///   a re-arm gap between one reception finishing and the next being issued, and
///   pulses arriving inside it are lost. That gap is exactly what would eat the
///   opening of a following frame, and nothing on the host can size it.
fn bursts(pulses: &[Pulse], idle_threshold_us: u32) -> std::vec::Vec<std::vec::Vec<Pulse>> {
    let mut out: std::vec::Vec<std::vec::Vec<Pulse>> = std::vec::Vec::new();
    let mut current: std::vec::Vec<Pulse> = std::vec::Vec::new();
    for pulse in pulses {
        if pulse.micros > idle_threshold_us {
            if !current.is_empty() {
                out.push(core::mem::take(&mut current));
            }
        } else {
            current.push(*pulse);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Symbols a burst of this many edges would occupy in a reception buffer: one
/// entry per edge plus the peripheral's terminator, two entries per symbol.
fn symbols_for(burst: &[Pulse]) -> usize {
    (burst.len() + 1).div_ceil(2)
}

fn decode_all(pulses: &[Pulse]) -> std::vec::Vec<Frame> {
    let mut rx = RxDecoder::new();
    let mut frames = std::vec::Vec::new();
    for pulse in pulses {
        if let Some(raw) = rx.push(*pulse) {
            let bytes = raw.bytes.as_slice().try_into().expect("56-bit payload");
            frames.push(decode56(bytes).expect("checksum must verify"));
        }
    }
    frames
}

fn capture(name: &str) -> std::vec::Vec<Pulse> {
    support::load_fixture(FIXTURES, name)
}

/// One 56-bit transmission as this crate emits it: a first frame followed by
/// one repeat, merged edge-to-edge the way a receiver sees it.
fn first_plus_repeat() -> std::vec::Vec<Pulse> {
    let frame = Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 0x000A,
        address: 0x00C0DE,
    };
    let bytes = encode56(&frame).unwrap();
    let mut out = std::vec::Vec::new();
    for kind in [FrameKind::First, FrameKind::Repeat] {
        let mut rendered: Vec<Pulse, 320> = Vec::new();
        render_pulses(&bytes, kind, &mut rendered);
        let mut merged: Vec<Pulse, 320> = Vec::new();
        merge_pulses(&rendered, &mut merged);
        out.extend(merged.iter().copied());
    }
    out
}

// The window the threshold has to sit in is a relation between constants, so
// it is asserted where constants are checked — at compile time, in the crate
// that owns the value. Repeating it here as a runtime test would only restate
// something the build already refuses to get wrong. What this file adds is the
// part no inequality can express: what those numbers do to real pulse trains.

/// The property that matters most: a real remote's frame arrives as one
/// reception, not two.
#[test]
fn no_capture_is_split_by_the_chosen_threshold() {
    for name in CAPTURED_TIMING {
        let pulses = capture(name);
        let bursts = bursts(&pulses, IDLE_THRESHOLD_US);
        assert_eq!(
            bursts.len(),
            1,
            "{name} was split into {} parts",
            bursts.len()
        );
        assert_eq!(bursts[0], pulses, "{name}: the burst is the whole capture");
        assert_eq!(decode_all(&bursts[0]).len(), 1, "{name} must still decode");
        // Splitting in the right place is only half of it: the piece has to fit
        // a reception buffer, or the firmware drops the whole burst.
        assert!(
            symbols_for(&bursts[0]) <= MAX_SYMBOLS,
            "{name} needs {} symbols to receive",
            symbols_for(&bursts[0])
        );
    }
}

/// Why the spec's original figure was wrong, pinned rather than described.
///
/// 12,000 µs was chosen from `WAKEUP_LOW` (7357 µs) on the assumption that no
/// longer silence occurs inside a frame. A real remote's post-wake-up gap is
/// ~17.7 ms, so the threshold lands *inside* every real first frame and ends
/// the reception one pulse in.
///
/// The damage is mild — the discarded fragment is only the wake-up pulse, and
/// the decoder re-acquires on the hardware syncs that open the second fragment,
/// which this test also shows — but a bound the hardware does not respect is
/// not a bound, and a compile-time assertion resting on it would have been
/// asserting something the committed fixtures already contradict.
#[test]
fn the_specs_original_threshold_would_split_every_capture() {
    for name in CAPTURED_TIMING {
        let pulses = capture(name);
        let bursts = bursts(&pulses, SPEC_THRESHOLD_US);
        assert_eq!(bursts.len(), 2, "{name}");
        assert_eq!(
            bursts[0],
            std::vec![pulses[0]],
            "{name}: the lost fragment is the wake-up pulse alone"
        );
        assert_eq!(decode_all(&bursts[1]).len(), 1, "{name}: the rest decodes");
    }
}

/// The other half of the job: consecutive frames must be *split* into separate
/// receptions. A first frame and its repeat carry an inter-frame gap between
/// them, and the threshold has to sit below it.
///
/// Named for the split, not for what a receiver would then decode. Each piece
/// carries a whole frame and each fits a reception buffer, which is everything
/// the threshold is responsible for — but whether a real receiver decodes the
/// *second* one depends on the re-arm gap this model does not have (see
/// `bursts`), and no assertion here can speak to that.
#[test]
fn a_first_frame_and_its_repeat_are_split_into_two_bursts_that_each_carry_a_frame() {
    let stream = first_plus_repeat();
    let bursts = bursts(&stream, IDLE_THRESHOLD_US);
    assert_eq!(bursts.len(), 2, "one reception per frame");

    let frames: std::vec::Vec<Frame> = bursts.iter().flat_map(|b| decode_all(b)).collect();
    assert_eq!(frames.len(), 2, "each burst carries one frame");
    assert_eq!(frames[0], frames[1], "a repeat re-sends the same frame");
    assert_eq!(frames[0].command, Command::Up);

    for (i, burst) in bursts.iter().enumerate() {
        assert!(
            symbols_for(burst) <= MAX_SYMBOLS,
            "burst {i} needs {} symbols to receive",
            symbols_for(burst)
        );
    }
}

/// And the failure the ceiling exists to prevent, shown rather than described.
///
/// If the threshold were above the inter-frame gap, the same two frames would
/// arrive as one reception — and that reception does not fit a buffer, so the
/// firmware would drop the pair entirely rather than decode either. This is why
/// the upper bound is not merely cosmetic, and why a real remote's repeat gap is
/// worth measuring: it is inferred here, not known.
#[test]
fn a_threshold_above_the_inter_frame_gap_would_merge_the_pair_past_the_buffer() {
    let stream = first_plus_repeat();
    let merged = bursts(&stream, TIMINGS::INTER_FRAME_GAP + TIMINGS::HALF_SYMBOL);

    assert_eq!(merged.len(), 1, "both frames in one reception");
    assert!(
        symbols_for(&merged[0]) > MAX_SYMBOLS,
        "the merged reception would need {} symbols, which is why this must not happen",
        symbols_for(&merged[0])
    );
}

/// Our own transmitter is comfortably inside the window too — it is only the
/// weaker constraint, because its longest intra-frame silence is less than half
/// a real remote's.
#[test]
fn our_own_transmitted_frames_contain_no_silence_near_the_threshold() {
    let stream = first_plus_repeat();
    let longest_within = stream
        .iter()
        .map(|p| p.micros)
        .filter(|micros| *micros < IDLE_THRESHOLD_US)
        .max()
        .expect("a rendered burst has pulses");

    assert_eq!(
        longest_within,
        TIMINGS::WAKEUP_HIGH,
        "the longest in-frame segment we emit is the wake-up pulse"
    );
    assert!(longest_within < MEASURED_MAX_INTRA_FRAME_SEGMENT_US);
}
