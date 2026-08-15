use crate::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// "Encryption key" byte; C++ uses 0xA0 | low counter nibble.
    pub key: u8,
    pub command: Command,
    pub rolling_code: u16,
    /// 24-bit remote address.
    pub address: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    BadChecksum,
    UnknownCommand,
    /// An extended command (StepUp/Favorite/Stop) was passed to [`encode56`].
    /// Extended commands only exist on 80-bit frames; see [`encode56`] docs.
    ExtendedCommand,
}

/// Encode a 56-bit RTS frame (7 bytes).
///
/// Layout before obfuscation (matches ESPSomfy-RTS src/Somfy.cpp encodeFrame,
/// lines 335-341): `[0]`=key `[1]`=cmd<<4|cksum `[2..3]`=rolling code
/// big-endian (hi byte first) `[4..6]`=24-bit address big-endian (MSB at `[4]`,
/// LSB at `[6]`).
///
/// # Extended commands are rejected
///
/// Extended commands — `StepUp`, `Favorite`, `Stop` ([`Command::is_extended`]) —
/// require [`encode80`]: their identity lives in the un-obfuscated 80-bit tail,
/// not in the 4-bit command field. A 56-bit frame can only carry the *base*
/// nibble, which for these three is the OPPOSITE or a wrong command
/// (`StepUp 0x8B -> StepDown 0xB`; `Favorite 0xC1` and `Stop 0xF1 -> My 0x1`).
/// Rather than emit that silent misfire, `encode56` returns
/// [`FrameError::ExtendedCommand`].
///
/// Any 56-bit *downgrade* policy (e.g. the C++ reference deliberately maps
/// `Stop -> My` before its 56-bit encoder, Somfy.cpp:2944) is a domain-layer
/// decision, not this crate's: the caller must pick a base command explicitly.
pub fn encode56(f: &Frame) -> Result<[u8; 7], FrameError> {
    if f.command.is_extended() {
        return Err(FrameError::ExtendedCommand);
    }
    let mut b = [0u8; 7];
    b[0] = f.key;
    b[1] = f.command.nibble() << 4;
    b[2] = (f.rolling_code >> 8) as u8;
    b[3] = f.rolling_code as u8;
    b[4] = (f.address >> 16) as u8;
    b[5] = (f.address >> 8) as u8;
    b[6] = f.address as u8;

    b[1] |= checksum(&b);
    obfuscate(&mut b);
    Ok(b)
}

pub fn decode56(bytes: &[u8; 7]) -> Result<Frame, FrameError> {
    let mut b = *bytes;
    deobfuscate(&mut b);
    if checksum(&b) != 0 {
        return Err(FrameError::BadChecksum);
    }
    let command = Command::from_nibble(b[1] >> 4).ok_or(FrameError::UnknownCommand)?;
    Ok(Frame {
        key: b[0],
        command,
        rolling_code: ((b[2] as u16) << 8) | b[3] as u16,
        address: ((b[4] as u32) << 16) | ((b[5] as u32) << 8) | (b[6] as u32),
    })
}

