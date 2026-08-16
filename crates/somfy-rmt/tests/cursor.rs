//! Walking one received burst, one pulse at a time.
//!
//! This is the receive path's whole state machine minus the peripheral, and it
//! lives in this crate rather than in the firmware for one reason: the firmware
//! cannot be compiled for a host at all, so anything left there is untestable
//! until someone puts a board on a desk. The arithmetic that decides which pulse
//! comes next — and, above all, *when to stop* — is exactly the part where a
//! mistake reads previous traffic as live signal, so it belongs where a test can
//! reach it.
//!
//! Expectations here come from `merge_pulses`, never from `unpack`. Checking a
//! cursor built on `pulse_at` against an iterator built on `pulse_at` would
//! assert nothing at all.

use heapless::Vec;
use somfy_rmt::{build_symbols, BurstCursor, RmtSymbol, MAX_SYMBOLS};
use somfy_rts::{encode56, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse};

fn frame() -> Frame {
    Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 0x000A,
        address: 0x00C0DE,
    }
}

/// The pulse train a receiver would see for one frame, derived independently of
/// anything in the packed representation.
fn expected_pulses(kind: FrameKind) -> std::vec::Vec<Pulse> {
    let bytes = encode56(&frame()).unwrap();
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes, kind, &mut rendered);
    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);
    merged.iter().copied().collect()
}

fn symbols_for(kind: FrameKind) -> Vec<RmtSymbol, MAX_SYMBOLS> {
    let bytes = encode56(&frame()).unwrap();
    let mut symbols: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    build_symbols(&bytes, kind, &mut symbols).unwrap();
    symbols
}

fn drain(cursor: &mut BurstCursor, symbols: &[RmtSymbol]) -> std::vec::Vec<Pulse> {
    let mut out = std::vec::Vec::new();
    while let Some(pulse) = cursor.next(symbols) {
        out.push(pulse);
    }
    out
}

#[test]
fn a_cursor_hands_out_a_whole_burst_in_order() {
    let symbols = symbols_for(FrameKind::First);
    let mut cursor = BurstCursor::new();
    assert_eq!(
        drain(&mut cursor, &symbols),
        expected_pulses(FrameKind::First)
    );
}

/// The failure this type exists to make testable.
///
/// A reception reports how many symbols it filled, and the caller passes that
/// prefix. Everything past it is whatever the *previous* burst left in RMT RAM.
/// A cursor that stopped only at an end marker would walk straight into it —
/// and on the chips that cannot wrap a reception, a burst that fills channel RAM
/// stops with no marker at all, so that is not a hypothetical.
#[test]
fn a_cursor_stops_at_the_end_of_the_received_prefix_with_no_marker_in_sight() {
    // A full buffer with no zero-length entry anywhere: nothing but the slice
    // length can stop a walk over it.
    let stale = RmtSymbol {
        level1: true,
        length1: 9999,
        level2: false,
        length2: 8888,
    };
    let buffer = [stale; 8];

    // Only the first three symbols were "received".
    let mut cursor = BurstCursor::new();
    let got = drain(&mut cursor, &buffer[..3]);

    assert_eq!(got.len(), 6, "three symbols carry six entries");
    assert!(
        got.iter().all(|p| p.micros == 9999 || p.micros == 8888),
        "nothing outside the prefix should appear"
    );
}

#[test]
fn a_cursor_stops_at_a_zero_length_entry_before_the_end_of_the_prefix() {
    let buffer = [
        RmtSymbol {
            level1: true,
            length1: 640,
            level2: false,
            length2: 0,
        },
        RmtSymbol {
            level1: true,
            length1: 2560,
            level2: false,
            length2: 2560,
        },
    ];
    let mut cursor = BurstCursor::new();
    assert_eq!(
        drain(&mut cursor, &buffer),
        std::vec![Pulse {
            high: true,
            micros: 640
        }]
    );
}

/// Exhaustion must be sticky within a burst: a pump loop that stops on the
/// first `None` and then asks again — which is exactly what happens between the
/// buffer running dry and the next reception being issued — must not be handed
/// a pulse it has already delivered.
#[test]
fn an_exhausted_cursor_stays_exhausted_until_it_is_restarted() {
    let symbols = symbols_for(FrameKind::Repeat);
    let mut cursor = BurstCursor::new();
    let first_pass = drain(&mut cursor, &symbols);
    assert!(!first_pass.is_empty());

    assert_eq!(cursor.next(&symbols), None);
    assert_eq!(cursor.next(&symbols), None);
}

/// And `restart` is what un-sticks it, for the next burst rather than the same
/// one. A cursor that kept its index across a reception would hand out the tail
/// of a buffer whose contents had been replaced underneath it.
#[test]
fn restart_puts_a_cursor_back_at_the_first_entry_of_a_new_burst() {
    let first = symbols_for(FrameKind::First);
    let repeat = symbols_for(FrameKind::Repeat);

    let mut cursor = BurstCursor::new();
    drain(&mut cursor, &first);
    assert_eq!(cursor.next(&first), None, "spent");

    cursor.restart();
    assert_eq!(
        drain(&mut cursor, &repeat),
        expected_pulses(FrameKind::Repeat)
    );
}

/// Restarting partway through is the cancelled-reception case: the burst is
/// abandoned, not resumed.
#[test]
fn restart_discards_an_unfinished_burst_rather_than_resuming_it() {
    let symbols = symbols_for(FrameKind::First);
    let expected = expected_pulses(FrameKind::First);

    let mut cursor = BurstCursor::new();
    assert_eq!(cursor.next(&symbols), Some(expected[0]));
    assert_eq!(cursor.next(&symbols), Some(expected[1]));

    cursor.restart();
    assert_eq!(
        cursor.next(&symbols),
        Some(expected[0]),
        "back to the start"
    );
}

#[test]
fn a_cursor_over_an_empty_burst_yields_nothing() {
    let mut cursor = BurstCursor::new();
    assert_eq!(cursor.next(&[]), None);
}
