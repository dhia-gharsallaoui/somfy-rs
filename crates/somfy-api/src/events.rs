//! WebSocket event envelope: the `{"ev": "...", ...}` messages the firmware
//! pushes to the UI. Only [`WsEvent::ShadeState`] exists in this generation;
//! more variants land in Plan 5.
//!
//! ## Why not `#[serde(tag = "ev")]`
//!
//! Internally-tagged enum (de)serialization is built on serde's `Content`
//! buffer, compiled only under serde's `alloc`/`std` feature. This crate keeps
//! serde allocator-free (design spec: "no allocator; heapless only"), so the
//! flat tagged wire form is produced by a derive-based helper ([`WsEventWire`])
//! plus thin manual [`Serialize`]/[`Deserialize`] impls. The JSON shape is
//! identical to an internally-tagged enum and grows cleanly as Plan 5 adds
//! variants.

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};
use serde::{Deserialize as DeriveDeserialize, Serialize as DeriveSerialize};
use somfy_domain::StateDelta;

/// One WebSocket message. On the wire it is a flat object tagged by `ev`:
/// `{"ev":"shadeState","id":...,"position":...,"tiltPosition":...,"direction":...}`.
/// Keeping the payload flat lets the UI consume it without unwrapping a nested
/// object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
// The wire form is a flat, internally-tagged object (`{"ev":"shadeState", ...}`)
// produced by the manual `Serialize`/`Deserialize` below. `WsEvent` carries no
// `#[serde]` container attribute (the tagging is hand-rolled), so ts-rs cannot
// infer the shape and MUST be told explicitly: tag on `ev`, camelCase tag
// values. The newtype variant `ShadeState(ShadeStateEvent)` inlines its inner
// struct's fields alongside the tag, matching `WsEventWire`.
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        tag = "ev",
        rename_all = "camelCase"
    )
)]
pub enum WsEvent {
    ShadeState(ShadeStateEvent),
}

/// Live shade state pushed on the WebSocket. Positions are whole percent
/// (0-100); `direction` uses the same sign convention deployed devices use
/// (-1 up, 0 idle, +1 down).
#[derive(Debug, Clone, PartialEq, Eq, DeriveSerialize, DeriveDeserialize)]
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
pub struct ShadeStateEvent {
    pub id: u8,
    pub position: u8,
    pub tilt_position: u8,
    pub direction: i8,
}

impl From<&StateDelta> for ShadeStateEvent {
    fn from(d: &StateDelta) -> Self {
        ShadeStateEvent {
            id: d.id.0,
            position: d.pos.percent(),
            tilt_position: d.tilt_pos.percent(),
            direction: d.direction.sign(),
        }
    }
}

/// Wire discriminant for [`WsEvent`]. A unit-only enum (de)serializes as the
/// bare tag string with no `Content` buffer, so it stays allocator-free.
#[derive(DeriveSerialize, DeriveDeserialize)]
enum EvTag {
    #[serde(rename = "shadeState")]
    ShadeState,
}

/// Flat wire form of a `shadeState` message: the tag followed by the payload
/// fields. Kept in lockstep with [`ShadeStateEvent`]; the manual [`WsEvent`]
/// impls are the single conversion point.
#[derive(DeriveSerialize, DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
struct WsEventWire {
    ev: EvTag,
    id: u8,
    position: u8,
    tilt_position: u8,
    direction: i8,
}

impl Serialize for WsEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match self {
            WsEvent::ShadeState(e) => WsEventWire {
                ev: EvTag::ShadeState,
                id: e.id,
                position: e.position,
                tilt_position: e.tilt_position,
                direction: e.direction,
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WsEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WsEventWire::deserialize(deserializer)?;
        Ok(match wire.ev {
            EvTag::ShadeState => WsEvent::ShadeState(ShadeStateEvent {
                id: wire.id,
                position: wire.position,
                tilt_position: wire.tilt_position,
                direction: wire.direction,
            }),
        })
    }
}
