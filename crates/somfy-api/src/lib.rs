//! # somfy-api
//!
//! Typed REST/WebSocket payloads for somfy-rs. Serde DTOs mirror the live
//! [`somfy_domain`] entities (shades, groups, rooms) on the wire: field names are
//! camelCase, positions are whole percent, and `kind`/`tiltMode`/`direction`
//! serialize as their C++ numeric discriminants so payloads stay compact and
//! migration-consistent with the original firmware.
//!
//! The crate is `no_std` by default (default features) so it links into the
//! firmware; the `std`/`ts` features enable host-side TypeScript generation for
//! the UI.

#![cfg_attr(not(any(test, feature = "ts")), no_std)]

mod entities;

pub use entities::{GroupDto, RoomDto, ShadeDto};
