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
use somfy_domain::{RemoteIdentity, Shade, ShadeId};

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

/// The reference firmware's compiled-in travel-time defaults, which are also
/// [`somfy_domain::ShadeConfig::new`]'s.
///
/// Restated here rather than imported because `ShadeConfig::new` returns them
/// inside a value instead of exposing them as constants. `tests/shades.rs`
/// pins each one against what that constructor actually produces, so the
/// restatement cannot drift silently — which matters, because a wrong default
/// here would misclassify a *measured* value as uncalibrated.
pub const FACTORY_UP_TIME_MS: u32 = 10_000;
/// See [`FACTORY_UP_TIME_MS`].
pub const FACTORY_DOWN_TIME_MS: u32 = 10_000;
/// See [`FACTORY_UP_TIME_MS`].
pub const FACTORY_TILT_TIME_MS: u32 = 7_000;

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
    /// The device measured it by sweeping the shade.
    ///
    /// **Not produced yet**: the guided calibration of R2 does not exist. The
    /// variant is here so that building it later adds behaviour rather than
    /// changing this contract, and so the UI's exhaustive map already has a
    /// branch waiting for it.
    Measured,
}

impl CalibrationSource {
    /// Classify one travel time against the factory default for that field.
    ///
    /// # What this can and cannot tell apart today
    ///
    /// It separates [`FactoryDefault`](CalibrationSource::FactoryDefault) from
    /// everything else, and nothing more, because a shade's stored
    /// configuration currently has nowhere to record provenance — only the
    /// number survives. So a value that differs from the default is reported as
    /// [`OperatorSupplied`](CalibrationSource::OperatorSupplied), which is true
    /// in the sense that matters: some human put that number there, whether
    /// here or on the device this setup was migrated from.
    ///
    /// **The upgrade path is one line and no contract change.** When the
    /// persisted shade record grows a provenance field (Plan 6's record-format
    /// task), [`ShadeDto::from_shade`] reads it instead of calling this, and
    /// [`Measured`](CalibrationSource::Measured) starts appearing. Nothing on
    /// the wire moves.
    ///
    /// # The false positive, and why it is the right one to accept
    ///
    /// An operator who measures a shade and gets exactly 10.0 s is told it is
    /// uncalibrated. That is wrong, and it is deliberate: R7 rules that "a
    /// value that is merely *plausible* is not evidence anybody chose it". The
    /// two errors are not symmetric — being invited to re-measure something
    /// already correct costs a minute, while presenting a factory default as
    /// configured is the failure that produced a 25% command moving a shade
    /// 1%, and it cost an afternoon to diagnose.
    pub fn of(value_ms: u32, factory_default_ms: u32) -> CalibrationSource {
        if value_ms == factory_default_ms {
            CalibrationSource::FactoryDefault
        } else {
            CalibrationSource::OperatorSupplied
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
pub const SHADE_JSON_MAX_BYTES: usize = 640;

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
            kind: shade.config.kind as u8,
            tilt_mode: shade.config.tilt_mode as u8,
            position: shade.pos().percent(),
            target: shade.target().percent(),
            tilt_position: shade.tilt_pos().percent(),
            my_position: shade.my_pos().map(|p| p.percent()),
            direction: shade.direction().sign(),
            up_time_ms: shade.config.up_time_ms,
            up_time_source: CalibrationSource::of(shade.config.up_time_ms, FACTORY_UP_TIME_MS),
            down_time_ms: shade.config.down_time_ms,
            down_time_source: CalibrationSource::of(
                shade.config.down_time_ms,
                FACTORY_DOWN_TIME_MS,
            ),
            tilt_time_ms: shade.config.tilt_time_ms,
            tilt_time_source: CalibrationSource::of(
                shade.config.tilt_time_ms,
                FACTORY_TILT_TIME_MS,
            ),
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
