use heapless::Vec;
use somfy_rmt::{pack, PackError, RmtSymbol, MAX_SYMBOLS, MAX_TICKS};
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
fn packs_two_pulses_per_symbol() {
    let input = [
        Pulse {
            high: true,
            micros: 100,
        },
        Pulse {
            high: false,
            micros: 200,
        },
        Pulse {
            high: true,
            micros: 300,
        },
        Pulse {
            high: false,
            micros: 400,
        },
    ];
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&input, &mut out).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[0],
        RmtSymbol {
            level1: true,
            length1: 100,
            level2: false,
            length2: 200
        }
    );
    assert_eq!(
        out[1],
        RmtSymbol {
            level1: true,
            length1: 300,
            level2: false,
            length2: 400
        }
    );
}

/// An odd pulse count leaves the second half of the last symbol empty. A
/// zero-length entry is RMT's end marker, so this is exactly what we want.
///
/// The asymmetric case (not covered by this test): an *even* pulse count
/// fills every symbol completely and `pack` emits no terminator at all — the
/// caller must append one. See `pack`'s doc comment.
#[test]
fn odd_pulse_count_zero_pads_final_symbol() {
    let input = [
        Pulse {
            high: true,
            micros: 100,
        },
        Pulse {
            high: false,
            micros: 200,
        },
        Pulse {
            high: true,
            micros: 300,
        },
    ];
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&input, &mut out).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[1].length1, 300);
    assert_eq!(out[1].length2, 0, "zero length terminates the transmission");
}

#[test]
fn rejects_a_pulse_longer_than_the_15_bit_field() {
    let input = [Pulse {
        high: true,
        micros: MAX_TICKS + 1,
    }];
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    match pack(&input, &mut out) {
        Err(PackError::TooLong { micros }) => assert_eq!(micros, MAX_TICKS + 1),
        other => panic!("expected TooLong, got {other:?}"),
    }
}

/// The longest real pulse is INTER_FRAME_GAP. If this ever fails, the timing
/// model outgrew the RMT length field and rmt_tx must switch to wrap-around
/// refill (design spec §5.2).
///
/// Both sides are `const`, so clippy sees this as trivially true; it is kept
/// as a runtime test (in addition to the compile-time guard in `lib.rs`) so a
/// future change to either constant shows up as a named test failure here.
#[test]
#[allow(clippy::assertions_on_constants)]
fn inter_frame_gap_fits_the_length_field() {
    assert!(somfy_rts::TIMINGS::INTER_FRAME_GAP <= MAX_TICKS);
}

#[test]
fn every_frame_shape_fits_in_max_symbols() {
    let cases = [
        (
            encode56(&frame(Command::Up)).unwrap().to_vec(),
            FrameKind::First,
        ),
        (
            encode56(&frame(Command::Up)).unwrap().to_vec(),
            FrameKind::Repeat,
        ),
        (encode80(&frame(Command::Up), 0).to_vec(), FrameKind::First),
        (encode80(&frame(Command::Up), 1).to_vec(), FrameKind::Repeat),
    ];
    for (bytes, kind) in cases {
        let merged = merged_for(&bytes, kind);
        let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
        pack(&merged, &mut out).unwrap_or_else(|e| panic!("{kind:?} {} bytes: {e:?}", bytes.len()));
    }
}

/// Worst case for symbol count: a payload where no adjacent halves merge.
#[test]
fn worst_case_80bit_payload_fits() {
    let merged = merged_for(&[0xFFu8; 10], FrameKind::First);
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&merged, &mut out).expect("worst-case 80-bit frame must fit MAX_SYMBOLS");
    assert!(out.len() <= MAX_SYMBOLS, "needed {}", out.len());
    // Pinned exactly, not just bounded: this is a measurement, not a
    // prediction, and it leaves only 2 symbols of headroom against
    // MAX_SYMBOLS = 96 — the two-RMT-memory-block allocation this crate is
    // sized for. That margin is a hardware design decision resting on this
    // specific number, so a regression must fail loudly here rather than
    // slide under the `<=` check above unnoticed. If a legitimate change to
    // the frame or timing model moves this number, re-derive and re-pin it
    // consciously — do not just bump the constant to make the test pass.
    assert_eq!(
        out.len(),
        94,
        "worst-case symbol count changed — re-derive deliberately, see comment above"
    );
}
