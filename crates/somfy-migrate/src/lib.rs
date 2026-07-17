#![cfg_attr(not(test), no_std)]
//! # somfy-migrate
//!
//! Parser for the C++ ESPSomfy-RTS backup file format. It turns a backup
//! exported from a running C++ device (Settings → Backup, which is the on-flash
//! `shades.cfg` serialization) into [`MigrationData`] — the shades, rooms, and
//! groups a fresh somfy-rs install needs to adopt an existing setup without
//! re-pairing every motor.
//!
//! [`parse_backup`] is the entry point. The [`parse_header`] and per-record
//! [`parse_shade_record`]/[`parse_room_record`]/[`parse_group_record`] parsers,
//! and the [`Reader`] value tokenizer beneath them, are public for targeted
//! testing. The crate is `no_std` and allocation-free (heapless collections, no
//! floating-point — a workspace constraint; fractional positions parse straight
//! into integer hundredths via [`Reader::read_f32_as_centi`]).
//!
//! ## File format
//!
//! The C++ writer (`ShadeConfigFile::save`/`backup`, `src/ConfigFile.cpp:315-383`)
//! emits a header line, then records in a fixed order — **rooms, shades,
//! groups** — followed by repeater/settings/net/trans trailer records. Every
//! record is a line of comma-separated, space-padded fields terminated by `\n`.
//! [`Reader`] walks the buffer with the same field/terminator rules as the C++
//! read primitives but tolerates the fixed-width padding (`atoi`/`_rtrim`), so it
//! decodes both real device output and unpadded hand-authored fixtures.
//!
//! Only backup **versions 19..=25** are accepted — [`parse_header`] rejects
//! anything outside that window. Below 19 the record layouts differ; an unknown
//! future version could append fields and silently misalign every record parser,
//! so the ceiling is a deliberate choke point. The one per-version layout
//! difference inside the accepted range is the group record's rolling-code
//! position (see [`parse_group_record`]).
//!
//! ## Rolling-code migration contract
//!
//! The single most important transform: the C++ file stores each remote's
//! *last-sent* rolling code, but somfy-rs holds the *next-to-send* value. Every
//! recovered code is imported as [`somfy_rts::RollingCode`]`(last_sent + 1)`,
//! wrapping at 65535 — importing the stored value verbatim would replay the last
//! frame and desync the motor. This applies to both shades and groups: a C++
//! group *is* a virtual remote. See [`MigratedShade::next_code`].
//!
//! ## What this pass does NOT migrate
//!
//! - **Repeater, settings, net, and transceiver records** are skipped to EOF.
//!   Network credentials are intentionally not imported — the user re-enters them
//!   through the captive portal (design spec §3.4).
//! - **Linked-remote rolling codes and v19–v22 group rolling codes** are
//!   NVS-only in the C++ firmware and absent from the backup file, so they cannot
//!   be recovered from a file-only migration. Addresses are recovered;
//!   linked-remote codes are dropped and v19–v22 group codes are fabricated as
//!   [`somfy_rts::RollingCode`]`(1)` — see [`parse_group_record`], which calls
//!   this out loudly as a re-pair prompt for the consuming plan.
//! - **MQTT settings import is deferred to Plan 6** — a deliberate deviation from
//!   design spec §3.4, which lists MQTT among the migrated fields. Rationale: the
//!   settings record (`writeSettingsRecord`, `ConfigFile.cpp:1019`) parses fine,
//!   but there is nowhere to store the result until configuration persistence
//!   exists in Plan 6, which owns both MQTT config storage and this import. It is
//!   deferred, not dropped.
//!
//! ## Port fidelity
//!
//! Every primitive mirrors a specific C++ function and cites it in its doc
//! comment. Where this crate deliberately diverges from the C++ (EOF handling,
//! buffer-overflow handling, the `writeBool` format), the divergence is called
//! out at [`MigrateError`] and on the affected method — the C++ behavior is the
//! reference, but a migrator surfaces corruption rather than silently
//! substituting defaults the way the on-device reader does.

mod header;
mod migrate;
mod reader;
mod records;

pub use header::{parse_header, BackupHeader};
pub use migrate::{parse_backup, MigrationData};
pub use reader::{MigrateError, Reader};
pub use records::{
    parse_group_record, parse_room_record, parse_shade_record, MigratedGroup, MigratedRoom,
    MigratedShade,
};
