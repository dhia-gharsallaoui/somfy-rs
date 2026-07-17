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
//! - **`kind`/`tiltMode`/`direction` are the C++ numeric discriminants**
//!   (`direction` keeps the C++ sign convention: -1 up, 0 idle, +1 down), so
//!   payloads stay compact and consistent with the original firmware and with
//!   migrated backups.
//! - **No tilt *commands* exist this generation.** Tilt is config-carriage only
//!   (see [`somfy_domain::ShadeConfig`]); [`CommandDto`] carries no tilt action.
//! - **[`CommandDto`] is deserialize-only** — it is the inbound REST command the
//!   firmware receives, never one it emits.
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

#![cfg_attr(not(any(test, feature = "ts")), no_std)]

mod commands;
mod entities;
mod events;

pub use commands::CommandDto;
pub use entities::{GroupDto, RoomDto, ShadeDto};
pub use events::{ShadeStateEvent, WsEvent};
