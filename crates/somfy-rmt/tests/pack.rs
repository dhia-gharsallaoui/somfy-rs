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

/// The longest pulse that can actually reach the length field is the
/// inter-frame gap *merged with* the LOW half-symbol that a trailing `0` bit
/// ends on. Measured against a real frame rather than asserted from the
/// constants, so the arithmetic and the pulse train have to agree.
///
/// If this ever fails, the timing model outgrew the RMT length field and a
/// single pulse can no longer be expressed at all — a protocol-level problem,
/// not a buffering one.
#[test]
fn the_longest_merged_pulse_fits_the_length_field() {
    let longest = merged_for(&[0x00u8; 7], FrameKind::First)
        .iter()
        .map(|p| p.micros)
        .max()
        .expect("a rendered frame has pulses");

    assert_eq!(
        longest,
        somfy_rts::TIMINGS::INTER_FRAME_GAP + somfy_rts::TIMINGS::HALF_SYMBOL,
        "the gap should have absorbed the final LOW half-symbol"
    );
    assert!(
        longest <= MAX_TICKS,
        "{longest} µs exceeds the length field"
    );
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

/// An all-ones 80-bit first frame: 187 merged pulses, so `pack` fills 94
/// symbols and the odd count leaves the end marker in the last one for free.
///
/// This is one pulse short of the true worst case — an all-*zeros* payload
/// merges nowhere at all, where all-ones lets its leading LOW half absorb the
/// software sync's trailing LOW. The sizing decision rests on that case, and on
/// the end marker `pack` deliberately does not append; both live with
/// `build_symbols` in `tests/build_symbols.rs`.
///
/// Pinned exactly, not just bounded: a regression must fail loudly here rather
/// than slide under the `<=` check unnoticed. If a legitimate change to the
/// frame or timing model moves this number, re-derive and re-pin it
/// consciously — do not just bump the constant to make the test pass.
#[test]
fn all_ones_80bit_payload_packs_to_94_symbols() {
    let merged = merged_for(&[0xFFu8; 10], FrameKind::First);
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&merged, &mut out).expect("an 80-bit first frame must fit MAX_SYMBOLS");
    assert!(out.len() <= MAX_SYMBOLS, "needed {}", out.len());
    assert_eq!(
        out.len(),
        94,
        "symbol count changed — re-derive deliberately, see comment above"
    );
}
