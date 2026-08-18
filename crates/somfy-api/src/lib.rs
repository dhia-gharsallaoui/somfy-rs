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
//! Five routes beyond the command surface, and the shapes of their answers are
//! the contract:
//!
//! | Route | Body | Success |
//! |---|---|---|
//! | `POST /api/v1/shades` | [`CreateShadeDto`] | `201` + [`ShadeDto`] |
//! | `PATCH /api/v1/shades/{id}` | [`PatchShadeDto`] | `200` + [`ShadeDto`] |
//! | `DELETE /api/v1/shades/{id}` | — | `204` |
//! | `POST /api/v1/shades/{id}/pair` | — | `202` |
//! | `POST /api/v1/shades/{id}/confirm-pairing` | — | `200` + [`ShadeDto`] |
//!
//! ### Adding a shade is one flow, and it cannot be one request
//!
//! Three constraints force the shape, and none of them is negotiable. The
//! address a motor will be taught has to **exist before** the `Prog` frame that
//! teaches it, so a record must be created first. A **person has to act in the
//! middle**: only a remote the motor already obeys can put it into programming
//! mode, and this controller is by definition not one of those. And the device
//! **can never confirm success**, because RTS is one-way.
//!
//! So it is three requests — create, pair, confirm — and the thing that makes it
//! one *flow* rather than three optional steps is that the intermediate state is
//! not presented as finished: a created shade has [`PairingState`]
//! `awaitingConfirmation` and **no Home Assistant entities at all** until the
//! last request lands. Abandoning halfway is a `DELETE`, and it leaves nothing
//! behind because there was never anything on the broker to clear.
//!
//! `PATCH` exists because travel times were otherwise settable only at
//! creation, and correcting one meant deleting the shade — which loses its
//! address and costs a fresh pairing at the window. See [`PatchShadeDto`].
//!
//! **Pairing answers `202 Accepted` and can never answer `200 OK`.** RTS is
//! one-way: the device queues a `Prog` burst and never learns whether the motor
//! took it. `202` is the honest code for "this has been accepted for
//! processing" with no claim about the outcome, and the outcome genuinely lives
//! outside the system — it is a person watching the shade move.
//!
//! It is also **not** a [`CommandDto`] action, and that is deliberate rather
//! than an omission; [`CommandDto`]'s own documentation carries the argument.
//!
//! **Confirmation answers `200 OK` with the shade**, because unlike pairing it
//! *is* a claim about something that happened: a person watched the shade obey a
//! command and said so, and the device has recorded that and announced the
//! entities. The client needs the new [`ShadeDto`] to stop presenting the shade
//! as unfinished. It is a route of its own rather than a [`PatchShadeDto`]
//! field for the reasons on [`PairingState`] — chiefly that a field would be
//! settable in the other direction.
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

mod calibration;
mod commands;
mod entities;
mod errors;
mod events;
pub mod origin;
mod settings;
mod shades;

pub use calibration::{CalibrationLegDto, CalibrationMarkDto, CalibrationStepDto};
pub use commands::CommandDto;
pub use entities::{
    AddressOrigin, CalibrationSource, GroupDto, PairingState, RoomDto, ShadeDto,
    FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS, FACTORY_UP_TIME_MS, SHADE_JSON_MAX_BYTES,
};
pub use errors::{ApiErrorCode, ApiErrorDto, SettingsFieldDto};
pub use events::{ShadeStateEvent, WsEvent};
pub use settings::{
    MqttSettingsDto, MqttUpdateDto, SecretDto, SettingsDto, TrialDecisionDto, TrialPhaseDto,
    WifiSettingsDto, WifiTrialDto, WifiUpdateDto, MAX_ADDRESS_LEN, MAX_SECRET_LEN,
    SETTINGS_JSON_MAX_BYTES,
};
pub use shades::{CreateShadeDto, PatchShadeDto, NAME_MAX_BYTES};
