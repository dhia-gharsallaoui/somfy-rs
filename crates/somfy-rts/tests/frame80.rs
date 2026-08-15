use heapless::Vec;
use somfy_rts::{
    decode80, encode80, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder, TIMINGS,
};

fn frame(command: Command) -> Frame {
    Frame {
        key: 0xA7,
        command,
        rolling_code: 0x1234,
        address: 0x00C0DE,
    }
}

/// Test-only reversal of the forward-XOR chain obfuscation applied to bytes
/// 1-6, so tests can assert on raw wire bytes. Bytes 7-9 are never obfuscated
/// in the first place — this only matters for bytes 0-6 — but it walks the
/// whole buffer to mirror the production encode/decode symmetry exactly.
/// Kept local to this test file rather than exposed from the crate: the
/// public API must not grow for test convenience.
fn deobfuscate_for_test(b: &mut [u8; 10]) {
    for i in (1..7).rev() {
        b[i] ^= b[i - 1];
    }
}

/// Byte 7 progresses as `196 + 4*repeat`, wrapping by -15 whenever the sum
/// would exceed 255.
#[test]
fn byte7_progresses_by_four_per_repeat_and_wraps_at_15() {
    let b7 = |r: u8| {
        let mut b = encode80(&frame(Command::Up), r);
        deobfuscate_for_test(&mut b);
        b[7]
    };
    assert_eq!(b7(0), 196);
    assert_eq!(b7(1), 200);
    assert_eq!(b7(14), 252);
    // repeat 15 would be 256 -> repeat -= 15 -> 0 -> back to 196.
    assert_eq!(b7(15), 196);
    assert_eq!(b7(16), 200);
}

/// Favorite and Stop flip byte 7 from 196 to 132 on any repeat > 0.
#[test]
fn favorite_and_stop_flip_byte7_on_later_repeats() {
    for cmd in [Command::Favorite, Command::Stop] {
        let mut first = encode80(&frame(cmd), 0);
        let mut later = encode80(&frame(cmd), 1);
        deobfuscate_for_test(&mut first);
        deobfuscate_for_test(&mut later);
        assert_eq!(first[7], 196, "{cmd:?} first frame");
        assert_eq!(later[7], 132, "{cmd:?} repeat frame");
    }
}

/// Base-command tails: the fixed byte 8 / byte 9 high-nibble values each
/// base command encodes.
#[test]
fn base_command_tails_match_cpp() {
    let cases = [
        (Command::Up, 32u8, 0x00u8),
        (Command::Down, 44, 0x80),
        (Command::My, 0x00, 0x10),
    ];
    for (cmd, b8, b9_hi) in cases {
        let mut b = encode80(&frame(cmd), 0);
        deobfuscate_for_test(&mut b);
        assert_eq!(b[8], b8, "{cmd:?} byte 8");
        assert_eq!(b[9] & 0xF0, b9_hi, "{cmd:?} byte 9 high nibble");
    }
}

#[test]
fn roundtrips_at_every_repeat() {
    for cmd in [
        Command::Up,
        Command::Down,
        Command::My,
        Command::Stop,
        Command::Favorite,
    ] {
        for repeat in 0..=16u8 {
            let bytes = encode80(&frame(cmd), repeat);
            let got = decode80(&bytes).expect("decode");
            assert_eq!(got.command, cmd, "cmd {cmd:?} repeat {repeat}");
            assert_eq!(got.address, 0x00C0DE);
            assert_eq!(got.rolling_code, 0x1234);
        }
    }
}

#[test]
fn roundtrip_80_extended_commands() {
    for cmd in [Command::StepUp, Command::Favorite, Command::Stop] {
        let f = Frame {
            key: 0xA5,
            command: cmd,
            rolling_code: 100,
            address: 0x654321,
        };
        let back = decode80(&encode80(&f, 0)).unwrap();
        assert_eq!(back, f, "roundtrip failed for {:?}", cmd);
    }
}

#[test]
fn rx_decoder_recognizes_80_bit_frames() {
    let f = Frame {
        key: 0xA5,
        command: Command::StepUp,
        rolling_code: 5,
        address: 0x111111,
    };
    let mut pulses: Vec<Pulse, 320> = Vec::new();
    render_pulses(&encode80(&f, 0), FrameKind::Repeat, &mut pulses);
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in &pulses {
        if let Some(fr) = rx.push(*p) {
            got = Some(fr);
        }
    }
    let rxf = got.expect("80-bit frame decoded");
    assert_eq!(rxf.bit_length, 80);
    let back = decode80(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

/// The pulse layer must key its sync counts and gap emission off frame size:
/// an 80-bit frame sends 12 hardware syncs on the first frame and 6 on
/// repeats (vs 2 / 7 for 56-bit), and the inter-frame gap is suppressed
/// entirely for 80-bit frames.
#[test]
fn pulse_layer_uses_80_bit_sync_counts_and_no_gap() {
    let f = Frame {
        key: 0xA5,
        command: Command::Stop,
        rolling_code: 9,
        address: 0x222222,
    };
    let bytes = encode80(&f, 0);

    // First frame: wakeup pulse then 12 hardware syncs (24 half-pulses).
    let mut first: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes, FrameKind::First, &mut first);
    assert!(first[0].high && first[0].micros == TIMINGS::WAKEUP_HIGH);
    assert!(!first[1].high && first[1].micros == TIMINGS::WAKEUP_LOW);
    for p in &first[2..26] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(first[26].micros, TIMINGS::SW_SYNC_HIGH);

    // Repeat frame: no wakeup, 6 hardware syncs (12 half-pulses).
    let mut repeat: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes, FrameKind::Repeat, &mut repeat);
    for p in &repeat[0..12] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(repeat[12].micros, TIMINGS::SW_SYNC_HIGH);

    // No inter-frame gap for 80-bit: the last pulse is a data half-symbol, not
    // the long INTER_FRAME_GAP silence a 56-bit frame ends with.
    assert_ne!(repeat.last().unwrap().micros, TIMINGS::INTER_FRAME_GAP);
}
