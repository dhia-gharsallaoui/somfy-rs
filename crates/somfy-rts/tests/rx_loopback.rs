use heapless::Vec;
use somfy_rts::{decode56, encode56, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder};

fn tx_pulses(f: &Frame, kind: FrameKind) -> Vec<Pulse, 320> {
    let mut out = Vec::new();
    render_pulses(&encode56(f), kind, &mut out);
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
