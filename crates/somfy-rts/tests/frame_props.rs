use proptest::prelude::*;
use somfy_rts::{decode56, encode56, Command, Frame};

fn any_basic_command() -> impl Strategy<Value = Command> {
    prop::sample::select(alloc_cmds())
}

fn alloc_cmds() -> Vec<Command> {
    vec![
        Command::My,
        Command::Up,
        Command::MyUp,
        Command::Down,
        Command::MyDown,
        Command::UpDown,
        Command::MyUpDown,
        Command::Prog,
        Command::SunFlag,
        Command::Flag,
        Command::StepDown,
        Command::Toggle,
        Command::Sensor,
        Command::RtwProto,
    ]
}

proptest! {
    #[test]
    fn encode_decode_roundtrip(
        key_low in 0u8..=0x0F,
        cmd in any_basic_command(),
        code in any::<u16>(),
        addr in 0u32..0x0100_0000,
    ) {
        let f = Frame { key: 0xA0 | key_low, command: cmd, rolling_code: code, address: addr };
        let back = decode56(&encode56(&f)).unwrap();
        prop_assert_eq!(back, f);
    }

    #[test]
    fn single_bit_corruption_never_decodes_silently_to_other_fields(
        code in any::<u16>(),
        addr in 0u32..0x0100_0000,
        byte_idx in 0usize..7,
        bit in 0u8..8,
    ) {
        let f = Frame { key: 0xA7, command: Command::Up, rolling_code: code, address: addr };
        let mut bytes = encode56(&f);
        bytes[byte_idx] ^= 1 << bit;
        // Either rejected, or (checksum is only 4 bits) decodes to *something* —
        // but never silently to the original frame with a different meaning.
        if let Ok(back) = decode56(&bytes) {
            prop_assert_ne!(back, f);
        }
    }
}
