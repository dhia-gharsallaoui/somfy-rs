use heapless::Vec;
use somfy_rts::{encode56, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse, TIMINGS};

fn bytes() -> [u8; 7] {
    encode56(&Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 7,
        address: 0xAABBCC,
    })
    .unwrap()
}

#[test]
fn first_frame_starts_with_wakeup_then_two_hw_syncs() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::First, &mut out);
    assert!(out[0].high && out[0].micros == TIMINGS::WAKEUP_HIGH);
    assert!(!out[1].high && out[1].micros == TIMINGS::WAKEUP_LOW);
    // 2 hardware syncs = 4 half-pulses of HW_SYNC_HALF
    for p in &out[2..6] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(out[6].micros, TIMINGS::SW_SYNC_HIGH);
}

#[test]
fn repeat_frame_has_no_wakeup_and_seven_hw_syncs() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    for p in &out[0..14] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(out[14].micros, TIMINGS::SW_SYNC_HIGH);
}

#[test]
fn data_section_is_manchester_with_constant_energy() {
    // 56 data bits -> exactly 112 half-symbols of HALF_SYMBOL µs each,
    // adjacent same-level halves merged is NOT done at this layer.
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    let data: Vec<&Pulse, 320> = out
        .iter()
        .filter(|p| p.micros == TIMINGS::HALF_SYMBOL)
        .collect();
    // 112 half symbols + the SW-sync trailing HALF_SYMBOL low half
    assert_eq!(data.len(), 113);
    let highs = data.iter().filter(|p| p.high).count();
    assert_eq!(highs, 56); // Manchester: every bit contributes one high half
}

#[test]
fn frame_ends_with_inter_frame_gap() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    let last = out.last().unwrap();
    assert!(!last.high && last.micros == TIMINGS::INTER_FRAME_GAP);
}

/// Pin the exact TIMINGS literals so a silent regression to internet "folklore"
/// values can never pass the suite. These are the authoritative TX-side numbers
/// from the C++ reference `ESPSomfy-RTS/src/Somfy.cpp` (`sendFrame`,
/// Somfy.cpp:4311-4383, with `#define SYMBOL 640` at Somfy.cpp:23).
///
/// Do NOT "correct" these from the widely-cited folklore values
/// (604 / 9415 / 89565 / 4550 / 30415): those are RX-detection tolerances or
/// stale earlier drafts and do NOT belong on the TX path. See the per-constant
/// source-line rationale in `pulse.rs`.
#[test]
fn timings_literals_are_pinned_to_cpp() {
    assert_eq!(TIMINGS::WAKEUP_HIGH, 10_920);
    assert_eq!(TIMINGS::WAKEUP_LOW, 7_357);
    assert_eq!(TIMINGS::HW_SYNC_HALF, 2_560);
    assert_eq!(TIMINGS::SW_SYNC_HIGH, 4_850);
    assert_eq!(TIMINGS::HALF_SYMBOL, 640);
    assert_eq!(TIMINGS::INTER_FRAME_GAP, 27_434);
}

#[test]
fn merges_adjacent_same_level_runs() {
    let input = [
        Pulse {
            high: true,
            micros: 640,
        },
        Pulse {
            high: true,
            micros: 640,
        },
        Pulse {
            high: false,
            micros: 640,
        },
        Pulse {
            high: true,
            micros: 640,
        },
    ];
    let mut out: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&input, &mut out);
    assert_eq!(out.len(), 3);
    assert_eq!(
        out[0],
        Pulse {
            high: true,
            micros: 1280
        }
    );
    assert_eq!(
        out[1],
        Pulse {
            high: false,
            micros: 640
        }
    );
    assert_eq!(
        out[2],
        Pulse {
            high: true,
            micros: 640
        }
    );
}

#[test]
fn merged_output_strictly_alternates() {
    let f = Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 0x000A,
        address: 0x00C0DE,
    };
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(&encode56(&f).unwrap(), FrameKind::First, &mut rendered);

    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);

    assert!(!merged.is_empty());
    for pair in merged.windows(2) {
        assert_ne!(pair[0].high, pair[1].high, "merged stream must alternate");
    }
    let total_in: u32 = rendered.iter().map(|p| p.micros).sum();
    let total_out: u32 = merged.iter().map(|p| p.micros).sum();
    assert_eq!(total_in, total_out, "merging must preserve total duration");
}

/// An all-ones payload is the worst case: Manchester renders `1` as (low, high)
/// so no adjacent halves share a level and nothing merges.
#[test]
fn all_ones_payload_does_not_shrink() {
    let bytes = [0xFFu8; 7];
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes, FrameKind::First, &mut rendered);
    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);
    // Only the sync run and the gap boundary can merge; the 112 data halves cannot.
    assert!(merged.len() >= 112, "got {}", merged.len());
}
