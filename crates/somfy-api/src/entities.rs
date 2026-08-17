//! Serde DTOs mirroring the live [`somfy_domain`] entities on the wire.
//!
//! Wire contract (kept stable for the UI and for backup/migration parity with
//! deployed devices): field names are camelCase, positions are whole percent
//! (0-100), `kind`/`tiltMode` reuse the numeric discriminants deployed
//! devices already emit, and `direction` uses the same sign convention
//! deployed devices use (-1 up, 0 idle, +1 down).

// NB: heapless `String`/`Vec` are referenced fully qualified rather than
// imported. The `ts` feature derives `ts_rs::TS`, whose generated code uses the
// std prelude `String` (e.g. `fn ident() -> String`); a `use heapless::String`
// here would shadow it and break the derive.
use serde::{Deserialize, Serialize};
use somfy_domain::{Pos, RemoteIdentity, Shade, ShadeId};

/// Where a shade's remote address came from, and therefore whether pairing it
/// can accomplish anything.
///
/// # Why this exists and `paired: bool` does not
///
/// RTS is one-way. The controller transmits `Prog` and never learns whether the
/// motor accepted it; the only acknowledgement that exists anywhere in the
/// protocol is the motor jogging, seen by a person standing at it. A `paired`
/// flag would therefore be a *belief* rendered as a *fact*, and the UI would go
/// on presenting it long after somebody reset the motor.
///
/// This is a different kind of claim: it is read straight off the address, so
/// it is true by construction.
///
/// # What it gates
///
/// Pairing teaches a motor one remote address — this controller's. An address
/// that came from *another* controller is already known to the motor and is
/// already being transmitted at by that other controller, so pairing it teaches
/// the motor nothing and leaves the two-controllers-one-identity failure
/// [`somfy_domain::RemoteIdentity`] documents fully in place. So pairing is
/// offered for [`AddressOrigin::Allocated`] and refused for
/// [`AddressOrigin::Imported`] — see [`crate::ApiErrorCode::AddressNotAllocated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum AddressOrigin {
    /// This controller invented the address, so no other controller transmits
    /// at it — and no motor knows it until somebody pairs one.
    Allocated,
    /// The address arrived with a provisioned table or a migrated backup and
    /// belongs to whichever controller allocated it.
    Imported,
}

impl AddressOrigin {
    /// Classify a 24-bit remote address.
    ///
    /// The test is bit 23, which is exactly
    /// [`RemoteIdentity::SPACE_START`]: that crate sets the bit on every
    /// address it allocates precisely so the separation is structural rather
    /// than probabilistic, and it is `pub` there because "a guarantee a caller
    /// cannot check is a guarantee it has to take on trust". This is the caller
    /// checking it.
    ///
    /// Note what this does *not* claim. A foreign controller is free to emit an
    /// address with bit 23 set — nothing in RTS reserves it — so this reads
    /// "allocated under this project's scheme", not "provably ours". It is
    /// still the right gate, because the failure it prevents (pairing a motor
    /// to an address a second controller is counting on) is only reachable
    /// through addresses that arrived from that second controller, and those
    /// are the ones this classifies as [`Imported`](AddressOrigin::Imported).
    pub fn of(address: u32) -> AddressOrigin {
        if address & RemoteIdentity::SPACE_START != 0 {
            AddressOrigin::Allocated
        } else {
            AddressOrigin::Imported
        }
    }
}

