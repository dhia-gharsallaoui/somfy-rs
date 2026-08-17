use heapless::String;

/// Fixed-point shade position in hundredths of a percent.
/// 0 = fully up/open, 10000 = fully closed. Deployed controllers track
/// position as a 0.0-100.0 floating-point percentage; this crate uses a
/// fixed-point integer instead so position math is deterministic and
/// reproducible without depending on float behavior — an intentional
/// deviation documented in the crate docs.
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

/// v1.0 shade kinds (spec §1.2). Discriminants match the shade-type byte
/// values used in deployed device backups, so a migrated backup's raw
/// byte maps directly onto a [`ShadeKind`] without a translation table.
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

impl ShadeKind {
    /// Map a raw shade-type byte from a device backup to a v1.0
    /// [`ShadeKind`].
    ///
    /// `None` covers two cases: a shade kind that deployed devices support
    /// but v1.0 does not yet model, or a byte that is not a valid
    /// shade-type value at all — Plan 6 policy: import such shades with
    /// kind defaulted to Roller and surface a warning to the user (decision
    /// recorded in README contracts). The kinds not yet supported are
    /// garage `0x05`/`0x06`, drycontact `0x09`/`0x0A`, and gate
    /// `0x0B`–`0x10`.
    pub fn from_raw(raw: u8) -> Option<ShadeKind> {
        match raw {
            0x00 => Some(ShadeKind::Roller),
            0x01 => Some(ShadeKind::Blind),
            0x02 => Some(ShadeKind::DraperyLeft),
            0x03 => Some(ShadeKind::Awning),
            0x04 => Some(ShadeKind::Shutter),
            0x07 => Some(ShadeKind::DraperyRight),
            0x08 => Some(ShadeKind::DraperyCenter),
            _ => None,
        }
    }
}

/// Tilt modes, matching the tilt-type byte values used in deployed device
/// backups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TiltMode {
    None = 0x00,
    TiltMotor = 0x01,
    Integrated = 0x02,
    TiltOnly = 0x03,
    EuroMode = 0x04,
}

impl TiltMode {
    /// Map a raw tilt-type byte from a device backup to a [`TiltMode`].
    ///
    /// `None` = invalid byte — Plan 6 policy: import such shades with kind
    /// defaulted to Roller and surface a warning to the user (decision
    /// recorded in README contracts). Every tilt-type value `0x00`-`0x04`
    /// used by deployed devices is modeled here, so only bytes outside
    /// that range return `None`.
    pub fn from_raw(raw: u8) -> Option<TiltMode> {
        match raw {
            0x00 => Some(TiltMode::None),
            0x01 => Some(TiltMode::TiltMotor),
            0x02 => Some(TiltMode::Integrated),
            0x03 => Some(TiltMode::TiltOnly),
            0x04 => Some(TiltMode::EuroMode),
            _ => None,
        }
    }
}

/// Movement direction. Signs match the convention deployed firmware uses
/// for its position-tracking integer: -1 toward 0 (open), +1 toward 100
/// (closed), 0 idle.
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
    /// Two entries claimed the same registry slot. Only
    /// [`Registry::add_shade_with_id`](crate::Registry::add_shade_with_id)
    /// raises it: the caller named an id, and the slot already holds a shade.
    /// Refused rather than overwritten, because overwriting would delete a
    /// provisioned shade to make room for one that asked for its place.
    DuplicateId,
    /// An id past the last slot the registry has
    /// ([`MAX_SHADES`](crate::MAX_SHADES) is one past it). Distinct from
    /// [`DomainError::RegistryFull`] on purpose: a full registry may be fixed
    /// by removing a shade, and an id of 200 in a 32-slot registry cannot be
    /// fixed by anything except correcting the id.
    IdOutOfRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadeConfig {
    pub name: String<32>,
    pub address: u32,
    pub kind: ShadeKind,
    /// Tilt mode. **TRAP: only [`TiltMode::None`] has command semantics in
    /// Plan 2.** Every other variant is *config-carriage only* — it is stored
    /// and round-tripped for backup/migration but drives no behavior yet: the
    /// [`Shade`](crate::Shade) command path treats all shades as lift-only and
    /// [`Shade::tilt_pos`](crate::Shade::tilt_pos) is always [`Pos::ZERO`].
    ///
    /// Each tilt mode needs genuinely different behavior to actually move a
    /// tilt axis: `TiltOnly` and `EuroMode` redirect a long-pressed Up/Down
    /// onto the tilt axis instead of the lift axis, while `TiltMotor` and
    /// `EuroMode` need a half-second hold before a press resolves into a
    /// tilt command, to disambiguate it from a lift command. So wiring
    /// these up is a real behavior port, not a config toggle. Plan 3's API
    /// MUST NOT surface tilt as functional until that port lands, or it
    /// will advertise moves the domain does not make.
    pub tilt_mode: TiltMode,
    pub up_time_ms: u32,
    pub down_time_ms: u32,
    pub tilt_time_ms: u32,
}

impl ShadeConfig {
    /// Defaults: 10s up, 10s down, 7s tilt — the same factory defaults
    /// deployed devices ship with, so a migrated setup behaves identically.
    /// Address guard: 0 and 0xFFFFFF are invalid sentinel addresses used by
    /// deployed devices to mean "unset", so they must be rejected here too.
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
