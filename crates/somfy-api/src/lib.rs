//! # somfy-api
//!
//! Typed REST/WebSocket payloads for somfy-rs. The serde DTOs mirror the live
//! [`somfy_domain`] entities (shades, groups, rooms) on the wire and are the
//! single contract shared by the firmware and the Preact UI.
//!
//! ## Wire conventions
//!
//! - **Field names are camelCase** (`tiltMode`, `myPosition`, `upTimeMs`).
//! - **Positions are whole percent as `u8`** (0–100) — never floats.
//! - **`kind`/`tiltMode`/`direction` reuse the numeric discriminants deployed
//!   devices already emit** (`direction` keeps the same sign convention:
//!   -1 up, 0 idle, +1 down), so payloads stay compact and consistent with
//!   deployed firmware and with migrated backups.
//! - **No tilt *commands* exist this generation.** Tilt is config-carriage only
//!   (see [`somfy_domain::ShadeConfig`]); [`CommandDto`] carries no tilt action.
//! - **[`CommandDto`] and [`CreateShadeDto`] are deserialize-only** — they are
//!   inbound REST payloads the firmware receives, never ones it emits.
//! - **Rejections are typed** ([`ApiErrorCode`]), not English sentences: the UI
//!   ships two languages and the device ships none.
//!
//! ## Shade lifecycle
//!
//! Three routes beyond the command surface, and the shapes of their answers are
//! the contract:
//!
//! | Route | Body | Success |
//! |---|---|---|
//! | `POST /api/v1/shades` | [`CreateShadeDto`] | `201` + [`ShadeDto`] |
//! | `PATCH /api/v1/shades/{id}` | [`PatchShadeDto`] | `200` + [`ShadeDto`] |
//! | `DELETE /api/v1/shades/{id}` | — | `204` |
//! | `POST /api/v1/shades/{id}/pair` | — | `202` |
//!
//! `PATCH` exists because travel times were otherwise settable only at
//! creation, and correcting one meant deleting the shade — which loses its
//! address and costs a fresh pairing at the window. See [`PatchShadeDto`].
//!
//! **Pairing answers `202 Accepted` and can never answer `200 OK`.** RTS is
//! one-way: the device queues a `Prog` burst and never learns whether the motor
//! took it. `202` is the honest code for "this has been accepted for
//! processing" with no claim about the outcome, and the outcome genuinely lives
//! outside the system — it is a person watching the shade jog.
//!
//! It is also **not** a [`CommandDto`] action, and that is deliberate rather
//! than an omission; [`CommandDto`]'s own documentation carries the argument.
//!
//! ## Manual tagged (de)serialization
//!
//! [`CommandDto`] and [`WsEvent`] are tag-dispatched enums (`action`/`ev`) but
//! deliberately avoid `#[serde(tag = "…")]`: serde's internally-tagged codec is
//! built on the `Content` buffer, which compiles only with serde's `alloc`/`std`
//! feature. This crate pins serde to `default-features = false` + `derive` so the
//! firmware stays allocator-free, so each enum's wire form is produced by a
//! derive-based flat helper plus a thin manual impl. The JSON shape is identical
//! to an internally-tagged enum.
//!
//! ## Features
//!
//! The crate is `no_std` by default so it links into the firmware. The
//! `std`/`ts` features enable host-side `ts-rs` TypeScript generation into
//! `ui/src/api/generated/`, making UI/firmware drift a compile error.

#![cfg_attr(not(any(test, feature = "std", feature = "ts")), no_std)]

mod commands;
mod entities;
mod errors;
mod events;
mod shades;

pub use commands::CommandDto;
pub use entities::{
    AddressOrigin, CalibrationSource, GroupDto, RoomDto, ShadeDto, FACTORY_DOWN_TIME_MS,
    FACTORY_TILT_TIME_MS, FACTORY_UP_TIME_MS, SHADE_JSON_MAX_BYTES,
};
pub use errors::{ApiErrorCode, ApiErrorDto};
pub use events::{ShadeStateEvent, WsEvent};
pub use shades::{CreateShadeDto, PatchShadeDto, NAME_MAX_BYTES};
