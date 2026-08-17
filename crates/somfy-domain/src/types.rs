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

/// How wide a frame the motor behind a shade was paired as.
///
/// A motor learns a remote at one width and answers nothing else, so this is a
/// property of the *installation*, not a preference: a shade paired as an
/// 80-bit device is deaf to every 56-bit frame, and a controller that sends the
/// wrong one produces a shade that imports looking healthy and never moves.
///
/// Discriminants are the bit counts themselves, which is also the byte deployed
/// device backups store, so a migrated width needs no translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameWidth {
    /// The 7-byte frame nearly every RTS motor in the field uses.
    Bits56 = 56,
    /// The 10-byte extended frame.
    Bits80 = 80,
}

impl FrameWidth {
    /// Map a raw bit-length byte from a device backup or a stored record.
    ///
    /// `None` for anything that is not one of the two widths the protocol has.
    /// Reported rather than defaulted, for the same reason
    /// [`ShadeKind::from_raw`] reports: a shade silently re-widthed is a shade
    /// that stops responding, and nothing says why.
    pub fn from_raw(raw: u8) -> Option<FrameWidth> {
        match raw {
            56 => Some(FrameWidth::Bits56),
            80 => Some(FrameWidth::Bits80),
            _ => None,
        }
    }
}

/// Which radio protocol a shade speaks.
///
/// Only [`RadioProtocol::Rts`] is the one this firmware transmits — see
/// [`ShadeConfig::protocol`] for what carrying the others buys. The
/// discriminants are the protocol bytes deployed device backups store, so a
/// migrated value needs no translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RadioProtocol {
    /// Somfy RTS — what `somfy-rts` encodes and what every motor this project
    /// has transmitted at is paired as.
    Rts = 0x00,
    /// Somfy RTW.
    Rtw = 0x01,
    /// Somfy RTV.
    Rtv = 0x02,
    /// A general-purpose relay output rather than a motor.
    GpRelay = 0x08,
    /// A general-purpose remote input rather than a motor.
    GpRemote = 0x09,
}

impl RadioProtocol {
    /// Map a raw protocol byte from a device backup or a stored record.
    ///
    /// `None` for a byte outside the set above.
    pub fn from_raw(raw: u8) -> Option<RadioProtocol> {
        match raw {
            0x00 => Some(RadioProtocol::Rts),
            0x01 => Some(RadioProtocol::Rtw),
            0x02 => Some(RadioProtocol::Rtv),
            0x08 => Some(RadioProtocol::GpRelay),
            0x09 => Some(RadioProtocol::GpRemote),
            _ => None,
        }
    }
}

/// Whether a person has reported that this shade actually works.
///
/// # This is a fact about a human's report, and nothing else
///
/// **The device cannot observe pairing and this type does not claim it does.**
/// RTS is one-way: a `Prog` burst goes out and nothing comes back, so no
/// controller can learn whether a motor accepted it. A field called `paired`
/// would therefore be a belief stored as a fact, and it would keep saying
/// "paired" long after somebody reset the motor — which is why there is no such
/// field anywhere in this workspace.
///
/// What *is* observable is what an operator told us. Somebody stood at the
/// shade, pressed Open, watched it move, and said so. That is evidence, it is
/// evidence about the path the user will actually use, and — crucially — it is
/// evidence this controller acquired legitimately rather than inferred from its
/// own transmission. The variant names say whose knowledge it is, so that a
/// reader of the field cannot mistake it for a measurement:
/// [`ConfirmedByOperator`](PairingState::ConfirmedByOperator), never `Paired`.
///
/// # What it gates
///
/// Announcing the shade's entities to Home Assistant. A shade that has been
/// created and not confirmed exists, is commandable over the local API — which
/// is how the confirmation is obtained — and has **no entities**, because an
/// entity that accepts commands and drives nothing is exactly the failure this
/// project exists to avoid.
///
/// # The discriminants are stored
///
/// They are written into the persisted shade record, so they may not be
/// reordered or renumbered. `somfy-config`'s record format carries the
/// migration rule for tables written before this field existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PairingState {
    /// Nobody has reported this shade working yet. It may never have been
    /// paired, or the pairing may have been attempted and not checked — the
    /// two are indistinguishable to a one-way transmitter, and this variant
    /// deliberately does not distinguish them.
    AwaitingConfirmation = 0x00,
    /// An operator reported that the shade responded to a command. Not a
    /// measurement, not an acknowledgement from the motor: a person's account,
    /// stored as one.
    ConfirmedByOperator = 0x01,
}

