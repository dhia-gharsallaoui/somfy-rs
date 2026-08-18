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
//! [`parse_shade_record`]/[`parse_room_record`]/[`parse_group_record`]/[`parse_net_record`] parsers,
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
//! ## Recovered data shape
//!
//! - **Cleared-slot sentinels are filtered and empty slots compacted.** The C++
//!   writer never emits cleared rooms (`roomId == 0`), shades (`shadeId == 255`),
//!   or groups (`groupId == 255`); if one appears in a hand-edited or corrupt
//!   backup it is dropped rather than surfaced as a live entity, and the `0`
//!   linked-remote / member-shade slots the file pads with are compacted out. See
//!   [`parse_backup`] (sentinel filtering) and [`MigratedShade::linked_addresses`]
//!   / [`MigratedGroup::member_shade_ids`] (slot compaction).
//! - **Domain normalization is deferred to the consumer.** The record parsers are
//!   faithful *deserializers*: `kind_raw`/`tilt_mode_raw`/`*_centi` carry the raw
//!   wire values, and the C++ post-load clamps (myPos/tilt normalization, tilt-only
//!   fully-closed) are left to the domain layer that consumes [`MigrationData`].
//!   That is why the `*_raw` fields exist — see [`MigratedShade`].
//!
//! ## What this pass does NOT migrate
//!
//! - **Repeater, settings, and transceiver records** are stepped over. The
//!   **net** record between them *is* read, but only for the broker settings in
//!   it — see below. Network credentials are intentionally not imported: the
//!   user re-enters them through the captive portal (design spec §3.4), and the
//!   static-IP block a net record also carries would be a claim about a network
//!   this device has not joined.
//! - **Linked-remote rolling codes and v19–v22 group rolling codes** are
//!   NVS-only in the C++ firmware and absent from the backup file, so they cannot
//!   be recovered from a file-only migration. Addresses are recovered;
//!   linked-remote codes are dropped and v19–v22 group codes are fabricated as
//!   [`somfy_rts::RollingCode`]`(1)` — see [`parse_group_record`], which calls
//!   this out loudly as a re-pair prompt for the consuming plan.
//! - **The broker username, password, and whether MQTT was even enabled.** The
//!   backup does not carry them: `writeNetRecord` emits the protocol, host,
//!   port, discovery flag and the two topic namespaces, while `MQTTSettings`
//!   also holds `enabled`, `username` and `password` in NVS. So an import can
//!   recover *where* to publish and never *as whom*.
//!
//! ## MQTT settings, and the one transform that is not a copy
//!
//! [`MigratedMqtt`] carries `rootTopic` and `discoTopic` exactly as the file
//! holds them. The C++ then **concatenates** them at publish time —
//! `MQTTClass::makeTopic` prepends `rootTopic` to every topic, including the
//! discovery topic built from `discoTopic` — which is the single fault that
//! makes Home Assistant discovery unusable on that firmware.
//!
//! This crate does not undo it, and deliberately: a deserializer that silently
//! reinterpreted its input would make the file and the struct disagree. Undoing
//! it belongs to the consumer, which maps `discoTopic` onto `discovery_prefix`
//! and `rootTopic` onto `state_root` as two independent namespaces, and refuses
//! the result if the pair breaks a rule. See
//! `docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md` R1 and R3.
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

pub use header::{parse_header, BackupHeader, MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION};
pub use migrate::{parse_backup, MigrationData};
pub use reader::{MigrateError, Reader};
pub use records::{
    parse_group_record, parse_net_record, parse_room_record, parse_shade_record, MigratedGroup,
    MigratedMqtt, MigratedRoom, MigratedShade,
};
