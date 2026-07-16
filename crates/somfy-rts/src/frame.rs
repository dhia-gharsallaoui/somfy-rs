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
/// are not encoded even on 80-bits", Somfy.cpp:130). Per command (this crate
/// emits the first-frame / `repeat == 0` form):
///
/// | cmd            | `[7]`      | `[8]`                    | `[9]` (hi nibble) |
/// |----------------|------------|--------------------------|-------------------|
/// | StepUp (0x8B)  | 132 (0x84) | `((step&0x70)>>4)|0x38`   | `(step&0x0F)<<4`  |
/// | Favorite(0xC1) | 196 (0xC4) | 44 (0x2C)                | 0x9_             |
/// | Stop   (0xF1)  | 196 (0xC4) | 47 (0x2F)                | 0xF_             |
/// | base cmds      | 132 (0x84) | 0                        | 0x1_ (placeholder)|
///
/// **Caveats.**
/// - The three EXTENDED rows (`StepUp`/`Favorite`/`Stop`) are C++-exact for
///   **first frames** (`repeat == 0`).
/// - **The `base cmds` row is a PLACEHOLDER, not the C++ wire bytes.** This
///   crate emits `[7]=132, [8]=0, [9] hi=0x1` for every base command so it
///   round-trips through [`decode80`]; the reference firmware instead gives
///   Up/Down/Toggle/etc. distinct tails and uses `[7]=196` as its My-family
///   default (Somfy.cpp:322-325).
/// - **No per-repeat progression.** C++ re-encodes byte 7 each repeat
///   (`196 + 4*repeat`, with Favorite/Stop flipping `196->132` on later
///   repeats). This repeat-less API cannot express that: `encode80` must grow a
///   `repeat` parameter before hardware TX of these frames (Plan 4 contract).
///
/// StepUp defaults `step` to 1 (Somfy.cpp:268) -> `[8]=0x38, [9]=0x10`. The
/// selector recovered at decode is: for base `My (0x1)`,
/// `cmd = 0x1 | ((decoded[8] & 0x0F) << 4)` (Somfy.cpp:177; `0x0`=My,
/// `0xC`=Favorite, `0xF`=Stop); for base `StepDown (0xB)`,
/// `cmd = 0xB | ((decoded[8] & 0x08) << 4)` — bit `0x08` of `[8]` is what tells
/// StepUp (`0x38`) from StepDown (`0x30`) (Somfy.cpp:179).
///
/// Two checksums, both computed after the tail is filled:
/// - `[1]` low nibble: the 56-bit nibble checksum over bytes 0-6 only
///   (`i < 7`, Somfy.cpp:424-431).
/// - `[9]` low nibble: `calc80Checksum` over the raw tail bytes 7,8,9
///   (Somfy.cpp:119-125, applied at 272/287/294).
///
/// Finally the forward-XOR obfuscation is applied to bytes 1-6 only
/// (`i in 1..7`, Somfy.cpp:433-435); bytes 7-9 stay raw.
pub fn encode80(f: &Frame) -> [u8; 10] {
    let mut b = [0u8; 10];
    b[0] = f.key;
    b[1] = f.command.nibble() << 4;
    b[2] = (f.rolling_code >> 8) as u8;
    b[3] = f.rolling_code as u8;
    b[4] = (f.address >> 16) as u8;
    b[5] = (f.address >> 8) as u8;
    b[6] = f.address as u8;
    encode80_tail(&mut b, f.command);

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

/// Fill the un-obfuscated tail bytes 7-9 and their `calc80Checksum`
/// (`encode80BitFrame`, Somfy.cpp:263-331). See [`encode80`] for the map.
fn encode80_tail(b: &mut [u8; 10], cmd: Command) {
    match cmd {
        Command::StepUp => {
            // stepSize defaults to 1 (Somfy.cpp:268): [8]=0x38, [9] hi=0x10.
            b[7] = 132;
            b[8] = 0x38;
            b[9] = 0x10;
        }
        Command::Favorite => {
            b[7] = 196;
            b[8] = 44;
            b[9] = 0x90;
        }
        Command::Stop => {
            b[7] = 196;
            b[8] = 47;
            b[9] = 0xF0;
        }
        // Base commands: PLACEHOLDER tail that round-trips through this crate's
        // own decode80 but does NOT match the C++ wire bytes. The reference
        // firmware does not emit a single fixed base tail: `encode80BitFrame`
        // gives Up/Down/Toggle/etc. distinct tails, uses `[7]=196` (not 132) as
        // its My-family default (Somfy.cpp:322-325), and progresses byte 7 per
        // repeat (`196 + 4*repeat`, with Favorite/Stop flipping 196->132 on
        // later repeats). This crate's repeat-less API cannot express that
        // progression yet — encode80 must grow a `repeat` parameter before any
        // hardware TX of base commands as 80-bit (recorded Plan 4 contract).
        // (The three EXTENDED arms above ARE C++-exact for first frames.) A zero
        // low nibble in [8] keeps the My (0x1) and StepDown (0xB) nibbles
        // un-translated at decode, so any base command still round-trips here.
        _ => {
            b[7] = 132;
            b[8] = 0;
            b[9] = 0x10;
        }
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
