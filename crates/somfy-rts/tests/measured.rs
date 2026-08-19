//! What the wall-remote captures actually measure, as distinct from what the
//! transmit-side constants predict.
//!
//! [`TIMINGS`] describes the pulse train *this crate emits*. A receiver has to
//! cope with the pulse train a **real remote** emits, and the two are not the
//! same: the silence after the wake-up pulse is over twice as long on the real
//! device as `WAKEUP_LOW` says. Anything that has to bound a real transmission
//! — the RMT receiver's idle threshold above all — must be sized against the
//! captures, not against the transmit constants, so the captures' measurements
//! are pinned here as named values rather than left implicit in the files.
//!
//! These tests are the *derivation* of [`MEASURED_MAX_INTRA_FRAME_SEGMENT_US`]. If
//! a capture is ever re-taken, they fail rather than quietly let the constant
//! describe a remote nobody has any more.
//!
//! # The captures were anonymised on 2026-08-19, and this file is why they were
//! # not simply deleted
//!
//! Their pulse trains encoded the transmitting remote's own address, which is
//! not publishable. The payload was therefore replaced and the timing kept —
//! and the wake-up pulse, the silence after it and the sync structure, which is
//! everything this file measures, were **copied across verbatim**. So every
//! number asserted below is still a number a physical remote produced.
//! `fixtures/README.md` records the method; the alternative was deleting the
//! only evidence a shipping firmware constant has.

mod support;

use somfy_rts::{Pulse, MEASURED_MAX_INTRA_FRAME_SEGMENT_US, TIMINGS};
use support::load_fixture;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// Every capture whose timing came off the physical wall remote.
///
/// `synthetic_up_56bit.pulses` is deliberately absent: it is rendered from this
/// crate's own [`TIMINGS`], so measuring it would only re-measure the constants
/// this file exists to distrust. It also carries a trailing `INTER_FRAME_GAP`,
/// which is the *separator between* transmissions and would be counted here as
/// if it sat inside one.
const CAPTURED_TIMING: [&str; 3] = [
    "anonymised_up_56bit_1.pulses",
    "anonymised_down_56bit_1.pulses",
    "anonymised_my_56bit_1.pulses",
];

fn capture(name: &str) -> std::vec::Vec<Pulse> {
    load_fixture(FIXTURES, name)
}

/// The longest segment of either level in any real capture, taken from the
/// files themselves.
///
/// Level-agnostic on purpose: the hardware rule this measurement feeds ends a
/// reception when no *edge* arrives for long enough, so a long HIGH counts
/// exactly as a long LOW does.
fn measured_max_segment() -> u32 {
    CAPTURED_TIMING
        .iter()
        .flat_map(|name| capture(name))
        .map(|pulse| pulse.micros)
        .max()
        .expect("the captures contain segments")
}

fn measured_max(high: bool) -> u32 {
    CAPTURED_TIMING
        .iter()
        .flat_map(|name| capture(name))
        .filter(|pulse| pulse.high == high)
        .map(|pulse| pulse.micros)
        .max()
        .expect("the captures contain segments of both levels")
}

/// The constant is the largest segment any real capture contains. Pinned with
/// `==`, not `<=`: it is a measurement, and a measurement that drifted away
/// from its evidence while still satisfying an inequality is exactly the
/// failure this file is here to prevent.
#[test]
fn measured_max_intra_frame_segment_is_the_largest_segment_in_any_real_capture() {
    assert_eq!(
        MEASURED_MAX_INTRA_FRAME_SEGMENT_US,
        measured_max_segment(),
        "re-derive the constant from the captures rather than adjusting the captures"
    );
}

/// Which level that longest segment is, and by how much it beats the other.
///
/// Recorded because the constant's name deliberately does *not* say "LOW", and
/// a reader who checks will find that the maximum is in fact a LOW today. The
/// margin is what makes the level-agnostic definition currently free: the
/// longest HIGH is the ~10.2 ms wake-up pulse, so covering both levels costs
/// nothing and removes a way for a future capture to slip past the bound.
#[test]
fn the_longest_segment_is_a_low_but_the_longest_high_is_not_far_behind_the_family() {
    let longest_low = measured_max(false);
    let longest_high = measured_max(true);

    assert_eq!(measured_max_segment(), longest_low, "the maximum is a LOW");
    assert!(
        longest_high < longest_low,
        "wake-up HIGH {longest_high} µs vs wake-up gap {longest_low} µs"
    );
    // The wake-up pulse, and nothing else in the frame, is in that league.
    assert!(longest_high > 4 * TIMINGS::HW_SYNC_HALF);
}

