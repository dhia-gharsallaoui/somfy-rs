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
/// says so rather than leaving it looking forgotten. Pairing is not a way to
/// move a shade; it is one step inside *adding* one, and it only does anything
/// while somebody standing at the motor has just put it into programming mode
/// with a remote this controller does not have. Everything else in this enum is
/// a movement that can be watched and undone from anywhere in the house.
///
/// So it lives on its own route — `POST /api/v1/shades/{id}/pair`, answering
/// `202 Accepted` — which differs from a command in three ways that matter:
///
/// - it is refused outright on a shade whose address came from another
///   controller ([`ApiErrorCode::AddressNotAllocated`]), where a `Prog` burst
///   would teach the motor nothing;
/// - it cannot be aimed at a group, which the domain also refuses
///   ([`somfy_domain::DomainError::NotAGroupCommand`]) — fanned across a group
///   it becomes a `Prog` at every shade in the house with nobody at any of them;
/// - it never reports success, because there is none to report.
///
/// `docs/hardware-checklist.md` carries the full sequence and its hazards.
///
/// [`ApiErrorCode::AddressNotAllocated`]: crate::ApiErrorCode::AddressNotAllocated
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
    GoTo {
        position: u8,
    },
    SetMy {
        position: Option<u8>,
    },
    /// Close fully, then open just far enough to separate the slats.
    ///
    /// A command rather than a position, because it is not one: it is reached
    /// **from the closed limit** by timing, and the whole reason to have it is
    /// that it therefore uses no position estimate at all. See
    /// [`somfy_domain::ShadeCommand::Vent`].
    ///
    /// It carries no `position` field for that reason. What it aims at is the
    /// shade's own measured `ventBandMs`, and the device refuses the command
    /// with [`ApiErrorCode::VentBandNotMeasured`] while that is zero rather than
    /// guessing one.
    ///
    /// Unlike [`somfy_domain::ShadeCommand::Pair`], which is absent from this
    /// enum, a vent **is** a movement anybody can watch and undo — so it is an
    /// ordinary command here and it may be aimed at a group.
    ///
    /// [`ApiErrorCode::VentBandNotMeasured`]: crate::ApiErrorCode::VentBandNotMeasured
    Vent,
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
            CommandDto::Vent => ShadeCommand::Vent,
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
    Vent,
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
            // No position: the vent point is the shade's measured
            // slat-separation band, not something a caller may name.
            ActionTag::Vent => CommandDto::Vent,
        })
    }
}
