//! Serde DTOs mirroring the live [`somfy_domain`] entities on the wire.
//!
//! Wire contract (kept stable for the UI and for backup/migration parity with
//! the C++ firmware): field names are camelCase, positions are whole percent
//! (0-100), `kind`/`tiltMode` are the C++ numeric discriminants, and
//! `direction` uses the C++ sign convention (-1 up, 0 idle, +1 down).

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};
use somfy_domain::{Shade, ShadeId};

/// Live snapshot of one shade for REST/WS payloads. Field names are
/// camelCase on the wire; positions are whole percent (0-100);
/// `kind`/`tiltMode` are the C++ numeric discriminants; `direction`
/// uses the C++ sign convention (-1 up, 0 idle, +1 down).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadeDto {
    pub id: u8,
    pub name: String<32>,
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
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id: u8,
    pub name: String<32>,
    pub shade_ids: Vec<u8, 32>,
}

/// A named room of shade ids for REST/WS payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomDto {
    pub id: u8,
    pub name: String<32>,
    pub shade_ids: Vec<u8, 32>,
}