/// Whether an operator has reported that this shade actually works.
///
/// # Read the name twice — it is the whole design
///
/// This is **not** `paired`, and the difference is not pedantry. RTS is one-way:
/// the device transmits a `Prog` burst and never hears anything back, so no
/// controller anywhere can know whether a motor accepted it. A `paired: bool`
/// would be a user's belief stored as a device fact, and it would keep saying
/// `true` long after somebody reset the motor — which is why this workspace has
/// never had one and why [`AddressOrigin`] was the previous answer to the
/// question.
///
/// What *is* knowable is what a person told us. The variant names say so:
/// [`ConfirmedByOperator`](PairingState::ConfirmedByOperator) attributes the
/// claim to the human who made it, so a reader of this field cannot mistake it
/// for something the device measured. `AddressOrigin` is still the other half
/// and still derived — it says whether pairing *could* accomplish anything;
/// this says whether anybody has seen that it did.
///
/// # What it gates, on the wire
///
/// **Announcement to Home Assistant.** A shade in
/// [`AwaitingConfirmation`](PairingState::AwaitingConfirmation) exists, appears
/// in `GET /api/v1/shades`, and accepts commands on this API — which is how the
/// setup flow gets the operator to test it — and has **no MQTT entities at
/// all**. That is the point: an entity that appears in Home Assistant, accepts
/// commands and drives nothing is the failure this endpoint set was rebuilt to
/// prevent.
///
/// # How it moves
///
/// `POST /api/v1/shades/{id}/confirm-pairing`, and nothing else. It is not a
/// [`crate::PatchShadeDto`] field, because a PATCH field would be settable both
/// ways and "set this back to unconfirmed" would retire the entities of a
/// working shade from a body a client sent by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum PairingState {
    /// Nobody has reported this shade working. It has entities nowhere and is
    /// presented as an unfinished setup.
    AwaitingConfirmation,
    /// An operator reported that it responded to a command.
    ConfirmedByOperator,
}

impl PairingState {
    /// Carry the domain's state onto the wire.
    ///
    /// A separate type rather than a `serde` derive on
    /// [`somfy_domain::PairingState`], for the reason every DTO here is
    /// separate: `somfy-domain` depends on neither `serde` nor `ts-rs`, and the
    /// wire form is this crate's contract to keep stable rather than the
    /// domain's. The mapping is exhaustive, so a third state added in the domain
    /// stops this compiling.
    pub fn of(state: somfy_domain::PairingState) -> PairingState {
        match state {
            somfy_domain::PairingState::AwaitingConfirmation => PairingState::AwaitingConfirmation,
            somfy_domain::PairingState::ConfirmedByOperator => PairingState::ConfirmedByOperator,
        }
    }
}

/// The travel-time defaults a shade is created with, which are also the ones
/// deployed devices ship with.
///
/// **Re-exported rather than restated.** They used to be copied here, pinned
/// against the constructor by a test, because the domain returned them inside a
/// value instead of naming them. It names them now — the record decoder needs
/// them too, to reconstruct provenance for a table written before provenance was
/// stored — so there is one definition and nothing left to drift.
pub use somfy_domain::{FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS, FACTORY_UP_TIME_MS};

/// Where a travel time came from — and therefore how much the position
/// estimate computed from it is worth.
///
/// # Why three states and not `calibrated: bool`
///
/// The same objection as `paired: bool` (see [`AddressOrigin`]), for a
/// different reason: a boolean here does not overstate confidence, it *loses*
/// the distinction the operator needs. "Nobody chose this", "somebody measured
/// it with a stopwatch" and "the device swept it" call for three different
/// actions, and collapsing the last two hides the comparison that makes an
/// automatic sweep trustworthy — a sweep reporting 10 s where a stopwatch said
/// 30 s must be *visibly* disagreeing with something.
/// (`docs/specs/2026-08-15-position-accuracy-requirements.md` R9.)
///
/// # Why this is worth a field at all
///
/// On 2026-08-17 a command for 25% open moved a shade about 1%. All three
/// shades carried 10000/10000/7000 — the reference firmware's compiled-in
/// defaults, imported faithfully because nobody had ever calibrated them, and
/// presented by the UI as though they were configured. Hand measurement gave
/// 30 s up and 27 s down. R7 was raised from SHOULD to MUST on the strength of
/// that: a factory default MUST be surfaced as **uncalibrated**, not shown as a
/// setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum CalibrationSource {
    /// Equal to the reference firmware's compiled-in default. **Nobody chose
    /// this**, and the UI must not present it as though somebody had.
    FactoryDefault,
    /// A human supplied it — typed in, or carried over from a device where
    /// somebody had typed it in.
    OperatorSupplied,
    /// The device measured it, through the guided calibration.
    Measured,
}

