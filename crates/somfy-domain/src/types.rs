use heapless::String;
use somfy_rts::Command;

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
/// It travels with every frame the domain plans — [`PlannedTx::width`](crate::PlannedTx)
/// — so the width a shade is driven at is the width its own record names, and
/// there is no controller-wide setting for it to disagree with.
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

    /// Whether a frame of this width can carry `command` on the wire at all.
    ///
    /// The three extended commands — `StepUp`, `Favorite`, `Stop` — do not have
    /// a 4-bit command field to live in. Their identity is in the extended
    /// frame's un-obfuscated tail, and the nibble a narrow frame *would* carry
    /// is their base command's, which for `StepUp` is `StepDown`: the opposite
    /// direction. So a narrow frame cannot express them, and encoding one
    /// narrow is not a degraded send but a different command — which is why
    /// `somfy_rts::encode56` refuses rather than truncates.
    ///
    /// This is the whole rule, in one place, so that the guard which stops such
    /// a command being planned and the encoder which would refuse it cannot
    /// drift apart.
    pub fn carries(self, command: Command) -> bool {
        match self {
            FrameWidth::Bits80 => true,
            FrameWidth::Bits56 => !command.is_extended(),
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

/// Where a travel time came from.
///
/// # Why the number alone is not enough
///
/// On 2026-08-17 a command for 25% open moved a shade about 1%. All three
/// shades in the estate carried 10000/10000/7000 — values nobody had ever
/// chosen, carried across from a previous controller because they had never
/// been changed there either, and presented as though they were settings. The
/// position estimate is computed from these numbers, so a number nobody chose
/// is an estimate nobody should believe.
///
/// A value cannot be classified by looking at it. 10000 might be a factory
/// default or a shade that genuinely takes ten seconds, and those two deserve
/// opposite treatment. So the provenance is **recorded when the value is set**
/// and stored beside it, rather than inferred afterwards.
///
/// # What each variant licenses
///
/// [`Shade::confidence`](crate::Shade::confidence) reads this directly: a
/// factory default contributes its whole span to the uncertainty of every
/// partial move, because the number is not evidence of anything. See
/// [`CalibrationSource::relative_error_raw`].
///
/// # The discriminants are stored
///
/// They are written into the persisted shade record, so they may not be
/// reordered or renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CalibrationSource {
    /// Nobody chose it. Equal to what a shade is created with, and to what
    /// deployed devices ship with — which is why a migrated table is full of
    /// them.
    FactoryDefault = 0x00,
    /// A person typed it, here or on a device this setup was migrated from.
    OperatorSupplied = 0x01,
    /// The device timed the shade itself, through
    /// [`Shade::finish_calibration`](crate::Shade::finish_calibration).
    Measured = 0x02,
}

impl CalibrationSource {
    /// Map a raw byte from a stored record.
    ///
    /// `None` for anything outside the set, reported rather than defaulted for
    /// the same reason [`ShadeKind::from_raw`] reports.
    pub fn from_raw(raw: u8) -> Option<CalibrationSource> {
        match raw {
            0x00 => Some(CalibrationSource::FactoryDefault),
            0x01 => Some(CalibrationSource::OperatorSupplied),
            0x02 => Some(CalibrationSource::Measured),
            _ => None,
        }
    }

    /// How wrong a travel time from this source may be, as raw [`Pos`] units of
    /// error accumulated over one full traverse.
    ///
    /// **These are policy figures and are not measurements.** Nothing available
    /// to a one-way controller can price the error in its own travel time; what
    /// the three numbers encode is an ordering — a value nobody chose deserves
    /// no confidence at all, a stopwatch is good to a few percent, and the
    /// device's own clock read through a human's tap is better than a stopwatch
    /// but not by much, because the tap is the slow part of both.
    ///
    /// [`FactoryDefault`](CalibrationSource::FactoryDefault) is the full span
    /// deliberately: one partial move on an uncalibrated shade saturates
    /// [`Shade::confidence`](crate::Shade::confidence), which is the correct
    /// report — the estimate is worth nothing, and that is exactly the state
    /// three shades were in when a 25% command moved one of them 1%.
    pub const fn relative_error_raw(self) -> u16 {
        match self {
            // 100% of the span.
            CalibrationSource::FactoryDefault => 10_000,
            // 5%.
            CalibrationSource::OperatorSupplied => 500,
            // 2%.
            CalibrationSource::Measured => 200,
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
    /// A vent was asked for on a shade whose slat-separation band has never been
    /// measured.
    ///
    /// Raised by [`Controller::command_shade`](crate::Controller::command_shade)
    /// before anything is planned. The vent position **is**
    /// [`ShadeConfig::vent_band_ms`] — it is not derived from a position
    /// estimate and there is nothing else to fall back on — so a zero band makes
    /// the command a full close followed by an Up and an immediate stop, which
    /// leaves the shade shut and looks like the command did nothing.
    ///
    /// Refused rather than approximated: the whole point of venting from the
    /// closed limit is that it depends on one measured number instead of on an
    /// estimate, and substituting a guess for that number gives up the only
    /// thing the design bought.
    VentBandNotMeasured,
    /// A start lag and a dead band that together consume their direction's whole
    /// travel time, leaving no phase in which the curtain moves. See
    /// [`ShadeConfig::checked_bands`].
    DeadBandTooLong,
    /// A calibration run was marked or finished when none was running. See
    /// [`Shade::begin_calibration`](crate::Shade::begin_calibration).
    NotCalibrating,
    /// A calibration run produced numbers this crate will not store: a traverse
    /// of zero or past
    /// [`MAX_TRAVEL_TIME_MS`](crate::MAX_TRAVEL_TIME_MS), or a lag or band past
    /// what the model can express.
    ///
    /// The run is discarded and the shade left exactly as it was, because a
    /// half-applied calibration — a new up time against an old band — is worse
    /// than no calibration at all.
    CalibrationImplausible,
    /// More shades are already part-way through a multi-step movement than this
    /// controller can track at once. See
    /// [`MAX_ACTIVITIES`](crate::MAX_ACTIVITIES) for the bound and for the
    /// measurement that forces it.
    ///
    /// Reached by a vent, or a go-to-position routed via a limit, on a
    /// controller that already has four of them running — which in practice
    /// means a group vent of more than four shades. Refused rather than dropped
    /// silently: a shade that closed fully and never vented is a shade somebody
    /// has to go and open.
    TooManySequences,
    /// A command was asked for on a shade whose paired frame width has no way
    /// to express it. Today that is [`ShadeCommand`](crate::ShadeCommand)`::StepUp`
    /// on a 56-bit shade, and only that.
    ///
    /// Raised by [`Controller::command_shade`](crate::Controller::command_shade)
    /// **before anything is planned**, and by
    /// [`Controller::command_group`](crate::Controller::command_group) across
    /// the whole group before any member moves.
    ///
    /// The narrow frame has one 4-bit field for the command, and `StepUp` has no
    /// value in it — the nibble it would occupy is `StepDown`'s. So there is no
    /// degraded send available here, only a different command, in the opposite
    /// direction, with the estimate moving the way that was asked for and the
    /// motor moving the other. Refused rather than approximated, and refused in
    /// the domain rather than at the encoder so that the position estimate does
    /// not move either: a silently-inverted step is worse than a step that did
    /// not happen, and a step the estimate believes in is worse than both.
    ///
    /// See [`FrameWidth::carries`].
    CommandNotAtThisWidth,
}

/// The travel-time defaults a shade is created with, which are also the ones
/// deployed devices ship with.
///
/// Named constants rather than literals inside [`ShadeConfig::new`] because
/// three separate readers have to recognise them: the record decoder, when it
/// reconstructs provenance for a table written before provenance was stored; the
/// API, which surfaces a value equal to one of these as *uncalibrated*; and the
/// mock server the web UI is developed against. A wrong copy of this number in
/// any of them misclassifies a measured value as one nobody chose, or worse, the
/// other way round.
pub const FACTORY_UP_TIME_MS: u32 = 10_000;
/// See [`FACTORY_UP_TIME_MS`].
pub const FACTORY_DOWN_TIME_MS: u32 = 10_000;
/// See [`FACTORY_UP_TIME_MS`].
pub const FACTORY_TILT_TIME_MS: u32 = 7_000;

/// Resolution of [`ShadeConfig::start_lag_ms`], in milliseconds.
///
/// **The resolution is the measurement's, not the storage's.** A start lag is
/// obtained by a person tapping a button as the shade begins to move, and a
/// human tap lands within a couple of hundred milliseconds of what it aims at.
/// Ten milliseconds is already an order of magnitude finer than the thing being
/// measured; recording it more precisely would be recording noise. The persisted
/// record then holds it in one byte *because* it is that coarse.
pub const START_LAG_RESOLUTION_MS: u32 = 10;

/// Largest [`ShadeConfig::start_lag_ms`] this model accepts.
///
/// The lag has a computable floor and a physical ceiling. The floor is the air
/// time of the command burst — a 56-bit frame is about 106 ms of wake-up, sync
/// and data, and a motor cannot act on a frame it has not finished hearing. The
/// ceiling is the motor's soft-start ramp on top of that, which is under a
/// second on every RTS motor this project has driven. 2,550 ms is what one byte
/// at [`START_LAG_RESOLUTION_MS`] reaches, and it is comfortably past both.
pub const MAX_START_LAG_MS: u32 = 255 * START_LAG_RESOLUTION_MS;

/// Resolution of the two dead bands, in milliseconds.
///
/// Same argument as [`START_LAG_RESOLUTION_MS`], one decade coarser because the
/// quantity is one decade larger: a slat-separation band is seconds, measured by
/// a person watching for the moment the curtain starts to rise, which is a
/// judgement good to perhaps a quarter of a second.
pub const DEAD_BAND_RESOLUTION_MS: u32 = 100;

/// Largest dead band this model accepts.
///
/// A dead band is a *part* of its direction's travel time, so the binding check
/// is not this one but [`ShadeConfig::checked_bands`], which refuses a band that
/// does not leave travel behind it. This bound is what one byte at
/// [`DEAD_BAND_RESOLUTION_MS`] reaches, and 25.5 s is longer than any full
/// traverse in the estate that produced the requirement.
pub const MAX_DEAD_BAND_MS: u32 = 255 * DEAD_BAND_RESOLUTION_MS;

/// Round a start lag onto [`START_LAG_RESOLUTION_MS`], or refuse it.
///
/// Rounding happens **at the boundary a value enters through**, not on the way
/// to flash, so that what the device is running is always what a reboot would
/// load. The alternative — keep the typed value in memory and quantise it on the
/// way out — leaves the running estimate and the stored one disagreeing by up to
/// half a quantum, with nothing saying which is which.
pub const fn round_start_lag_ms(ms: u32) -> Option<u16> {
    round_onto(ms, START_LAG_RESOLUTION_MS, MAX_START_LAG_MS)
}

/// Round a dead band onto [`DEAD_BAND_RESOLUTION_MS`], or refuse it. See
/// [`round_start_lag_ms`].
pub const fn round_dead_band_ms(ms: u32) -> Option<u16> {
    round_onto(ms, DEAD_BAND_RESOLUTION_MS, MAX_DEAD_BAND_MS)
}

/// Round to the nearest multiple of `step`, refusing anything past `max`.
///
/// Refused rather than clamped: a value past the ceiling is somebody meaning
/// something this model cannot express, and silently substituting the ceiling
/// would be a position estimate computed from a number nobody entered.
const fn round_onto(ms: u32, step: u32, max: u32) -> Option<u16> {
    if ms > max {
        return None;
    }
    let rounded = ((ms + step / 2) / step) * step;
    // `rounded` cannot exceed `max` — `ms <= max` and `max` is a multiple of
    // `step`, so rounding to nearest cannot cross it — and `max` fits `u16`.
    Some(rounded as u16)
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
    /// Wall-clock milliseconds for a **full traverse** toward fully open:
    /// command issued to motor stopped, which is what a stopwatch measures and
    /// what [`Shade::finish_calibration`](crate::Shade::finish_calibration)
    /// records.
    ///
    /// [`ShadeConfig::start_lag_ms`] and [`ShadeConfig::vent_band_ms`] are parts
    /// *of* this, not additions to it — see [`TravelProfile::up_span_ms`]. That
    /// is what keeps a hand-entered figure meaning the same thing after a dead
    /// band is measured: adding one shortens the lifting phase inside an
    /// unchanged total rather than lengthening the total.
    pub up_time_ms: u32,
    /// Wall-clock milliseconds for a full traverse toward fully closed. See
    /// [`ShadeConfig::up_time_ms`]; [`ShadeConfig::close_band_ms`] is a part of
    /// this one.
    pub down_time_ms: u32,
    pub tilt_time_ms: u32,
    /// Where [`ShadeConfig::up_time_ms`] came from. See [`CalibrationSource`].
    pub up_time_source: CalibrationSource,
    /// Where [`ShadeConfig::down_time_ms`] came from.
    pub down_time_source: CalibrationSource,
    /// Where [`ShadeConfig::tilt_time_ms`] came from.
    pub tilt_time_source: CalibrationSource,
    /// Milliseconds between a command being planned and the motor moving.
    ///
    /// Two things in series: the command burst's air time, which is fixed by the
    /// protocol and identical for every shade, and the motor's soft-start ramp,
    /// which is not. Neither is separable from the other by any measurement a
    /// one-way controller can make, so this is the sum and is stored per shade.
    ///
    /// **It applies at both ends of a move.** The first `start_lag_ms` after a
    /// command produce no motion, and a stop frame takes the same time to land —
    /// so the estimator declines to integrate the opening lag and plans the
    /// arrival stop that much early. On a 30 s traverse either correction is
    /// noise; on the 2.5 s a 25%-open command used to run for, they are the
    /// difference between a quarter and a third of it being real.
    ///
    /// Zero by default, which reproduces the un-compensated model exactly.
    /// Always a multiple of [`START_LAG_RESOLUTION_MS`]; see
    /// [`round_start_lag_ms`].
    pub start_lag_ms: u16,
    /// Milliseconds an **Up** command spends separating the slats at the closed
    /// limit before the curtain begins to rise.
    ///
    /// On a European roller shutter with perforated slats the slats are
    /// compressed shut at the bottom of the travel, and the first seconds of Up
    /// open the light gaps without lifting anything. Measured on the estate that
    /// produced this requirement: about 4 s of a 30 s traverse, so roughly 13%
    /// of a commanded Up produces no elevation at all — which is a second,
    /// independent reason a "25% open" command lifted almost nothing, on top of
    /// the uncalibrated travel time.
    ///
    /// It is a **part of** [`ShadeConfig::up_time_ms`], and it applies only when
    /// a move starts at [`Pos::FULL`] — anywhere else the slats are already
    /// apart.
    ///
    /// It is also the one number [`ShadeCommand::Vent`](crate::ShadeCommand)
    /// needs, and a vent is refused while it is zero: see
    /// [`DomainError::VentBandNotMeasured`].
    ///
    /// Always a multiple of [`DEAD_BAND_RESOLUTION_MS`]; see
    /// [`round_dead_band_ms`].
    pub vent_band_ms: u16,
    /// Milliseconds a **Down** command spends compressing the slats at the
    /// closed limit *after* the curtain has reached the sill.
    ///
    /// The mirror of [`ShadeConfig::vent_band_ms`], at the other end of the same
    /// travel: a part of [`ShadeConfig::down_time_ms`] during which the position
    /// no longer changes because it is already [`Pos::FULL`]. Leaving it out
    /// makes every downward seek run slow by its length spread across the whole
    /// range.
    pub close_band_ms: u16,
    /// The frame width the motor behind this shade was paired as.
    ///
    /// **Honoured on the wire.** Every frame this shade plans carries it
    /// ([`PlannedTx::width`](crate::PlannedTx)), and the radio encodes to it, so
    /// an installation may mix widths and each motor hears the one it was
    /// paired at. There is no controller-wide width to disagree with — the
    /// record is the only thing that decides.
    ///
    /// It also decides which commands are available: the extended commands live
    /// only in the wide frame, so [`FrameWidth::carries`] is what stops a
    /// `StepUp` being planned for a narrow shade, where the nibble it would
    /// occupy means `StepDown`.
    pub frame_width: FrameWidth,
    /// The radio protocol this shade speaks.
    ///
    /// **Carried, not honoured — and unlike [`ShadeConfig::frame_width`] it
    /// cannot be**, which is why the two fields part company here. `somfy-rts`
    /// encodes [`RadioProtocol::Rts`] and has no byte layout for the others at
    /// either width, so a shade set to any other value is one no configuration
    /// of this firmware can drive. Storing it is what lets the device name that
    /// shade at boot instead of transmitting frames its motor is not listening
    /// for.
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
            up_time_ms: FACTORY_UP_TIME_MS,
            down_time_ms: FACTORY_DOWN_TIME_MS,
            tilt_time_ms: FACTORY_TILT_TIME_MS,
            // **Said out loud rather than left to be inferred from the numbers
            // above.** The three defaults are the same ones deployed devices
            // ship with, and three shades carrying them unchanged are what
            // produced a 25%-open command that moved a shade 1%. Recording the
            // provenance here is what lets everything downstream say
            // *uncalibrated* instead of presenting a value nobody chose as a
            // setting.
            up_time_source: CalibrationSource::FactoryDefault,
            down_time_source: CalibrationSource::FactoryDefault,
            tilt_time_source: CalibrationSource::FactoryDefault,
            // Zero reproduces the un-compensated linear model exactly, which is
            // the only honest starting value: nothing has been measured yet, and
            // a guessed dead band would move every estimate on the strength of a
            // number nobody took.
            start_lag_ms: 0,
            vent_band_ms: 0,
            close_band_ms: 0,
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

    /// Check that the lag and the two dead bands leave real travel behind them.
    ///
    /// Both bands and the lag are **parts of** their direction's travel time, so
    /// the phase that actually moves the curtain is what is left over. A shade
    /// whose lag plus band consumed the whole traverse would have a zero-length
    /// lifting phase, which [`TravelProfile`] answers by jumping straight to the
    /// target — a shade that reports arriving instantly and never does.
    ///
    /// Refused rather than clamped, for the reason this crate refuses a zero
    /// travel time: a substituted value is a wrong one, and it presents as an
    /// estimate that drifts with nothing saying why.
    /// A direction with **no** travel time at all is not this check's business
    /// and is passed over: "there is no time to divide up" is a different
    /// complaint from "the compensations ate it", it has its own rule and its
    /// own message wherever a config enters, and reporting it here would replace
    /// a sentence naming the empty field with one naming a dead band the
    /// operator never set.
    pub fn checked_bands(&self) -> Result<(), DomainError> {
        let lag = self.start_lag_ms as u32;
        let up_eaten = self.up_time_ms > 0 && lag + self.vent_band_ms as u32 >= self.up_time_ms;
        let down_eaten =
            self.down_time_ms > 0 && lag + self.close_band_ms as u32 >= self.down_time_ms;
        if up_eaten || down_eaten {
            return Err(DomainError::DeadBandTooLong);
        }
        Ok(())
    }

    /// The timing model this shade's estimator runs on.
    pub fn travel(&self) -> TravelProfile {
        TravelProfile {
            up_time_ms: self.up_time_ms,
            down_time_ms: self.down_time_ms,
            start_lag_ms: self.start_lag_ms,
            vent_band_ms: self.vent_band_ms,
            close_band_ms: self.close_band_ms,
        }
    }

    /// The timing model for the tilt axis: one time, both directions, and no
    /// compensation.
    ///
    /// The lag and the dead bands are properties of the **lift** — a start lag
    /// measured by watching a curtain, a band measured by watching slats
    /// separate at the bottom of the lift's travel. Nothing has measured either
    /// for a tilt axis, and this crate drives no tilt command, so applying the
    /// lift's figures here would move a placeholder estimate on borrowed
    /// evidence.
    pub fn tilt_travel(&self) -> TravelProfile {
        TravelProfile {
            up_time_ms: self.tilt_time_ms,
            down_time_ms: self.tilt_time_ms,
            start_lag_ms: 0,
            vent_band_ms: 0,
            close_band_ms: 0,
        }
    }
}

/// Everything [`Motion`](crate::Motion) needs to turn elapsed milliseconds into
/// a position.
///
/// # Why the times are not simply divided into
///
/// A full traverse takes `up_time_ms`, and always did — that is what a stopwatch
/// measures. What this type adds is that not all of it moves the curtain. Two
/// intervals inside that total produce no position change:
///
/// ```text
/// Up, from the closed limit:
///   |<- start_lag ->|<- vent_band ->|<---------- up_span ---------->|
///   command                       slats                          fully
///   planned                       apart                           open
///   |<------------------------ up_time_ms ------------------------>|
///
/// Down, from the open limit:
///   |<- start_lag ->|<--------- down_span --------->|<- close_band ->|
///   command                                       curtain          motor
///   planned                                      at the sill        stops
///   |<---------------------- down_time_ms -------------------------->|
/// ```
///
/// So the lifting phase is shorter than the total, and the position moves faster
/// through it than a flat division would say. That is the whole correction, and
/// it is why a 25% command used to under-travel: it spent its budget on the two
/// phases that do not lift.
///
/// With every compensation at zero — which is what a shade is created with and
/// what a migrated one carries — `up_span_ms == up_time_ms`, the lag subtracts
/// nothing, and the model is *exactly* the linear one it refines. Nothing moves
/// until something is measured.
///
/// # Where this differs from the firmware it was ported from
///
/// Entirely. The controller this estimator was derived from integrates from the
/// instant a command is planned, at one flat rate per direction, with no lag and
/// no piecewise phase — its position is `elapsed / travel_time` and nothing
/// else. Both refinements here are deliberate divergences; see
/// `docs/provenance.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TravelProfile {
    /// Full traverse toward open, command to stop.
    pub up_time_ms: u32,
    /// Full traverse toward closed, command to stop.
    pub down_time_ms: u32,
    /// See [`ShadeConfig::start_lag_ms`].
    pub start_lag_ms: u16,
    /// See [`ShadeConfig::vent_band_ms`].
    pub vent_band_ms: u16,
    /// See [`ShadeConfig::close_band_ms`].
    pub close_band_ms: u16,
}

impl TravelProfile {
    /// Two travel times and no compensation — the flat model an un-calibrated
    /// shade runs on, and the one this estimator was ported from.
    ///
    /// Every refinement in this type is additive to it, so this is both the
    /// starting state of every shade and the control case any test of those
    /// refinements is measured against.
    pub const fn linear(up_time_ms: u32, down_time_ms: u32) -> TravelProfile {
        TravelProfile {
            up_time_ms,
            down_time_ms,
            start_lag_ms: 0,
            vent_band_ms: 0,
            close_band_ms: 0,
        }
    }

    /// Milliseconds of the up traverse that actually raise the curtain.
    ///
    /// Zero if the compensations consume the whole traverse, which
    /// [`ShadeConfig::checked_bands`] refuses at every boundary a value enters
    /// through — this saturates rather than underflowing so that a record from
    /// somewhere that check did not run degrades to the estimator's existing
    /// zero-travel behaviour instead of wrapping.
    pub const fn up_span_ms(self) -> u32 {
        self.up_time_ms
            .saturating_sub(self.start_lag_ms as u32)
            .saturating_sub(self.vent_band_ms as u32)
    }

    /// Milliseconds of the down traverse that actually lower the curtain.
    pub const fn down_span_ms(self) -> u32 {
        self.down_time_ms
            .saturating_sub(self.start_lag_ms as u32)
            .saturating_sub(self.close_band_ms as u32)
    }

    /// The lifting span in one direction.
    pub const fn span_ms(self, going_down: bool) -> u32 {
        if going_down {
            self.down_span_ms()
        } else {
            self.up_span_ms()
        }
    }
}
