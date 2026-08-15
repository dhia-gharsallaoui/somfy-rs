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
use somfy_domain::{Shade, ShadeId};

/// Live snapshot of one shade for REST/WS payloads. Field names are
/// camelCase on the wire; positions are whole percent (0-100);
/// `kind`/`tiltMode` reuse the numeric discriminants deployed devices
/// already emit; `direction` uses the same sign convention deployed
/// devices use (-1 up, 0 idle, +1 down).
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
