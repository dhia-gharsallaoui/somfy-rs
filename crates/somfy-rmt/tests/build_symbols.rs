//! `build_symbols` — the whole render → merge → pack → terminate pipeline.
//!
//! This is where the transmit path's real logic lives, and it is pure data, so
//! it is tested here on the host rather than on a chip. The firmware's half is
//! a two-line delegation to `esp_hal::rmt::PulseCode`.

use heapless::Vec;
use somfy_rmt::{build_symbols, PackError, RmtSymbol, MAX_SYMBOLS};
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

fn rendered_for(bytes: &[u8], kind: FrameKind) -> Vec<Pulse, 320> {
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(bytes, kind, &mut rendered);
    rendered
}

fn merged_for(bytes: &[u8], kind: FrameKind) -> Vec<Pulse, 320> {
    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered_for(bytes, kind), &mut merged);
    merged
}

fn built(bytes: &[u8], kind: FrameKind) -> Vec<RmtSymbol, MAX_SYMBOLS> {
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    build_symbols(bytes, kind, &mut out)
        .unwrap_or_else(|e| panic!("{} bytes, {kind:?}: {e:?}", bytes.len()));
    out
}

/// Flatten symbols back into (level, length) halves, dropping the trailing
/// zero-length end marker(s). What remains must be the merged pulse train.
fn halves(symbols: &[RmtSymbol]) -> std::vec::Vec<(bool, u16)> {
    let mut flat = std::vec::Vec::new();
    for s in symbols {
        flat.push((s.level1, s.length1));
        flat.push((s.level2, s.length2));
    }
    while flat.last().is_some_and(|(_, length)| *length == 0) {
        flat.pop();
    }
    flat
}

/// Every symbol the RMT peripheral consumes must correspond, half for half, to
/// the merged pulse train — no reordering, no dropped edge, no unmerged pair.
#[test]
fn symbols_reproduce_the_merged_pulse_train_exactly() {
    for (bytes, kind) in real_frames() {
        let merged = merged_for(&bytes, kind);
        let expected: std::vec::Vec<(bool, u16)> = merged
            .iter()
            .map(|p| (p.high, u16::try_from(p.micros).unwrap()))
            .collect();
        assert_eq!(
            halves(&built(&bytes, kind)),
            expected,
            "{} bytes, {kind:?}",
            bytes.len()
        );
    }
}

/// A transmission that lost or gained time would key the carrier at the wrong
/// moments. Merging redistributes duration between entries; it must never
/// change the total.
#[test]
fn total_on_air_duration_survives_merging_and_packing() {
    for (bytes, kind) in real_frames() {
        let rendered: u32 = rendered_for(&bytes, kind).iter().map(|p| p.micros).sum();
        let packed: u32 = built(&bytes, kind)
            .iter()
            .map(|s| u32::from(s.length1) + u32::from(s.length2))
            .sum();
        assert_eq!(packed, rendered, "{} bytes, {kind:?}", bytes.len());
    }
}

/// Proof that the merge step actually ran: an edge-to-edge stream alternates
/// level on every entry. Two same-level entries in a row would mean a pair of
/// half-symbols went out unmerged, wasting a symbol and shifting every
/// subsequent edge.
#[test]
fn no_two_adjacent_entries_share_a_level() {
    for (bytes, kind) in real_frames() {
        let flat = halves(&built(&bytes, kind));
        for pair in flat.windows(2) {
            assert_ne!(
                pair[0].0,
                pair[1].0,
                "unmerged adjacent halves in {} bytes, {kind:?}: {pair:?}",
                bytes.len()
            );
        }
    }
}

/// An odd merged-pulse count already leaves the last symbol's second half
/// zero-length, which *is* the RMT end marker. `build_symbols` must recognise
/// that and not append a redundant symbol on top of it.
#[test]
fn an_odd_pulse_count_reuses_the_terminator_pack_already_produces() {
    let bytes = [0xFF; 10];
    let merged = merged_for(&bytes, FrameKind::First);
    assert_eq!(
        merged.len() % 2,
        1,
        "this case must have an odd pulse count"
    );

    let symbols = built(&bytes, FrameKind::First);
    assert_eq!(symbols.len(), merged.len().div_ceil(2));
    let last = symbols.last().unwrap();
    assert_ne!(last.length1, 0, "the first half still carries a real pulse");
    assert_eq!(last.length2, 0, "the second half is the end marker");
}

/// An even merged-pulse count fills every half of every symbol, so nothing in
/// the buffer says "stop". Without an explicit appended marker the peripheral
/// keeps transmitting whatever follows in RMT RAM.
#[test]
fn an_even_pulse_count_gets_an_explicit_appended_terminator() {
    let bytes = [0x00; 10];
    let merged = merged_for(&bytes, FrameKind::First);
    assert_eq!(
        merged.len() % 2,
        0,
        "this case must have an even pulse count"
    );

    let symbols = built(&bytes, FrameKind::First);
    assert_eq!(symbols.len(), merged.len() / 2 + 1);
    assert_eq!(
        *symbols.last().unwrap(),
        RmtSymbol {
            level1: false,
            length1: 0,
            level2: false,
            length2: 0,
        },
        "the appended symbol must be wholly zero-length"
    );
}

