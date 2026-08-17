//! REST command DTOs: the `{"action": "...", ...}` payloads the UI POSTs to
//! drive a shade, and their conversion into [`somfy_domain::ShadeCommand`].
//!
//! ## Why not `#[serde(tag = "action")]`
//!
//! serde's internally-tagged enum `Deserialize` is built on the `Content`
//! buffer, which is compiled only when serde has the `alloc` or `std` feature.
//! This crate pins `serde` to `default-features = false` + `derive` so the
//! firmware stays allocator-free (design spec: "no allocator; heapless only"),
//! and the firmware is the side that *deserializes* incoming REST commands.
//! So the enum's wire form is produced by a derive-based flat helper
//! ([`CommandWire`]) plus a thin manual [`Deserialize`], keeping the JSON
//! contract identical while never touching `Content`.

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::Deserialize as DeriveDeserialize;
use somfy_domain::{Pos, ShadeCommand};

/// REST command payload. On the wire it is a flat object tagged by `action`:
/// `{"action":"up"}`, `{"action":"goTo","position":42}`,
/// `{"action":"setMy","position":null}`, etc. Actions are camelCase
/// (`up`/`down`/`my`/`stepUp`/`stepDown`/`goTo`/`setMy`).
///
/// No tilt actions exist in this generation — tilt modes are config-carriage
/// only (see [`somfy_domain::ShadeConfig::tilt_mode`]), so the API MUST NOT
/// surface a tilt command until the domain ports tilt behavior.
///
/// [`ShadeCommand::Pair`] is absent **deliberately**, and this is the note that
/// says so rather than leaving it looking forgotten. Pairing transmits a frame
/// that teaches a motor a new remote, and it only works while somebody standing
/// at the shade has just put the motor into programming mode — so it is a step
/// in a procedure with a human in it, not an action a REST client should be able
/// to fire at an unattended shade. The controller exposes it as a Home Assistant
/// `button` per shade instead (`somfy-mqtt`), which is reached from the same
/// room the motor is in. `docs/hardware-checklist.md` carries the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
// The wire form is a flat, internally-tagged object (`{"action":"up"}`,
// `{"action":"goTo","position":42}`, ...) parsed by the manual `Deserialize`
// below via `CommandWire`. `CommandDto` carries no `#[serde]` container
// attribute (the tagging is hand-rolled), so ts-rs cannot infer the shape and
// MUST be told explicitly: tag on `action`, camelCase tag values. Unit variants
// emit `{ action: "up" }`; struct variants merge their fields alongside the tag,
// yielding `position: number` (required for `goTo`) and `position: number | null`
// (optional for `setMy`) — exactly the JSON the manual deserializer accepts.
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        tag = "action",
        rename_all = "camelCase"
    )
)]
pub enum CommandDto {
    Up,
    Down,
    My,
    StepUp,
    StepDown,
    GoTo { position: u8 },
    SetMy { position: Option<u8> },
}

impl CommandDto {
    /// Lower the wire command into the domain command. Percent positions are
    /// converted through [`Pos::from_percent`], which clamps values over 100 to
    /// [`Pos::FULL`].
    pub fn to_domain(&self) -> ShadeCommand {
        match *self {
            CommandDto::Up => ShadeCommand::Up,
            CommandDto::Down => ShadeCommand::Down,
            CommandDto::My => ShadeCommand::My,
            CommandDto::StepUp => ShadeCommand::StepUp,
            CommandDto::StepDown => ShadeCommand::StepDown,
            CommandDto::GoTo { position } => ShadeCommand::GoTo(Pos::from_percent(position)),
            CommandDto::SetMy { position } => ShadeCommand::SetMy(position.map(Pos::from_percent)),
        }
    }
}

/// Wire discriminant for [`CommandDto`]. A unit-only enum deserializes from the
/// bare action string with no `Content` buffer, so it stays allocator-free.
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
enum ActionTag {
    Up,
    Down,
    My,
    StepUp,
    StepDown,
    GoTo,
    SetMy,
}

/// Flat wire form: the tag plus the one optional numeric field every payload
/// shares. `position` is `Option<u8>`, so serde treats it as optional (missing
/// or explicit `null` both deserialize to `None`).
#[derive(DeriveDeserialize)]
#[serde(rename_all = "camelCase")]
struct CommandWire {
    action: ActionTag,
    position: Option<u8>,
}

impl<'de> Deserialize<'de> for CommandDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CommandWire::deserialize(deserializer)?;
        Ok(match wire.action {
            ActionTag::Up => CommandDto::Up,
            ActionTag::Down => CommandDto::Down,
            ActionTag::My => CommandDto::My,
            ActionTag::StepUp => CommandDto::StepUp,
            ActionTag::StepDown => CommandDto::StepDown,
            // `goTo` MUST carry a target — a missing one is a malformed request,
            // not a silent default (design rule: never swallow bad input).
            ActionTag::GoTo => CommandDto::GoTo {
                position: wire
                    .position
                    .ok_or_else(|| D::Error::missing_field("position"))?,
            },
            // `setMy` with no position means "clear the favorite".
            ActionTag::SetMy => CommandDto::SetMy {
                position: wire.position,
            },
        })
    }
}