/// Everything in these files is inside one transmission. The capture ISR stops
/// recording once a frame completes, so no committed capture contains the
/// silence that separates one frame from the next — which is why the *upper*
/// bound on an idle threshold cannot be measured here and has to be inferred
/// from this crate's own `INTER_FRAME_GAP`.
///
/// Asserted structurally rather than assumed: each capture holds exactly one
/// long LOW, it is the wake-up gap at index 1, and every other LOW belongs to
/// the hardware-sync family or shorter. A capture that had run on into the next
/// frame would break that shape and would make the measurement above mean
/// something else entirely.
///
/// Note this does *not* also assert that `pulses[0]` is HIGH. It is, but only
/// because the loader reconstructs levels by alternation starting from HIGH for
/// a durations-only file, so asserting it would test the loader's opening
/// assumption rather than anything about the capture.
#[test]
fn each_real_capture_holds_exactly_one_long_low_and_it_is_the_wakeup_gap() {
    // The longest LOW that is a *normal* part of a frame: a hardware-sync half
    // at the top of the decoder's tolerance window.
    let in_family_max = TIMINGS::HW_SYNC_HALF + TIMINGS::HW_SYNC_HALF / 4;

    for name in CAPTURED_TIMING {
        let long: std::vec::Vec<usize> = capture(name)
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.high && p.micros > in_family_max)
            .map(|(i, _)| i)
            .collect();

        assert_eq!(long, std::vec![1], "{name}: long LOWs at {long:?}");
    }
}

/// The guard against the one way anonymising these files could have destroyed
/// them: leaving behind this crate's own renderer's output under a capture's
/// name.
///
/// A fixture like that would decode, pass every other test in this workspace,
/// and validate nothing `synthetic_up_56bit.pulses` does not — it would be
/// decoration. The evidence that it did not happen is simply that the numbers
/// are still *wrong* in the way real hardware is wrong: [`render_pulses`] emits
/// `HALF_SYMBOL` and `HW_SYNC_HALF` exactly, and a transmitter with a real
/// oscillator never does.
///
/// Both halves of the file are checked, because they were preserved by
/// different means (see `fixtures/README.md`): the preamble was copied across
/// verbatim, and the body was rebuilt from this same capture's own measured
/// half-symbol deviations.
#[test]
fn the_captures_still_carry_a_real_transmitters_timing_rather_than_our_own_constants() {
    // A rendered body scores exactly zero here. The three captures score 1,747
    // (up), 888 (my) and 1,929 (down) µs — `my` is the low one because its
    // measured deviations are the most tightly clustered of the three, so more
    // of them cancel inside a merged segment. The floor is two-thirds of that
    // smallest figure: low enough that it is not a second re-measurement of the
    // files, high enough that nominal output cannot reach it.
    const MIN_BODY_DRIFT_US: u32 = 600;

    for name in CAPTURED_TIMING {
        let pulses = capture(name);
        let sync = pulses
            .iter()
            .position(|pulse| {
                pulse.high
                    && pulse.micros.abs_diff(TIMINGS::SW_SYNC_HIGH) <= TIMINGS::SW_SYNC_HIGH / 4
            })
            .unwrap_or_else(|| panic!("{name}: no software sync"));

        for (index, pulse) in pulses[..=sync].iter().enumerate() {
            let nominal = match index {
                0 => TIMINGS::WAKEUP_HIGH,
                1 => TIMINGS::WAKEUP_LOW,
                i if i == sync => TIMINGS::SW_SYNC_HIGH,
                _ => TIMINGS::HW_SYNC_HALF,
            };
            assert_ne!(
                pulse.micros, nominal,
                "{name}: preamble segment {index} is exactly what we would have rendered"
            );
        }

        let drift: u32 = pulses[sync + 1..]
            .iter()
            .map(|pulse| {
                let halves = (pulse.micros + TIMINGS::HALF_SYMBOL / 2) / TIMINGS::HALF_SYMBOL;
                pulse.micros.abs_diff(halves * TIMINGS::HALF_SYMBOL)
            })
            .sum();
        assert!(
            drift > MIN_BODY_DRIFT_US,
            "{name}: the body drifts only {drift} µs from nominal, which is what a rendered \
             frame would look like"
        );
    }
}

/// The reason this constant has to exist at all, asserted against the captures
/// rather than against the constant so it stays a statement about hardware.
///
/// A real remote's post-wake-up silence is more than twice `WAKEUP_LOW`, so the
/// bound the transmit constants imply — "the longest LOW inside a frame is
/// `WAKEUP_LOW`" — is false against the hardware a receiver has to hear. It
/// still lands below `INTER_FRAME_GAP`, which is what leaves any room at all
/// for a receiver to tell "inside a frame" from "between frames".
#[test]
fn the_captures_disagree_with_the_transmit_constants_but_stay_inside_the_window() {
    let measured = measured_max_segment();
    assert!(
        measured > 2 * TIMINGS::WAKEUP_LOW,
        "measured {measured} µs vs WAKEUP_LOW {} µs",
        TIMINGS::WAKEUP_LOW,
    );
    assert!(
        measured < TIMINGS::INTER_FRAME_GAP,
        "measured {measured} µs vs INTER_FRAME_GAP {} µs",
        TIMINGS::INTER_FRAME_GAP,
    );
}