/// The sizing measurement the two-RMT-memory-block allocation rests on.
///
/// Worst case is an 80-bit first frame whose payload produces no merges at all:
/// all-zero bytes. Every bit renders HIGH-then-LOW, so no bit's tail meets the
/// next bit's head at the same level, and the first bit's HIGH head does not
/// meet the software sync's LOW tail either. That is 188 merged pulses — 94
/// full symbols with no room left for an end marker, so a 95th is appended.
///
/// Pinned exactly, not merely bounded: `MAX_SYMBOLS` and the RMT memory-block
/// count are hardware decisions resting on this number. If a legitimate change
/// to the frame or timing model moves it, re-derive it deliberately rather than
/// bumping the constant until the test passes.
#[test]
fn worst_case_symbol_count_is_pinned_and_fits_max_symbols() {
    let bytes = [0x00; 10];
    assert_eq!(merged_for(&bytes, FrameKind::First).len(), 188);

    let symbols = built(&bytes, FrameKind::First);
    assert_eq!(
        symbols.len(),
        95,
        "worst-case symbol count changed — re-derive deliberately, see comment above"
    );
    assert!(symbols.len() <= MAX_SYMBOLS);
}

/// No all-zero payload is separately special: nothing the encoders emit may
/// exceed the budget either.
#[test]
fn every_real_frame_shape_fits_in_max_symbols() {
    for (bytes, kind) in real_frames() {
        let symbols = built(&bytes, kind);
        assert!(
            symbols.len() <= MAX_SYMBOLS,
            "{} bytes, {kind:?} needed {}",
            bytes.len(),
            symbols.len()
        );
    }
}

/// Every payload byte pattern, at both frame widths and both frame kinds, must
/// stay inside the budget — the worst case above is an argument, this is a
/// sweep.
#[test]
fn no_payload_byte_pattern_overflows_the_budget() {
    for fill in 0..=u8::MAX {
        for kind in [FrameKind::First, FrameKind::Repeat] {
            assert!(built(&[fill; 7], kind).len() <= MAX_SYMBOLS);
            assert!(built(&[fill; 10], kind).len() <= MAX_SYMBOLS);
        }
    }
}

/// A byte slice that is neither frame width is not a Somfy frame. Rendering it
/// anyway would frame it as a 56-bit transmission of the wrong bit count —
/// garbage on air, silently — and a long enough slice would overflow the pulse
/// buffer and panic inside the renderer. Reject it at the door instead.
#[test]
fn a_slice_that_is_not_a_somfy_frame_is_rejected() {
    for length in [0usize, 1, 6, 8, 9, 11, 20, 64, 320] {
        let bytes = std::vec![0xAAu8; length];
        for kind in [FrameKind::First, FrameKind::Repeat] {
            let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
            assert_eq!(
                build_symbols(&bytes, kind, &mut out),
                Err(PackError::UnsupportedFrameLength { bytes: length }),
                "{length} bytes, {kind:?}"
            );
        }
    }
}

/// The output buffer is cleared, not appended to: a caller reusing one buffer
/// across frames must not transmit the previous frame's tail.
#[test]
fn a_reused_buffer_holds_only_the_current_frame() {
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    build_symbols(&[0x00; 10], FrameKind::First, &mut out).unwrap();
    let long = out.len();

    build_symbols(&[0xFF; 7], FrameKind::Repeat, &mut out).unwrap();
    assert!(out.len() < long);
    assert_eq!(out, built(&[0xFF; 7], FrameKind::Repeat));
}

/// A rejected frame must not leave a half-built pulse train behind for the
/// caller to transmit.
#[test]
fn a_rejected_frame_leaves_the_buffer_empty() {
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    build_symbols(&[0x00; 10], FrameKind::First, &mut out).unwrap();
    assert!(!out.is_empty());

    assert!(build_symbols(&[0x00; 9], FrameKind::First, &mut out).is_err());
    assert!(out.is_empty());
}

/// Both frame widths, both frame kinds, built from the real encoders.
fn real_frames() -> std::vec::Vec<(std::vec::Vec<u8>, FrameKind)> {
    let mut cases = std::vec::Vec::new();
    for command in [Command::Up, Command::Down, Command::My] {
        cases.push((
            encode56(&frame(command)).unwrap().to_vec(),
            FrameKind::First,
        ));
        cases.push((
            encode56(&frame(command)).unwrap().to_vec(),
            FrameKind::Repeat,
        ));
    }
    for repeat in 0..3u8 {
        cases.push((
            encode80(&frame(Command::StepUp), repeat).to_vec(),
            if repeat == 0 {
                FrameKind::First
            } else {
                FrameKind::Repeat
            },
        ));
    }
    cases
}
