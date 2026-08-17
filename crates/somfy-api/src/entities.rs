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

/// Live snapshot of one shade for REST/WS payloads. Field names are
/// camelCase on the wire; positions are whole percent (0-100);
/// `kind`/`tiltMode` reuse the numeric discriminants deployed devices
/// already emit; `direction` uses the same sign convention deployed
/// devices use (-1 up, 0 idle, +1 down).
///
/// `addressOrigin` is **derived**, never stored and never accepted from a
/// client: a shade's address is allocated by the device, so its origin is a
/// fact about the address rather than a setting. See [`AddressOrigin`].
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
    pub down_time_ms: u32,
    pub tilt_time_ms: u32,
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
            down_time_ms: shade.config.down_time_ms,
            tilt_time_ms: shade.config.tilt_time_ms,
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
