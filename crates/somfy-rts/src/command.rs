/// Somfy commands, matching the discriminants used on the wire by real
/// remotes. Values > 0xF are "extended" commands that require 80-bit frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    My = 0x1,
    Up = 0x2,
    MyUp = 0x3,
    Down = 0x4,
    MyDown = 0x5,
    UpDown = 0x6,
    MyUpDown = 0x7,
    Prog = 0x8,
    SunFlag = 0x9,
    Flag = 0xA,
    StepDown = 0xB,
    Toggle = 0xC,
    Sensor = 0xE,
    RtwProto = 0xF,
    StepUp = 0x8B,
    Favorite = 0xC1,
    Stop = 0xF1,
}

impl Command {
    /// Low nibble placed in the 56-bit frame command field.
    pub fn nibble(self) -> u8 {
        (self as u8) & 0x0F
    }

    pub fn is_extended(self) -> bool {
        (self as u8) > 0x0F
    }

    pub fn from_nibble(n: u8) -> Option<Command> {
        Command::from_u8(n & 0x0F)
    }

    /// Map a full command byte back to a `Command`. Accepts both the base
    /// nibble values (0x1..=0xF) and the three extended 80-bit values.
    /// Unlike [`Command::from_nibble`] this does NOT mask to four bits, so
    /// `decode80` can reconstruct e.g. `0x8B` -> `StepUp` without it
    /// collapsing to `StepDown`.
    pub fn from_u8(v: u8) -> Option<Command> {
        Some(match v {
            0x1 => Command::My,
            0x2 => Command::Up,
            0x3 => Command::MyUp,
            0x4 => Command::Down,
            0x5 => Command::MyDown,
            0x6 => Command::UpDown,
            0x7 => Command::MyUpDown,
            0x8 => Command::Prog,
            0x9 => Command::SunFlag,
            0xA => Command::Flag,
            0xB => Command::StepDown,
            0xC => Command::Toggle,
            0xE => Command::Sensor,
            0xF => Command::RtwProto,
            0x8B => Command::StepUp,
            0xC1 => Command::Favorite,
            0xF1 => Command::Stop,
            _ => return None,
        })
    }
}
