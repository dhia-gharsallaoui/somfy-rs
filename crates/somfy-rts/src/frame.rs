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
}

/// Layout before obfuscation (matches ESPSomfy-RTS src/Somfy.cpp encodeFrame,
/// lines 335-341): [0]=key [1]=cmd<<4|cksum [2..3]=rolling code big-endian
/// (hi byte first) [4..6]=24-bit address big-endian (MSB at [4], LSB at [6]).
pub fn encode56(f: &Frame) -> [u8; 7] {
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
    b
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

/// XOR of all nibbles; a valid frame's nibbles XOR to 0.
fn checksum(b: &[u8; 7]) -> u8 {
    b.iter().fold(0u8, |acc, x| acc ^ (x >> 4) ^ (x & 0x0F)) & 0x0F
}

fn obfuscate(b: &mut [u8; 7]) {
    for i in 1..7 {
        b[i] ^= b[i - 1];
    }
}

fn deobfuscate(b: &mut [u8; 7]) {
    for i in (1..7).rev() {
        b[i] ^= b[i - 1];
    }
}