impl PairingState {
    /// Map a raw byte from a stored record.
    ///
    /// `None` for anything outside the set, reported rather than defaulted for
    /// the same reason [`ShadeKind::from_raw`] reports: both possible defaults
    /// are wrong in a way somebody pays for, one by hiding a working shade and
    /// one by announcing a dead one.
    pub fn from_raw(raw: u8) -> Option<PairingState> {
        match raw {
            0x00 => Some(PairingState::AwaitingConfirmation),
            0x01 => Some(PairingState::ConfirmedByOperator),
            _ => None,
        }
    }

    /// Whether an operator has reported this shade working.
    pub const fn is_confirmed(self) -> bool {
        matches!(self, PairingState::ConfirmedByOperator)
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
    /// A command that only means something for one shade at a time was aimed
    /// at a group.
    ///
    /// Raised only by
    /// [`Controller::command_group`](crate::Controller::command_group), and
    /// only for [`ShadeCommand::Pair`](crate::ShadeCommand::Pair). Pairing
    /// depends on a person standing at *one* motor having just put it into
    /// programming mode; fanned across a group it becomes a `Prog` burst at
    /// every shade in the house with nobody at any of them. Every other command
    /// here is a movement somebody can watch and undo.
    NotAGroupCommand,
    /// Every address the allocator could offer is already in the table.
    ///
    /// Raised only by
    /// [`allocate_if_absent`](crate::allocate_if_absent). Unreachable through a
    /// registry — it holds at most [`MAX_SHADES`](crate::MAX_SHADES) addresses
    /// and the allocator probes one more candidate than that — and reported
    /// rather than asserted, because the alternative to an error here is a
    /// silently wrong address.
    AddressUnavailable,
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
    /// The frame width the motor behind this shade was paired as.
    ///
    /// **Carried, not yet honoured on the wire.** The transmit width is still
    /// per-controller (`somfy_tasks::TxProfile`), so a shade whose width
    /// differs from the controller's is one the controller cannot drive. That
    /// is the state this field exists to make *visible*: before it, the width
    /// was read out of a backup, reported once, and dropped, so the shade
    /// imported looking healthy and never moved.
    pub frame_width: FrameWidth,
    /// The radio protocol this shade speaks.
    ///
    /// Carried for the same reason as [`ShadeConfig::frame_width`], and with a
    /// sharper edge: `somfy-rts` implements [`RadioProtocol::Rts`] and nothing
    /// else, so a shade set to any other value cannot be driven at all by any
    /// configuration of this firmware. Storing it is what lets the device say
    /// so instead of transmitting frames the motor is not listening for.
    pub protocol: RadioProtocol,
    /// Whether an operator has reported this shade working. See
    /// [`PairingState`] — it is **not** a claim that the motor was paired, and
    /// nothing here can make that claim.
    ///
    /// A public field like every other one here, and changed through
    /// [`Shade::confirm_pairing`](crate::Shade::confirm_pairing) rather than by
    /// assignment on a live shade: like `address`, it is a field
    /// [`Shade::reconfigure`](crate::Shade::reconfigure) refuses to take from
    /// an incoming configuration, so a rename cannot confirm a shade and a
    /// corrected travel time cannot un-confirm one.
    pub pairing_state: PairingState,
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
            // The width and protocol this firmware transmits. A shade built by
            // hand is one this controller is about to pair, so it is one this
            // controller can drive; an imported shade overwrites both from what
            // the backup recorded.
            frame_width: FrameWidth::Bits56,
            protocol: RadioProtocol::Rts,
            // **Nobody has said this shade works, because it does not exist
            // yet.** Every other default here is a value that behaves
            // reasonably if left alone; this one is the absence of a report,
            // and it is the only honest starting value — a constructor cannot
            // know that a motor obeys an address it is being handed for the
            // first time. The two readers that *do* know better say so
            // explicitly: the record decoder, for a table written before this
            // field existed, and the provisioning tool, for an address it did
            // not allocate.
            pairing_state: PairingState::AwaitingConfirmation,
        })
    }
}
