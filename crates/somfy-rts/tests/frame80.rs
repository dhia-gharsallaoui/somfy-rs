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
fn base_command_tails_are_fixed_per_command() {
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

// ---------------------------------------------------------------------------
// A whole burst, on the air and back
// ---------------------------------------------------------------------------

/// Render one burst exactly as the radio task drives it: a first frame and then
/// `repeats` repeats, each **re-encoded at its own repeat index**, concatenated
/// back to back with no gap between them — which for this width is not an
/// omission, it is what `render_pulses` does.
///
/// This is the transmit path's shape stated once so the tests below can assert
/// against a receiver rather than against the encoder they came from. A
/// transmitter reporting its own success proves nothing.
fn burst(f: &Frame, repeats: u8) -> std::vec::Vec<Pulse> {
    let mut air = std::vec::Vec::new();
    for repeat in 0..=repeats {
        let kind = if repeat == 0 {
            FrameKind::First
        } else {
            FrameKind::Repeat
        };
        let mut frame: Vec<Pulse, 320> = Vec::new();
        render_pulses(&encode80(f, repeat), kind, &mut frame);
        air.extend(frame.iter().copied());
    }
    air
}

/// Feed a pulse stream to one long-lived decoder and collect every frame it
/// completes — the same decoder the radio task keeps for the life of the task,
/// deliberately never reset between frames.
fn decode_air(air: &[Pulse]) -> std::vec::Vec<(u8, std::vec::Vec<u8>)> {
    let mut rx = RxDecoder::new();
    let mut out = std::vec::Vec::new();
    for pulse in air {
        if let Some(frame) = rx.push(*pulse) {
            out.push((frame.bit_length, frame.bytes.as_slice().to_vec()));
        }
    }
    out
}

/// The whole 80-bit burst comes back off the air, frame by frame.
///
/// This is the closest a host can get to the claim that matters: an 80-bit
/// shade is driven by a *burst*, not by one frame, and every one of its frames
/// has to survive on its own. It ties three things together that are otherwise
/// only checked in isolation — the sync counts (12 on the first frame, 6 on a
/// repeat) are what tell the receiver this is an 80-bit transmission at all, the
/// suppressed inter-frame gap is what lets the frames run back to back, and the
/// per-repeat tail is what distinguishes one frame of the burst from the next.
/// Break any of the three and this fails.
#[test]
fn an_80_bit_burst_decodes_frame_by_frame_off_the_air() {
    let f = frame(Command::StepUp);
    let repeats = 2u8;

    let decoded = decode_air(&burst(&f, repeats));

    assert_eq!(
        decoded.len(),
        repeats as usize + 1,
        "total frames = repeats + 1"
    );
    for (index, (bit_length, bytes)) in decoded.iter().enumerate() {
        assert_eq!(*bit_length, 80, "frame {index} must be read as 80-bit");
        assert_eq!(
            bytes.as_slice(),
            &encode80(&f, index as u8)[..],
            "frame {index} must arrive as its own repeat index encodes it"
        );
        let back = decode80(bytes.as_slice().try_into().unwrap()).expect("decode");
        assert_eq!(back, f, "frame {index} must decode to what was sent");
    }
}

/// The repeat index reaches the air, rather than being an encoder detail that
/// the pulse layer flattens.
///
/// `Favorite` is the command that shows it: its byte 7 is 196 on the first frame
/// and 132 on every repeat. A transmitter that encoded once and resent the same
/// bytes would produce three identical frames here, and nothing on air would
/// report it — the command still decodes, so only the bytes can tell.
#[test]
fn the_repeat_index_survives_the_pulse_train() {
    let f = frame(Command::Favorite);

    let decoded = decode_air(&burst(&f, 2));

    let tails: std::vec::Vec<u8> = decoded
        .iter()
        .map(|(_, bytes)| {
            let mut raw: [u8; 10] = bytes.as_slice().try_into().unwrap();
            deobfuscate_for_test(&mut raw);
            raw[7]
        })
        .collect();
    assert_eq!(tails, [196, 132, 132]);
    // ...and all three are still the same command at the same address, so the
    // difference above is the repeat index and nothing else.
    for (_, bytes) in &decoded {
        let back = decode80(bytes.as_slice().try_into().unwrap()).expect("decode");
        assert_eq!(back, f);
    }
}

/// The sync counts, counted on the burst rather than on a single frame.
///
/// A hardware sync is one HIGH half plus one LOW half, so a frame's count is
/// half the run of `HW_SYNC_HALF` pulses that opens it. 12 then 6 is what makes
/// `RxDecoder::detect_bit_length` answer 80; the narrow width's 2 then 7 would
/// make it answer 56 and every frame above would fail to decode.
#[test]
fn an_80_bit_burst_opens_each_frame_with_the_right_sync_run() {
    let air = burst(&frame(Command::Stop), 2);

    // Runs of hardware-sync half-pulses, in order.
    let mut runs = std::vec::Vec::new();
    let mut run = 0usize;
    for pulse in &air {
        if pulse.micros == TIMINGS::HW_SYNC_HALF {
            run += 1;
        } else if run > 0 {
            runs.push(run / 2);
            run = 0;
        }
    }
    assert_eq!(run, 0, "a burst does not end mid-sync");
    assert_eq!(runs, [12, 6, 6]);

    // And nothing in the burst is the 56-bit inter-frame silence: an 80-bit
    // frame runs straight into the next one, which is what lets the decoder
    // pick the next sync run up without re-acquiring from noise.
    assert!(air.iter().all(|p| p.micros != TIMINGS::INTER_FRAME_GAP));
}

/// A long burst still decodes, and byte 7 wraps rather than saturating.
///
/// Sixteen repeats crosses the point where `196 + 4 * repeat` would exceed 255,
/// which is where the progression cycles by -15 instead. That arithmetic is
/// already pinned against the encoder; this is it surviving the air.
#[test]
fn a_long_burst_wraps_byte_seven_and_still_decodes() {
    let f = frame(Command::Down);

    let decoded = decode_air(&burst(&f, 16));

    assert_eq!(decoded.len(), 17);
    let tails: std::vec::Vec<u8> = decoded
        .iter()
        .map(|(_, bytes)| {
            let mut raw: [u8; 10] = bytes.as_slice().try_into().unwrap();
            deobfuscate_for_test(&mut raw);
            raw[7]
        })
        .collect();
    assert_eq!(tails[0], 196);
    assert_eq!(tails[14], 252);
    assert_eq!(tails[15], 196, "repeat 15 wraps rather than overflowing");
    assert_eq!(tails[16], 200);
    for (index, (bit_length, bytes)) in decoded.iter().enumerate() {
        assert_eq!(*bit_length, 80, "frame {index}");
        assert_eq!(
            decode80(bytes.as_slice().try_into().unwrap()).expect("decode"),
            f
        );
    }
}

/// The narrow width has no frame for an extended command, which is the fact the
/// domain's `FrameWidth::carries` rule is about — stated here, at the encoder,
/// because this is where it is true.
#[test]
fn the_narrow_encoder_refuses_every_extended_command() {
    for cmd in [Command::StepUp, Command::Favorite, Command::Stop] {
        assert_eq!(
            somfy_rts::encode56(&frame(cmd)),
            Err(somfy_rts::FrameError::ExtendedCommand),
            "{cmd:?} has no 56-bit frame"
        );
    }
}