impl CalibrationSource {
    /// Carry the domain's provenance onto the wire.
    ///
    /// A separate type rather than a `serde` derive on the domain's, for the
    /// reason every DTO here is separate — and exhaustive, so a fourth state
    /// added in the domain stops this compiling.
    ///
    /// # This used to be a guess, and the guess was the bug
    ///
    /// It took the *value* and the factory default and compared them, because
    /// the persisted shade record had nowhere to keep provenance. That made
    /// [`Measured`](CalibrationSource::Measured) unreachable — nothing could
    /// produce it — and it misreported both directions: an operator who
    /// measured exactly 10.0 s was told the shade was uncalibrated, and a
    /// factory default that somebody had genuinely chosen was indistinguishable
    /// from one nobody had touched.
    ///
    /// The record carries it now, so this is a mapping rather than an inference.
    /// The one thing that has not changed is what happens to a *migrated* value:
    /// a table written before the field existed still has its provenance
    /// reconstructed by that same comparison, once, in the record decoder — see
    /// `somfy_config`'s calibration block. R7's ruling stands there and is what
    /// makes it the right reconstruction: "a value that is merely *plausible* is
    /// not evidence anybody chose it".
    pub fn of(source: somfy_domain::CalibrationSource) -> CalibrationSource {
        match source {
            somfy_domain::CalibrationSource::FactoryDefault => CalibrationSource::FactoryDefault,
            somfy_domain::CalibrationSource::OperatorSupplied => {
                CalibrationSource::OperatorSupplied
            }
            somfy_domain::CalibrationSource::Measured => CalibrationSource::Measured,
        }
    }
}

/// The widest any one entity here serialises to as JSON, in bytes.
///
/// # Why this is a constant in this crate
///
/// A device serialises one of these into a **fixed buffer** — there is no
/// allocator on that path — and a buffer one byte short is not an error it can
/// usefully report: the encoder returns `Err` in the middle of writing a
/// response whose status has already been sent. So the bound belongs beside the
/// types it describes, where adding a field moves it, rather than in a server
/// that would have to guess.
///
/// # Measured, not counted
///
/// `tests/wire_width.rs` constructs the widest legal value of each type and
/// checks it against this figure from both sides: over it, and more than 128
/// bytes under it. A hand-counted version of this number was wrong by 160
/// bytes, and the buffer it sized would have let a single shade break the list
/// endpoint permanently.
///
/// # What the worst case actually is
///
/// A name of thirty-two control characters. The field is a
/// `heapless::String<32>` and JSON escapes a control character as `\u00XX`, six
/// bytes for one, so the name alone reaches 192 — and nothing refuses such a
/// name, so it is reachable rather than hypothetical. An ordinary shade is
/// under half this.
pub const SHADE_JSON_MAX_BYTES: usize = 672;