/// Encode an 80-bit RTS frame (10 bytes) carrying an extended command.
///
/// # Discovered byte map (ported from `ESPSomfy-RTS/src/Somfy.cpp`)
///
/// Bytes 0-6 are byte-for-byte the same **pre-obfuscation** layout as
/// [`encode56`] (`encodeFrame`, Somfy.cpp:335-341):
/// `[0]=key`, `[1]=cmd_nibble<<4`, `[2..3]=rolling code big-endian`,
/// `[4..6]=24-bit address big-endian`. The command field holds the *base*
/// nibble, not the extended value: extended commands deliberately reuse their
/// base command's nibble (`StepUp 0x8B -> StepDown 0xB`, `Favorite 0xC1` and
/// `Stop 0xF1 -> My 0x1`, from `Command::nibble()`), so the extended identity
/// lives in the tail bytes instead.
///
/// Bytes 7-9 are the extended payload from `encode80BitFrame`
/// (Somfy.cpp:263-331) and are transmitted **un-obfuscated** ("the last 3 bytes
/// are not encoded even on 80-bits", Somfy.cpp:130). See `encode80_tail` for
/// the exact per-command byte map.
///
/// Two checksums, both computed after the tail is filled:
/// - `[1]` low nibble: the 56-bit nibble checksum over bytes 0-6 only
///   (`i < 7`, Somfy.cpp:424-431).
/// - `[9]` low nibble: `calc80Checksum` over the raw tail bytes 7,8,9
///   (Somfy.cpp:119-125, applied at 272/287/294).
///
/// Finally the forward-XOR obfuscation is applied to bytes 1-6 only
/// (`i in 1..7`, Somfy.cpp:433-435); bytes 7-9 stay raw.
///
/// # Repeats
///
/// Encode an 80-bit frame for a given `repeat` index (0 = first frame). The
/// reference re-encodes the tail per repeat, so a transmitter MUST call this
/// once per frame it sends with the matching index — see `encode80BitFrame`
/// (Somfy.cpp:263-331).
///
/// # Limitations
///
/// - `Toggle`'s tail bytes 7-9 are reference-exact (Somfy.cpp:299-301), but
///   the reference's `Toggle` case also sets `frame[0] = 164` and
///   `frame[1] |= 0xF0` (Somfy.cpp:297-298), which this crate deliberately
///   does NOT reproduce: forcing the command nibble to `0xF` would collide
///   with `RtwProto = 0xF` in this crate's [`Command`] enum, so an encoded
///   `Toggle` frame would decode back as `RtwProto` and break round-tripping.
///   `Toggle` is also unreachable from `somfy-domain`. Callers must not rely
///   on an encoded `Toggle` frame being wire-identical to the reference.
/// - `SunFlag`, `Flag`, `Sensor` and `RtwProto` have no defined 80-bit tail in
///   the reference (`default: break`, Somfy.cpp:328-329): bytes 7-9 are left
///   zeroed for these commands. Callers must not transmit them as 80-bit
///   frames.
pub fn encode80(f: &Frame, repeat: u8) -> [u8; 10] {
    let mut b = [0u8; 10];
    b[0] = f.key;
    b[1] = f.command.nibble() << 4;
    b[2] = (f.rolling_code >> 8) as u8;
    b[3] = f.rolling_code as u8;
    b[4] = (f.address >> 16) as u8;
    b[5] = (f.address >> 8) as u8;
    b[6] = f.address as u8;
    encode80_tail(&mut b, f.command, repeat);

    b[1] |= checksum(&b[..7]);
    obfuscate(&mut b);
    b
}

pub fn decode80(bytes: &[u8; 10]) -> Result<Frame, FrameError> {
    let mut b = *bytes;
    deobfuscate(&mut b);
    if checksum(&b[..7]) != 0 {
        return Err(FrameError::BadChecksum);
    }
    // Second, independent parity checksum over the raw tail (Somfy.cpp:174).
    if b[9] & 0x0F != calc80_checksum(b[7], b[8], b[9]) {
        return Err(FrameError::BadChecksum);
    }
    // Reconstruct the full command from the base nibble plus the tail selector
    // (Somfy.cpp:177-179). Only the My and StepDown nibbles carry extensions.
    let base = b[1] >> 4;
    let cmd_val = match base {
        0x1 => base | ((b[8] & 0x0F) << 4),
        0xB => base | ((b[8] & 0x08) << 4),
        _ => base,
    };
    let command = Command::from_u8(cmd_val).ok_or(FrameError::UnknownCommand)?;
    Ok(Frame {
        key: b[0],
        command,
        rolling_code: ((b[2] as u16) << 8) | b[3] as u16,
        address: ((b[4] as u32) << 16) | ((b[5] as u32) << 8) | (b[6] as u32),
    })
}

/// Byte 7 progression across repeats (`encode80Byte7`, Somfy.cpp:259-262):
///
/// ```c
/// while((repeat * 4) + start > 255) repeat -= 15;
/// return start + (repeat * 4);
/// ```
///
/// The subtraction cycles the sequence with period 15 rather than saturating.
fn encode80_byte7(start: u8, repeat: u8) -> u8 {
    let mut r = repeat as i32;
    let s = start as i32;
    while (r * 4) + s > 255 {
        r -= 15;
    }
    (s + r * 4) as u8
}

