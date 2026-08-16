//! Reading RMT symbols back out as pulses — the receive-side inverse of
//! `pack`.
//!
//! The peripheral hands a receiver the same packed form the transmitter builds:
//! two (level, duration) entries per 32-bit symbol, terminated by a zero-length
//! entry. Round-tripping a real frame through both directions is the strongest
//! statement available on the host that the two agree, since a mistake in
//! either — entry order, tick scaling, where the terminator stops the stream —
//! moves the pulses and the assertion catches it.

use heapless::Vec;
use somfy_rmt::{build_symbols, pack, pulse_at, unpack, RmtSymbol, MAX_SYMBOLS, TICK_US};
use somfy_rts::{
    encode56, encode80, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse,
};

fn frame(command: Command) -> Frame {
    Frame {
        key: 0xA7,
        command,
        rolling_code: 0x000A,
        address: 0x00C0DE,
    }
}

fn merged_for(bytes: &[u8], kind: FrameKind) -> Vec<Pulse, 320> {
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(bytes, kind, &mut rendered);
    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);
    merged
}

#[test]
fn unpack_yields_both_entries_of_each_symbol_in_order() {
    let symbols = [
        RmtSymbol {
            level1: true,
            length1: 100,
            level2: false,
            length2: 200,
        },
        RmtSymbol {
            level1: false,
            length1: 300,
            level2: true,
            length2: 400,
        },
    ];
    let got: std::vec::Vec<Pulse> = unpack(&symbols).collect();
    assert_eq!(
        got,
        std::vec![
            Pulse {
                high: true,
                micros: 100
            },
            Pulse {
                high: false,
                micros: 200
            },
            Pulse {
                high: false,
                micros: 300
            },
            Pulse {
                high: true,
                micros: 400
            },
        ]
    );
}

