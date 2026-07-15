use heapless::Vec;
use somfy_rts::{decode56, encode56, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder};

fn tx_pulses(f: &Frame, kind: FrameKind) -> Vec<Pulse, 320> {
    let mut out = Vec::new();
    render_pulses(&encode56(f), kind, &mut out);
    out
}

/// Collapse adjacent same-level pulses into single summed-duration pulses —
/// the edge-to-edge form an interrupt-driven receiver (and the C++ firmware's
/// `rx.pulses[]` captures) actually produces.
fn merge_pulses(pulses: &[Pulse]) -> Vec<Pulse, 320> {
    let mut out: Vec<Pulse, 320> = Vec::new();
    for p in pulses {
        if let Some(last) = out.last_mut() {
            if last.high == p.high {
                last.micros += p.micros;
                continue;
            }
        }
        out.push(*p).unwrap();
    }
    out
}

fn decode_stream(pulses: &[Pulse]) -> Option<somfy_rts::RxFrame> {
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in pulses {
        if let Some(fr) = rx.push(*p) {
            got = Some(fr);
        }
    }
    got
}

#[test]
fn software_loopback_roundtrip_first_frame() {
    let f = Frame {
        key: 0xA7,
        command: Command::Down,
        rolling_code: 1234,
        address: 0x0BCDEF,
    };
    let rxf = decode_stream(&tx_pulses(&f, FrameKind::First)).expect("frame decoded");
    assert_eq!(rxf.bit_length, 56);
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn software_loopback_roundtrip_repeat_frame() {
    let f = Frame {
        key: 0xA1,
        command: Command::My,
        rolling_code: 9,
        address: 0x000001,
    };
    let rxf = decode_stream(&tx_pulses(&f, FrameKind::Repeat)).expect("frame decoded");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn tolerates_10_percent_timing_jitter() {
    let f = Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 77,
        address: 0x123456,
    };
    let mut pulses = tx_pulses(&f, FrameKind::Repeat);
    for (i, p) in pulses.iter_mut().enumerate() {
        let sign: i64 = if i % 2 == 0 { 1 } else { -1 };
        p.micros = (p.micros as i64 + sign * (p.micros as i64 / 10)) as u32;
    }
    let rxf = decode_stream(&pulses).expect("jittered frame decoded");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn noise_before_frame_is_ignored() {
    let f = Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 2,
        address: 0x424242,
    };
    let mut stream: Vec<Pulse, 400> = Vec::new();
    for i in 0..40 {
        stream
            .push(Pulse {
                high: i % 2 == 0,
                micros: 137 + i * 13,
            })
            .unwrap();
    }
    stream.extend(tx_pulses(&f, FrameKind::Repeat).iter().copied());
    let rxf = decode_stream(&stream).expect("frame found after noise");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

/// Real hardware measures edge-to-edge durations, so consecutive same-level
/// half-symbols arrive merged into ~1280us segments (this is the shape of the
/// C++ firmware's `rx.pulses[]` captures). The decoder must accept that
/// stream, not just the unmerged synthetic one.
#[test]
fn merged_edge_to_edge_stream_decodes() {
    let f = Frame {
        key: 0xA7,
        command: Command::Down,
        rolling_code: 4242,
        address: 0x0BCDEF,
    };
    for kind in [FrameKind::First, FrameKind::Repeat] {
        let unmerged = tx_pulses(&f, kind);
        let merged = merge_pulses(&unmerged);
        assert!(
            merged.len() < unmerged.len(),
            "payload must contain 0<->1 transitions for this test to bite"
        );
        let rxf = decode_stream(&merged).expect("merged stream decoded");
        assert_eq!(rxf.bit_length, 56);
        let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
        assert_eq!(back, f, "kind {kind:?}");
    }
}

/// The tolerance window is +/-25%: a data half-pulse stretched +24% still
/// decodes; at +26% the decoder abandons the frame.
#[test]
fn tolerance_window_boundaries() {
    let f = Frame {
        key: 0xA7,
        command: Command::My,
        rolling_code: 55,
        address: 0x314159,
    };
    // Repeat layout: 14 hw-sync halves (0..=13), sw sync (14), start-0 (15),
    // data halves (16..=127), gap (128). Index 40 is a mid-payload half.
    let base = tx_pulses(&f, FrameKind::Repeat);

    let mut ok = base.clone();
    ok[40].micros = 640 + 640 * 24 / 100; // 793 <= 800 upper bound
    let rxf = decode_stream(&ok).expect("+24% half-pulse still decodes");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);

    let mut bad = base.clone();
    bad[40].micros = 640 + 640 * 26 / 100; // 806 > 800 upper bound
    assert_eq!(
        decode_stream(&bad),
        None,
        "+26% half-pulse must abort the frame"
    );
}
