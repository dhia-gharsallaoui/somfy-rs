use somfy_rts::{decode56, encode56, Command, Frame, FrameError};

fn sample() -> Frame {
    Frame {
        key: 0xA7,
        command: Command::Up,
        rolling_code: 42,
        address: 0x27_96_20,
    }
}

#[test]
fn roundtrip_56() {
    let f = sample();
    let bytes = encode56(&f).unwrap();
    let back = decode56(&bytes).unwrap();
    assert_eq!(back.command, Command::Up);
    assert_eq!(back.rolling_code, 42);
    assert_eq!(back.address, 0x27_96_20);
}

#[test]
fn encode56_rejects_extended_commands() {
    // Extended commands (StepUp/Favorite/Stop) only exist on 80-bit frames.
    // Encoding one over 56-bit would silently emit its *base* nibble — e.g.
    // StepUp (0x8B) collapses to StepDown (0xB), the OPPOSITE direction, and
    // Stop/Favorite collapse to My. encode56 must refuse rather than misfire;
    // any 56-bit downgrade policy is a domain-layer decision (see fix report).
    for cmd in [Command::StepUp, Command::Favorite, Command::Stop] {
        let f = Frame {
            key: 0xA7,
            command: cmd,
            rolling_code: 42,
            address: 0x27_96_20,
        };
        assert_eq!(
            encode56(&f),
            Err(FrameError::ExtendedCommand),
            "encode56 must reject {cmd:?}"
        );
    }
}

#[test]
fn checksum_nibbles_xor_to_zero_before_obfuscation() {
    // encode56 obfuscates; deobfuscate manually and check the RTS invariant:
    // XOR of all 14 nibbles == 0.
    let bytes = encode56(&sample()).unwrap();
    let mut clear = bytes;
    for i in (1..7).rev() {
        clear[i] ^= clear[i - 1];
    }
    let x = clear.iter().fold(0u8, |acc, b| acc ^ (b >> 4) ^ (b & 0x0F));
    assert_eq!(x & 0x0F, 0);
}

#[test]
fn corrupted_frame_rejected() {
    // The RTS obfuscation is a cumulative forward XOR, so flipping any interior
    // obfuscated byte propagates the delta into two adjacent clear bytes; their
    // nibble-XOR contributions cancel and the checksum stays valid. This is an
    // inherent weakness of the checksum scheme itself, not a decoding bug — any
    // correct decoder would also accept such a corrupted frame. The only single
    // byte whose corruption the nibble-XOR checksum can catch is the last one,
    // and only for a delta with nonzero nibble parity (0xFF cancels; 0x10 does not).
    let mut bytes = encode56(&sample()).unwrap();
    bytes[6] ^= 0x10;
    assert!(matches!(decode56(&bytes), Err(FrameError::BadChecksum)));
}

#[test]
fn command_nibble_mapping_matches_cpp_enum() {
    // Command nibble values as defined by the RTS protocol's command encoding.
    assert_eq!(Command::My.nibble(), 0x1);
    assert_eq!(Command::Up.nibble(), 0x2);
    assert_eq!(Command::Down.nibble(), 0x4);
    assert_eq!(Command::Prog.nibble(), 0x8);
    assert_eq!(Command::StepDown.nibble(), 0xB);
    assert!(Command::StepUp.is_extended());
    assert!(Command::Favorite.is_extended());
    assert!(Command::Stop.is_extended());
}