/// Fill un-obfuscated tail bytes 7-9 plus the `calc80Checksum` low nibble of
/// byte 9, verbatim from `encode80BitFrame` (Somfy.cpp:263-331).
///
/// `stepSize` is fixed at 1 here: the C++ defaults it to 1 when unset
/// (Somfy.cpp:268, 276) and this crate has no step-size field. Callers needing
/// a non-default step must extend `Frame` first.
fn encode80_tail(b: &mut [u8; 10], cmd: Command, repeat: u8) {
    const STEP_SIZE: u8 = 1;
    match cmd {
        // Somfy.cpp:266-273. Byte 1 high nibble is rewritten to StepDown on the
        // first frame only; decode80 reverses this via the byte-8 selector.
        Command::StepUp => {
            if repeat == 0 {
                b[1] = (Command::StepDown.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = 132;
            b[8] = ((STEP_SIZE & 0x70) >> 4) | 0x38;
            b[9] = (STEP_SIZE & 0x0F) << 4;
        }
        // Somfy.cpp:274-281.
        Command::StepDown => {
            if repeat == 0 {
                b[1] = (Command::StepDown.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = 132;
            b[8] = ((STEP_SIZE & 0x70) >> 4) | 0x30;
            b[9] = (STEP_SIZE & 0x0F) << 4;
        }
        // Somfy.cpp:282-288.
        Command::Favorite => {
            if repeat == 0 {
                b[1] = (Command::My.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = if repeat > 0 { 132 } else { 196 };
            b[8] = 44;
            b[9] = 0x90;
        }
        // Somfy.cpp:289-295.
        Command::Stop => {
            if repeat == 0 {
                b[1] = (Command::My.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = if repeat > 0 { 132 } else { 196 };
            b[8] = 47;
            b[9] = 0xF0;
        }
        // Somfy.cpp:304-309.
        Command::Up => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 32;
            b[9] = 0x00;
        }
        // Somfy.cpp:310-315.
        Command::Down => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 44;
            b[9] = 0x80;
        }
        // My and the multi-button family share one tail (Somfy.cpp:316-326).
        Command::Prog
        | Command::UpDown
        | Command::MyDown
        | Command::MyUp
        | Command::MyUpDown
        | Command::My => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 0x00;
            b[9] = 0x10;
        }
        // Toggle's tail bytes match Somfy.cpp:299-301. The reference's Toggle
        // case additionally does `frame[0] = 164; frame[1] |= 0xF0`
        // (Somfy.cpp:297-298), which we deliberately do NOT reproduce — see
        // the "Limitations" section on `encode80`'s doc comment for why.
        Command::Toggle => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 0x00;
            b[9] = 0x10;
        }
        // SunFlag/Flag/Sensor/RtwProto fall into the reference's
        // `default: break` (Somfy.cpp:328-329): the C++ defines no 80-bit
        // tail for these commands at all, not even a checksum. Leave bytes
        // 7-9 at their zeroed default — the trailing `calc80Checksum` OR
        // below is a no-op on all-zero input, so this is the faithful
        // analogue of "break". Callers must not transmit these as 80-bit
        // frames; there is no reference wire format to match.
        Command::SunFlag | Command::Flag | Command::Sensor | Command::RtwProto => {}
    }
    b[9] |= calc80_checksum(b[7], b[8], b[9]);
}

/// 80-bit parity checksum over the three raw tail bytes (`calc80Checksum`,
/// Somfy.cpp:119-125): XOR of the high nibbles of all three plus the low
/// nibbles of the first two. Occupies the low nibble of byte 9.
fn calc80_checksum(b7: u8, b8: u8, b9: u8) -> u8 {
    ((b7 >> 4) ^ (b8 >> 4) ^ (b9 >> 4) ^ (b7 & 0x0F) ^ (b8 & 0x0F)) & 0x0F
}

/// XOR of all nibbles of the first 7 bytes; a valid frame's nibbles XOR to 0.
/// The 56-bit checksum spans only bytes 0-6 even inside an 80-bit frame
/// (Somfy.cpp:426 loops `i < 7`).
fn checksum(b: &[u8]) -> u8 {
    b[..7]
        .iter()
        .fold(0u8, |acc, x| acc ^ (x >> 4) ^ (x & 0x0F))
        & 0x0F
}

/// Forward-XOR obfuscation over bytes 1-6 (Somfy.cpp:433-435). On an 80-bit
/// frame bytes 7-9 are intentionally left untouched.
fn obfuscate(b: &mut [u8]) {
    for i in 1..7 {
        b[i] ^= b[i - 1];
    }
}

fn deobfuscate(b: &mut [u8]) {
    for i in (1..7).rev() {
        b[i] ^= b[i - 1];
    }
}
