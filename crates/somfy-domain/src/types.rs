use heapless::String;

/// Fixed-point shade position in hundredths of a percent.
/// 0 = fully up/open, 10000 = fully closed. Deterministic integer
/// replacement for the C++ float positions (Somfy.h:295, 0.0-100.0);
/// intentional deviation documented in the crate docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos(u16);

impl Pos {
    pub const ZERO: Pos = Pos(0);
    pub const FULL: Pos = Pos(10_000);

    pub fn from_raw(raw: u16) -> Pos {
        Pos(raw.min(10_000))
    }

    pub fn from_percent(pct: u8) -> Pos {
        Pos((pct as u16).min(100) * 100)
    }

    pub fn raw(self) -> u16 {
        self.0
    }

    pub fn percent(self) -> u8 {
        (self.0 / 100) as u8
    }
}

/// v1.0 shade kinds (spec §1.2). Discriminants mirror the C++
/// shade_types enum (Somfy.h:56-74) for backup migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShadeKind {
    Roller = 0x00,
    Blind = 0x01,
    DraperyLeft = 0x02,
    Awning = 0x03,
    Shutter = 0x04,
    DraperyRight = 0x07,
    DraperyCenter = 0x08,
}

/// Tilt modes (Somfy.h:75-81).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TiltMode {
    None = 0x00,
    TiltMotor = 0x01,
    Integrated = 0x02,
    TiltOnly = 0x03,
    EuroMode = 0x04,
}

/// Movement direction. Signs match the C++ ints: -1 toward 0 (open),
/// +1 toward 100 (closed), 0 idle (Somfy.cpp:1071).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Idle,
    Down,
}

impl Direction {
    pub fn sign(self) -> i8 {
        match self {
            Direction::Up => -1,
            Direction::Idle => 0,
            Direction::Down => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    InvalidAddress,
    NameTooLong,
    RegistryFull,
    DuplicateAddress,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadeConfig {
    pub name: String<32>,
    pub address: u32,
    pub kind: ShadeKind,
    pub tilt_mode: TiltMode,
    pub up_time_ms: u32,
    pub down_time_ms: u32,
    pub tilt_time_ms: u32,
}

impl ShadeConfig {
    /// Defaults mirror Somfy.h:314-316 (10s/10s travel, 7s tilt).
    /// Address guard mirrors Somfy.cpp:169-170: 0 and 0xFFFFFF are
    /// invalid sentinels.
    pub fn new(name: &str, address: u32) -> Result<ShadeConfig, DomainError> {
        if address == 0 || address >= 0xFF_FFFF {
            return Err(DomainError::InvalidAddress);
        }
        let mut n: String<32> = String::new();
        n.push_str(name).map_err(|_| DomainError::NameTooLong)?;
        Ok(ShadeConfig {
            name: n,
            address,
            kind: ShadeKind::Roller,
            tilt_mode: TiltMode::None,
            up_time_ms: 10_000,
            down_time_ms: 10_000,
            tilt_time_ms: 7_000,
        })
    }
}