/// Live snapshot of one shade for REST/WS payloads. Field names are
/// camelCase on the wire; positions are whole percent (0-100);
/// `kind`/`tiltMode` reuse the numeric discriminants deployed devices
/// already emit; `direction` uses the same sign convention deployed
/// devices use (-1 up, 0 idle, +1 down).
///
/// Two fields are **derived**, never stored and never accepted from a client,
/// and each sits next to the value it describes:
///
/// - `addressOrigin` — a shade's address is allocated by the device, so its
///   origin is a fact about the address rather than a setting.
///   See [`AddressOrigin`].
/// - `upTimeSource` / `downTimeSource` / `tiltTimeSource` — whether anybody
///   ever measured that travel time, which decides how much the position
///   estimate computed from it is worth. See [`CalibrationSource`].
///
/// One field is **stored and settable through exactly one route**:
/// `pairingState`, which decides whether the shade has Home Assistant entities
/// at all. See [`PairingState`], including why it is not a `paired` boolean and
/// not a [`crate::PatchShadeDto`] field.
///
/// There is deliberately **no dead-band field** for the non-linear first
/// seconds of Up travel off the closed limit. See the note on
/// [`crate::PatchShadeDto`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct ShadeDto {
    pub id: u8,
    // `heapless::String<N>` does not implement `TS`; on the wire it is a plain
    // JSON string, so override the emitted type.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub name: heapless::String<32>,
    pub address: u32,
    pub address_origin: AddressOrigin,
    pub pairing_state: PairingState,
    pub kind: u8,
    pub tilt_mode: u8,
    pub position: u8,
    pub target: u8,
    pub tilt_position: u8,
    pub my_position: Option<u8>,
    pub direction: i8,
    pub up_time_ms: u32,
    pub up_time_source: CalibrationSource,
    pub down_time_ms: u32,
    pub down_time_source: CalibrationSource,
    pub tilt_time_ms: u32,
    pub tilt_time_source: CalibrationSource,
    /// Milliseconds between a command being sent and the motor moving. See
    /// [`somfy_domain::ShadeConfig::start_lag_ms`].
    pub start_lag_ms: u32,
    /// Milliseconds an Up command spends separating the slats at the closed
    /// limit, and therefore where `vent` stops. Zero means it has never been
    /// measured, and `vent` is refused until it has. See
    /// [`somfy_domain::ShadeConfig::vent_band_ms`].
    pub vent_band_ms: u32,
    /// Milliseconds a Down command spends compressing the slats after the
    /// curtain has reached the sill. See
    /// [`somfy_domain::ShadeConfig::close_band_ms`].
    pub close_band_ms: u32,
    /// How far `position` may be from the truth, in whole percent.
    ///
    /// `0` means the estimate was last set by a physical limit — the one thing a
    /// one-way protocol can be sure of — and `100` means it says nothing at all.
    /// It is what turns a confidently wrong "60%" into an honest "≈60%", and on
    /// a shade still carrying factory travel times it saturates after the first
    /// partial move, which is the correct report rather than a defect.
    ///
    /// **Derived, never stored and never accepted from a client**, like
    /// `addressOrigin`: it is a fact about how the estimate was arrived at.
    ///
    /// # There is deliberately no `calibrating` beside it
    ///
    /// A guided calibration run is a conversation with the operator who started
    /// it, and the screen driving it already knows it is running. What a *second*
    /// viewer would gain from the field is a spinner; what it would cost is a
    /// per-shade slot for something at most one shade uses at a time, on a device
    /// where the whole shade table is copied about five times onto the boot stack
    /// (`crates/firmware/src/heap.rs`). A stale tab is answered instead by
    /// [`crate::ApiErrorCode::NotCalibrating`], which is the honest reply and
    /// costs nothing.
    pub position_uncertainty: u8,
}

impl ShadeDto {
    /// Snapshot a shade's live state into a wire DTO. `id` is the registry slot
    /// index; positions read the dead-reckoned estimate at its current value
    /// (call after [`Shade::tick`] to reflect the latest position).
    pub fn from_shade(id: ShadeId, shade: &Shade) -> ShadeDto {
        ShadeDto {
            id: id.0,
            name: shade.config.name.clone(),
            address: shade.config.address,
            address_origin: AddressOrigin::of(shade.config.address),
            pairing_state: PairingState::of(shade.config.pairing_state),
            kind: shade.config.kind as u8,
            tilt_mode: shade.config.tilt_mode as u8,
            position: shade.pos().percent(),
            target: shade.target().percent(),
            tilt_position: shade.tilt_pos().percent(),
            my_position: shade.my_pos().map(|p| p.percent()),
            direction: shade.direction().sign(),
            up_time_ms: shade.config.up_time_ms,
            up_time_source: CalibrationSource::of(shade.config.up_time_source),
            down_time_ms: shade.config.down_time_ms,
            down_time_source: CalibrationSource::of(shade.config.down_time_source),
            tilt_time_ms: shade.config.tilt_time_ms,
            tilt_time_source: CalibrationSource::of(shade.config.tilt_time_source),
            start_lag_ms: shade.config.start_lag_ms as u32,
            vent_band_ms: shade.config.vent_band_ms as u32,
            close_band_ms: shade.config.close_band_ms as u32,
            // Raw hundredths of a percent down to whole percent, the same scale
            // `position` is on — so a client can render "60% ± 3%" without
            // knowing anything about the domain's fixed point.
            position_uncertainty: Pos::from_raw(shade.confidence()).percent(),
        }
    }
}

/// A named group of shade ids for REST/WS payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id: u8,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub name: heapless::String<32>,
    #[cfg_attr(feature = "ts", ts(type = "number[]"))]
    pub shade_ids: heapless::Vec<u8, 32>,
}

/// A named room of shade ids for REST/WS payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct RoomDto {
    pub id: u8,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub name: heapless::String<32>,
    #[cfg_attr(feature = "ts", ts(type = "number[]"))]
    pub shade_ids: heapless::Vec<u8, 32>,
}