/// A zero-length entry is the peripheral's end-of-transmission marker on both
/// sides of the link. Everything after it is stale RMT RAM, so a receiver that
/// read past it would decode the previous reception's tail as if it were live
/// signal.
#[test]
fn unpack_stops_at_a_zero_length_entry_and_never_resumes() {
    let symbols = [
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
    let mut pulses = unpack(&symbols);
    assert_eq!(
        pulses.next(),
        Some(Pulse {
            high: true,
            micros: 640
        })
    );
    assert_eq!(pulses.next(), None, "the zero-length entry ends the stream");
    assert_eq!(pulses.next(), None, "and it stays ended");
}

/// A terminator is not guaranteed. `Channel::receive` reports how many symbols
/// it copied, and a reception that filled the buffer exactly leaves no room for
/// one — so running off the end of the slice has to end the stream just as
/// cleanly as meeting a marker does.
#[test]
fn unpack_ends_at_the_slice_end_when_there_is_no_terminator() {
    let symbols = [RmtSymbol {
        level1: true,
        length1: 640,
        level2: false,
        length2: 640,
    }];
    assert_eq!(unpack(&symbols).count(), 2);
}

#[test]
fn unpack_of_an_empty_buffer_yields_nothing() {
    assert_eq!(unpack(&[]).next(), None);
}

/// Durations are tick counts in the symbol and microseconds in the pulse.
///
/// **At the configured 1 µs tick there is nothing here to catch**, and saying so
/// is the point: an implementation that dropped the scaling entirely would pass
/// any direct assertion on these numbers. So this pins the tick model itself,
/// which is what makes ticks and microseconds interchangeable everywhere else in
/// this crate. The scaling is covered where it can be — through `pack`, which
/// divides by the same constant, in the round-trip test below.
#[test]
fn ticks_and_microseconds_are_numerically_equal_at_the_configured_tick() {
    assert_eq!(TICK_US, 1, "the rest of this crate reads durations as µs");

    let symbols = [RmtSymbol {
        level1: true,
        length1: 7,
        level2: false,
        length2: 9,
    }];
    let got: std::vec::Vec<Pulse> = unpack(&symbols).collect();
    assert_eq!(got[0].micros, 7 * TICK_US);
    assert_eq!(got[1].micros, 9 * TICK_US);
}

/// `unpack` walks entries; `pulse_at` addresses one directly. A receiver that
/// keeps its own cursor beside the buffer uses the latter.
///
/// Checked against the **merged pulse train**, not against `unpack`: `unpack` is
/// implemented as `pulse_at` over an incrementing index, so comparing the two
/// would assert `pulse_at(s, n) == pulse_at(s, n)` and could not fail for any
/// implementation at all.
#[test]
fn pulse_at_addresses_the_merged_pulse_train_entry_for_entry() {
    let bytes = encode56(&frame(Command::Up)).unwrap();
    let merged = merged_for(&bytes, FrameKind::First);
    let mut symbols: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    build_symbols(&bytes, FrameKind::First, &mut symbols).unwrap();

    for (entry, expected) in merged.iter().enumerate() {
        assert_eq!(pulse_at(&symbols, entry), Some(*expected), "entry {entry}");
    }
    assert_eq!(
        pulse_at(&symbols, merged.len()),
        None,
        "past the last pulse"
    );
}

/// What a *reception* of the worst-case frame needs, which is not the same
/// calculation as what transmitting it needs even though it lands on the same
/// number.
///
/// Transmitting packs the merged edges two to a symbol and appends an end
/// marker. Receiving records one entry per edge and the peripheral writes its
/// own zero-length terminator, so the budget is `(edges + 1)` entries rounded up
/// to symbols. `MAX_SYMBOLS` has to cover the larger of the two, and the
/// firmware asserts its receive buffer against it — so pin the receive side here
/// rather than let that assertion rest on a transmit-side figure that happens to
/// be big enough.
#[test]
fn a_worst_case_reception_fits_max_symbols() {
    // All-zeros merges nowhere, which is what makes it the worst case.
    let cases = [
        (std::vec![0x00u8; 7], FrameKind::First, 61),
        (std::vec![0x00u8; 7], FrameKind::Repeat, 65),
        (std::vec![0x00u8; 10], FrameKind::First, 95),
        (std::vec![0x00u8; 10], FrameKind::Repeat, 88),
    ];

    for (bytes, kind, expected) in cases {
        let edges = merged_for(&bytes, kind).len();
        let needed = (edges + 1).div_ceil(2);
        assert_eq!(
            needed,
            expected,
            "{kind:?} {} bytes: {edges} edges — re-derive deliberately",
            bytes.len()
        );
        assert!(
            needed <= MAX_SYMBOLS,
            "{kind:?} {} bytes needs {needed} symbols to receive",
            bytes.len()
        );
    }
}

/// The whole point: every frame this transmitter can build survives a trip
/// through the packed representation and comes back as the pulse train it
/// started as.
#[test]
fn every_frame_shape_round_trips_through_the_packed_form() {
    let cases = [
        (
            encode56(&frame(Command::Up)).unwrap().to_vec(),
            FrameKind::First,
        ),
        (
            encode56(&frame(Command::Down)).unwrap().to_vec(),
            FrameKind::Repeat,
        ),
        (encode80(&frame(Command::Up), 0).to_vec(), FrameKind::First),
        (encode80(&frame(Command::Up), 1).to_vec(), FrameKind::Repeat),
        // The worst case for symbol count: an all-zeros payload merges nowhere.
        (std::vec![0x00u8; 7], FrameKind::First),
        (std::vec![0x00u8; 10], FrameKind::First),
    ];

    for (bytes, kind) in cases {
        let merged = merged_for(&bytes, kind);
        let mut symbols: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
        build_symbols(&bytes, kind, &mut symbols).unwrap();

        let got: std::vec::Vec<Pulse> = unpack(&symbols).collect();
        assert_eq!(
            got,
            merged.as_slice(),
            "{kind:?} {} bytes did not round trip",
            bytes.len()
        );
    }
}

/// `pack` deliberately emits no terminator for an even pulse count (that is
/// `build_symbols`' job). Unpacking such a buffer must still return everything
/// in it — the case above only covers buffers that carry a marker.
#[test]
fn a_packed_even_length_buffer_round_trips_without_a_terminator() {
    let input = [
        Pulse {
            high: true,
            micros: 640,
        },
        Pulse {
            high: false,
            micros: 1280,
        },
    ];
    let mut symbols: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&input, &mut symbols).unwrap();
    assert_eq!(symbols.len(), 1, "two pulses are one symbol");

    let got: std::vec::Vec<Pulse> = unpack(&symbols).collect();
    assert_eq!(got, input);
}
