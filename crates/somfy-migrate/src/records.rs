//! Record parsers for the C++ backup body: rooms, shades, and groups.
//!
//! Each parser ports the matching C++ `read*Record`/`write*Record` pair and
//! documents the discovered field map with citations. The shade parser is the
//! migration-critical rolling-code carrier; the group parser is a second one
//! (groups are their own virtual remotes) and carries the SAME `+1` contract.
//!
//! Ports C++ `ShadeConfigFile::readShadeRecord` (`src/ConfigFile.cpp:801-885`),
//! cross-checked field-for-field against `writeShadeRecord` (`:970-1018`). The
//! writer emits a **fixed-width** line: every value is space-padded by
//! `ConfigFile::writeString` (`:187-201`) — `%3u` u8, `%5u` u16, `%10u` u32,
//! `%4d` i8, `true`/`false` padded to 5 (`writeBool` :241-242), `%12.5f` floats
//! (`writeFloat` :236-239), and the 21-byte `name` padded to 20 — so a v25
//! record is exactly `SHADE_REC_SIZE` = 276 bytes (`:12`). [`Reader`] tolerates
//! the padding (`atoi` skips leading whitespace, `read_str` `_rtrim`s), so
//! fixtures need not reproduce it. The final field (`roomId`) is terminated by
//! the record end (`\n`, `writeUInt8(shade->roomId, CFG_REC_END)` :1016), which
//! [`Reader::read_i8`] consumes, leaving the cursor at the next record.

use crate::header::BackupHeader;
use crate::reader::{MigrateError, Reader};
use heapless::{String, Vec};
use somfy_rts::RollingCode;

/// Linked-remote slots per shade — C++ `SOMFY_MAX_LINKED_REMOTES` (`Somfy.h:8`).
const MAX_LINKED_REMOTES: usize = 7;

/// Member-shade slots per group — C++ `SOMFY_MAX_GROUPED_SHADES` (`Somfy.h:9`).
const MAX_GROUPED_SHADES: usize = 32;

/// Backup version whose group record carries `lastRollingCode` *before* the
/// linked shades (`readGroupRecord` :747). v24+ moved it to the record end.
const GROUP_ROLLING_MID_VERSION: u8 = 23;

/// First backup version whose group record carries `lastRollingCode` at the
/// record end, after `roomId` (`readGroupRecord` :763; `writeGroupRecord` :955).
const GROUP_ROLLING_TAIL_VERSION: u8 = 24;

/// One shade decoded from a C++ backup, carrying only the fields somfy-rs models.
///
/// Fields the C++ record also serializes but somfy-rs does not model (`paired`,
/// `stepSize`, `myTiltPos`, `flipCommands`, `flipPosition`, `repeats`,
/// `sortOrder`, `gpioUp`/`gpioDown`/`gpioMy`/`gpioFlags`) are still parsed
/// positionally so the cursor stays aligned, then dropped.
///
/// This is a faithful **deserializer**: values are the raw wire contents. The
/// C++ `readShadeRecord` applies post-load domain normalization this parser does
/// **not** — clamping `myPos`/`myTiltPos` outside `[0,100]` to the `-1` sentinel
/// (`:840-841`), zeroing tilt state when `tiltType == none` or the shade is not a
/// blind (`:844-848`), and forcing `tiltonly` shades to fully closed
/// (`:869-871`). Those belong to the domain layer that consumes this struct, so
/// `kind_raw`/`tilt_mode_raw`/`*_centi` are returned exactly as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedShade {
    /// Shade identifier — C++ `shadeId` (`readShadeRecord` :804). `255` marks a
    /// cleared/deleted slot in the C++ file; the caller decides whether to keep it.
    pub shade_id: u8,
    /// Display name — C++ `name` `char[21]` (`:808`), fixed-width, `_rtrim`med.
    pub name: String<32>,
    /// Motor remote address — C++ `remoteAddress` `uint32` (`:807`).
    pub address: u32,
    /// Next rolling code to transmit. **Migration contract:** the C++ file stores
    /// the *last-sent* code `lastRollingCode` (`:825`); somfy-rs holds the
    /// *next-to-send* value, so `next_code = RollingCode(last_sent + 1)` with wrap
    /// at 65535 (rolling.rs "persist before TX" / off-by-one note). Importing the
    /// stored value verbatim would replay the last frame and desync the motor.
    pub next_code: RollingCode,
    /// Raw `shadeType` discriminant — C++ `shade_types` (`:806`, `Somfy.h:56-74`);
    /// maps to [`somfy_domain::ShadeKind`].
    pub kind_raw: u8,
    /// Raw `tiltType` discriminant — C++ `tilt_types` (`:812`, `Somfy.h:75-81`);
    /// maps to [`somfy_domain::TiltMode`].
    pub tilt_mode_raw: u8,
    /// Full-travel up time in ms — C++ `upTime` `uint32` (`:815`).
    pub up_time_ms: u32,
    /// Full-travel down time in ms — C++ `downTime` `uint32` (`:816`).
    pub down_time_ms: u32,
    /// Full tilt time in ms — C++ `tiltTime` `uint32` (`:817`).
    pub tilt_time_ms: u32,
    /// Current position in hundredths of a percent — C++ `currentPos` `%12.5f`
    /// (`:842`), e.g. `55.25000` → `5525`.
    pub position_centi: i32,
    /// Current tilt position in centi-percent — C++ `currentTiltPos` (`:843`).
    pub tilt_position_centi: i32,
    /// "My"/favorite position in centi-percent — C++ `myPos` (`:837`). The C++
    /// unset sentinel `-1.0` arrives as `-100`.
    pub my_position_centi: i32,
    /// Room assignment — C++ `roomId` `uint8` (`readUInt8`, `:879`); `255` marks
    /// an unassigned shade in the C++ file.
    pub room_id: u8,
    /// Non-zero linked-remote addresses in slot order. The C++ file writes
    /// `SOMFY_MAX_LINKED_REMOTES` (7) address slots (`writeShadeRecord` :988-991);
    /// a `0` slot is empty. **Linked-remote rolling codes are NOT in the backup
    /// file** — the C++ reads them from NVS (`pref.getUShort`, `:822`), which a
    /// file-only migrator cannot recover, so only addresses are carried.
    pub linked_addresses: Vec<u32, MAX_LINKED_REMOTES>,
    /// Raw shade flags byte — C++ `flags` `uint8` (`:826`).
    pub flags_raw: u8,
    /// Radio symbol bit length — C++ `bitLength` `uint8` (`:814`), typically 56.
    pub bit_length: u8,
    /// Raw radio protocol discriminant — C++ `radio_proto` (`:813`, `Somfy.h:24`).
    pub proto_raw: u8,
}

/// Parse one shade record at the cursor, advancing to the next record.
///
/// Field order and version gates mirror C++ `readShadeRecord`
/// (`src/ConfigFile.cpp:801-885`). This crate only accepts backups `>= 19` (the
/// [`crate::parse_header`] floor), and every version in the accepted `19..=25`
/// range serializes the **identical** shade layout — the highest gate in
/// `readShadeRecord` is `roomId` at `version >= 19` (`:879`). The additive gates
/// below are therefore always taken for accepted headers; they are kept as an
/// exact port of the reference reader (and use `header.version`) so the wire
/// format is documented in code and the parser stays correct if the floor moves.
/// The legacy *alternate-format* branches for `version < 19` — the pre-v3
/// boolean `tiltType` (`:809-810`), the pre-v4 `uint8` `myPos` with no
/// `myTiltPos` (`:834-835`), and the pre-v5 5-remote cap (`:823`) — are
/// unreachable behind the header floor and are intentionally omitted.
///
/// ## Field map (wire order; `→` = modeled, `skip` = parsed then dropped)
///
/// | # | C++ field (`readShadeRecord`) | reader | destination |
/// |---|-------------------------------|--------|-------------|
/// | 1 | `shadeId` (:804)              | u8     | → `shade_id` |
/// | 2 | `paired` (:805)              | bool   | skip |
/// | 3 | `shadeType` (:806)          | u8     | → `kind_raw` |
/// | 4 | `remoteAddress` (:807)      | u32    | → `address` |
/// | 5 | `name` `char[21]` (:808)    | str    | → `name` |
/// | 6 | `tiltType` (:812)           | u8     | → `tilt_mode_raw` |
/// | 7 | `proto` (:813, v>6)         | u8     | → `proto_raw` |
/// | 8 | `bitLength` (:814, v>1)     | u8     | → `bit_length` |
/// | 9 | `upTime` (:815)             | u32    | → `up_time_ms` |
/// |10 | `downTime` (:816)           | u32    | → `down_time_ms` |
/// |11 | `tiltTime` (:817)           | u32    | → `tilt_time_ms` |
/// |12 | `stepSize` (:818, v>5)      | u16    | skip |
/// |13 | `linkedRemotes[0..7]` (:819-824) | 7×u32 | → `linked_addresses` (non-zero) |
/// |14 | `lastRollingCode` (:825)    | u16    | → `next_code` (`+1`, wrapping) |
/// |15 | `flags` (:826, v>7)         | u8     | → `flags_raw` |
/// |16 | `myPos` `%12.5f` (:837)     | centi  | → `my_position_centi` |
/// |17 | `myTiltPos` (:838)          | centi  | skip |
/// |18 | `currentPos` (:842)         | centi  | → `position_centi` |
/// |19 | `currentTiltPos` (:843)     | centi  | → `tilt_position_centi` |
/// |20 | `flipCommands` (:855, v>=9) | bool   | skip |
/// |21 | `flipPosition` (:856, v>=10)| bool   | skip |
/// |22 | `repeats` (:857, v>=12)     | u8     | skip |
/// |23 | `sortOrder` (:858, v>=13)   | u8     | skip |
/// |24 | `gpioUp`,`gpioDown` (:860-863, v>14) | 2×u8 | skip |
/// |25 | `gpioMy` (:864-865, v>15)   | u8     | skip |
/// |26 | `gpioFlags` (:866-867, v>16)| u8     | skip |
/// |27 | `roomId` (:879, v>=19)      | u8     | → `room_id` (`\n`-terminated) |
///
/// # Errors
///
/// - [`MigrateError::UnexpectedEof`] if the record is truncated.
/// - [`MigrateError::StringTooLong`] if `name` exceeds 32 bytes (the C++ 20-char
///   cap never does).
/// - [`MigrateError::BadRecord`] on invalid UTF-8 in `name`.
pub fn parse_shade_record(
    r: &mut Reader,
    header: &BackupHeader,
) -> Result<MigratedShade, MigrateError> {
    let v = header.version;

    let shade_id = r.read_u8()?; // 1 shadeId
    let _paired = r.read_bool()?; // 2 paired — not modeled
    let kind_raw = r.read_u8()?; // 3 shadeType
    let address = r.read_u32()?; // 4 remoteAddress
    let name = read_name(r)?; // 5 name (char[21], _rtrimmed)
    let tilt_mode_raw = r.read_u8()?; // 6 tiltType (v>=3 form; v19 floor)

    // 7 proto (v>6): C++ default readUInt8(0).
    let mut proto_raw = 0u8;
    if v > 6 {
        proto_raw = r.read_u8()?;
    }
    // 8 bitLength (v>1): C++ default readUInt8(56).
    let mut bit_length = 56u8;
    if v > 1 {
        bit_length = r.read_u8()?;
    }

    let up_time_ms = r.read_u32()?; // 9 upTime
    let down_time_ms = r.read_u32()?; // 10 downTime
    let tilt_time_ms = r.read_u32()?; // 11 tiltTime

    // 12 stepSize (v>5) — not modeled.
    if v > 5 {
        r.read_u16()?;
    }

    // 13 linkedRemotes: 7 address-only slots; 0 = empty. Rolling codes live in
    // NVS, not the file, so they cannot be migrated here.
    let mut linked_addresses: Vec<u32, MAX_LINKED_REMOTES> = Vec::new();
    for _ in 0..MAX_LINKED_REMOTES {
        let addr = r.read_u32()?;
        if addr != 0 {
            linked_addresses
                .push(addr)
                .map_err(|_| MigrateError::BadRecord("linked_remotes"))?;
        }
    }

    // 14 lastRollingCode → next_code. THE migration contract: stored value is the
    // last code SENT; the next transmit must be +1 (wrapping) or the motor desyncs.
    let last_rolling_code = r.read_u16()?;
    let next_code = RollingCode(last_rolling_code.wrapping_add(1));

    // 15 flags (v>7): C++ default readUInt8(0).
    let mut flags_raw = 0u8;
    if v > 7 {
        flags_raw = r.read_u8()?;
    }

    // 16-19 float block (v>=4 two-float myPos form; v19 floor). Positions arrive
    // as %12.5f, truncated to centi-percent by read_f32_as_centi.
    let my_position_centi = r.read_f32_as_centi()?; // 16 myPos (-1.0 → -100)
    let _my_tilt_pos_centi = r.read_f32_as_centi()?; // 17 myTiltPos — not modeled
    let position_centi = r.read_f32_as_centi()?; // 18 currentPos
    let tilt_position_centi = r.read_f32_as_centi()?; // 19 currentTiltPos

    if v >= 9 {
        r.read_bool()?; // 20 flipCommands — not modeled
    }
    if v >= 10 {
        r.read_bool()?; // 21 flipPosition — not modeled
    }
    if v >= 12 {
        r.read_u8()?; // 22 repeats — not modeled
    }
    if v >= 13 {
        r.read_u8()?; // 23 sortOrder — not modeled
    }
    if v > 14 {
        r.read_u8()?; // 24a gpioUp — not modeled
        r.read_u8()?; // 24b gpioDown — not modeled
    }
    if v > 15 {
        r.read_u8()?; // 25 gpioMy — not modeled
    }
    if v > 16 {
        r.read_u8()?; // 26 gpioFlags — not modeled
    }

    // 27 roomId (v>=19): the final field, terminated by the record end (\n), so
    // this read realigns the cursor to the start of the next record. C++ reads it
    // with readUInt8 (:879).
    let mut room_id = 0u8;
    if v >= 19 {
        room_id = r.read_u8()?;
    }

    Ok(MigratedShade {
        shade_id,
        name,
        address,
        next_code,
        kind_raw,
        tilt_mode_raw,
        up_time_ms,
        down_time_ms,
        tilt_time_ms,
        position_centi,
        tilt_position_centi,
        my_position_centi,
        room_id,
        linked_addresses,
        flags_raw,
        bit_length,
        proto_raw,
    })
}

/// One room decoded from a C++ backup.
///
/// C++ `SomfyRoom` (`Somfy.h:204-217`) also carries `sortOrder`, which is parsed
/// positionally to keep the cursor aligned but not modeled here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedRoom {
    /// Room identifier — C++ `roomId` `uint8` (`readRoomRecord` :791). A `0`
    /// marks a cleared slot in the C++ file (`save` skips it, `ConfigFile.cpp:332`);
    /// the caller decides whether to keep it.
    pub room_id: u8,
    /// Display name — C++ `name` `char[21]` (`:792`, `Somfy.h:207`), `_rtrim`med.
    pub name: String<32>,
}

/// Parse one room record at the cursor, advancing to the next record.
///
/// Field order mirrors C++ `readRoomRecord` (`src/ConfigFile.cpp:789-798`),
/// cross-checked against `writeRoomRecord` (`:964-968`). `readRoomRecord` has
/// **no version gates** — the layout is identical across every accepted version
/// — so `header` is accepted only for pipeline uniformity. The record is
/// `ROOM_REC_SIZE` = 29 bytes fixed-width (`ConfigFile.cpp:15`).
///
/// ## Field map (wire order; `→` = modeled, `skip` = parsed then dropped)
///
/// | # | C++ field (`readRoomRecord`) | reader | destination |
/// |---|------------------------------|--------|-------------|
/// | 1 | `roomId` (:791)              | u8     | → `room_id` |
/// | 2 | `name` `char[21]` (:792)    | str    | → `name` |
/// | 3 | `sortOrder` (:793)          | u8     | skip (`\n`-terminated) |
///
/// # Errors
///
/// - [`MigrateError::UnexpectedEof`] if the record is truncated.
/// - [`MigrateError::StringTooLong`] / [`MigrateError::BadRecord`] on a bad `name`.
pub fn parse_room_record(
    r: &mut Reader,
    _header: &BackupHeader,
) -> Result<MigratedRoom, MigrateError> {
    let room_id = r.read_u8()?; // 1 roomId
    let name = read_name(r)?; // 2 name (char[21], _rtrimmed)
    let _sort_order = r.read_u8()?; // 3 sortOrder — not modeled (\n-terminated)
    Ok(MigratedRoom { room_id, name })
}

/// One group decoded from a C++ backup.
///
/// A C++ `SomfyGroup` (`Somfy.h:380-419`) *is a `SomfyRemote`*: it has its own
/// remote address and rolling code and transmits like a shade, so the same
/// migration `+1` contract applies to `next_code`. Fields the record also
/// serializes but somfy-rs does not model here (`groupType`, `proto`,
/// `bitLength`, `repeats`, `sortOrder`, `flipCommands`, `roomId`) are parsed
/// positionally, then dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedGroup {
    /// Group identifier — C++ `groupId` `uint8` (`readGroupRecord` :741). `255`
    /// marks a cleared slot (`save` skips it, `ConfigFile.cpp:342`); the caller
    /// decides whether to keep it.
    pub group_id: u8,
    /// Display name — C++ `name` `char[21]` (`:744`, `Somfy.h:388`), `_rtrim`med.
    pub name: String<32>,
    /// Virtual-remote address — C++ `remoteAddress` `uint32` (`:743`).
    pub address: u32,
    /// Next rolling code to transmit. **Same migration contract as a shade:** the
    /// C++ file stores the *last-sent* code (`lastRollingCode`); somfy-rs holds
    /// the *next-to-send* value, so `next_code = RollingCode(last_sent + 1)` with
    /// wrap at 65535. See [`MigratedShade::next_code`] and the version note on
    /// [`parse_group_record`] for where the stored code lives per version.
    ///
    /// **⚠️ v19–v22 backups fabricate this value.** Those versions do not store a
    /// group rolling code (it lives in NVS), so `next_code` is set to
    /// `RollingCode(1)`. This type **cannot distinguish a fabricated code from a
    /// real one.** A paired receiver WILL reject `RollingCode(1)` as replayed, so
    /// Plan 6 must surface v19–v22 groups to the user (re-pair or set the code
    /// manually) — see [`parse_group_record`].
    pub next_code: RollingCode,
    /// Non-zero member shade ids in slot order. The C++ file writes
    /// `SOMFY_MAX_GROUPED_SHADES` (32) slots (`writeGroupRecord` :948-950);
    /// `readGroupRecord` drops `0` slots and preserves order (`:750-754`).
    pub member_shade_ids: Vec<u8, MAX_GROUPED_SHADES>,
}

/// Parse one group record at the cursor, advancing to the next record.
///
/// Field order and version gates mirror C++ `readGroupRecord`
/// (`src/ConfigFile.cpp:738-776`), cross-checked against the v25 writer
/// `writeGroupRecord` (`:941-957`). Unlike the shade record, the group record's
/// **rolling-code position moves with the version** — the one true per-version
/// layout difference in the accepted `19..=25` range:
///
/// - **v19–v22:** the file carries *no* group rolling code; the C++ sources it
///   from NVS only (`:764-767`). A file-only migrator cannot recover it, so
///   `next_code` is **fabricated** as `RollingCode(1)` (stored `0` → `+1`).
/// - **v23:** `lastRollingCode` sits *before* the linked shades (`:747`).
/// - **v24–v25:** `lastRollingCode` is the final, `\n`-terminated field, after
///   `roomId` (`:763`; writer `:955`).
///
/// Even where the file supplies the code (v23+), the C++ then takes
/// `max(nvs, file)` (`:766`); a file-only migrator uses the file value as the
/// best recoverable source (documented on [`MigratedGroup::next_code`]).
///
/// ## Field map (wire order for v24/v25; `→` = modeled, `skip` = dropped)
///
/// | # | C++ field (`readGroupRecord`) | reader | destination |
/// |---|-------------------------------|--------|-------------|
/// | 1 | `groupId` (:741)             | u8     | → `group_id` |
/// | 2 | `groupType` (:742)          | u8     | skip |
/// | 3 | `remoteAddress` (:743)      | u32    | → `address` |
/// | 4 | `name` `char[21]` (:744)    | str    | → `name` |
/// | 5 | `proto` (:745)              | u8     | skip |
/// | 6 | `bitLength` (:746)          | u8     | skip |
/// | – | `lastRollingCode` (:747)    | u16    | → `next_code` (**v23 only**, `+1`) |
/// | 7 | `linkedShades[0..32]` (:750-754) | 32×u8 | → `member_shade_ids` (non-zero) |
/// | 8 | `repeats` (:755, v>=12)     | u8     | skip |
/// | 9 | `sortOrder` (:756, v>=13)   | u8     | skip |
/// |10 | `flipCommands` (:761, v>=18)| bool   | skip |
/// |11 | `roomId` (:762, v>=19)      | u8     | skip |
/// |12 | `lastRollingCode` (:763)    | u16    | → `next_code` (**v>=24**, `+1`, `\n`-terminated) |
///
/// # ⚠️ Fabricated rolling codes (v19–v22)
///
/// A **v19–v22 backup cannot recover a group's rolling code** — the C++ keeps it
/// in NVS, not the file (`:764-767`). This parser fabricates `next_code =
/// RollingCode(1)`, and **[`MigratedGroup`] cannot distinguish a fabricated code
/// from a real one.** A paired Somfy receiver tracks its own rolling code and
/// **WILL reject `RollingCode(1)` as a replayed/stale frame**, so a group
/// migrated from a v19–v22 backup will not actuate until it is re-paired or its
/// code is set to the receiver's expected value. **Plan 6 must surface this to
/// the user** (prompt to re-pair the group or enter the code manually) rather
/// than silently importing a dead group. v23+ backups carry the real code.
///
/// # Errors
///
/// - [`MigrateError::UnexpectedEof`] if the record is truncated.
/// - [`MigrateError::StringTooLong`] / [`MigrateError::BadRecord`] on a bad `name`.
/// - [`MigrateError::BadRecord`] if more than 32 member shades are present.
pub fn parse_group_record(
    r: &mut Reader,
    header: &BackupHeader,
) -> Result<MigratedGroup, MigrateError> {
    let v = header.version;

    let group_id = r.read_u8()?; // 1 groupId
    let _group_type = r.read_u8()?; // 2 groupType — not modeled
    let address = r.read_u32()?; // 3 remoteAddress
    let name = read_name(r)?; // 4 name (char[21], _rtrimmed)
    let _proto = r.read_u8()?; // 5 proto — not modeled
    let _bit_length = r.read_u8()?; // 6 bitLength — not modeled

    // The file only carries the group rolling code from v23 up; below that it is
    // NVS-only and unrecoverable here, so 0 (→ next_code 1) is the honest default.
    let mut last_rolling_code = 0u16;

    // v23 places lastRollingCode here, before the linked shades (:747).
    if v == GROUP_ROLLING_MID_VERSION {
        last_rolling_code = r.read_u16()?;
    }

    // 7 linkedShades: 32 slots; 0 = empty. readGroupRecord compacts to eliminate
    // gaps (:750-754), so only non-zero ids are kept, in slot order.
    let mut member_shade_ids: Vec<u8, MAX_GROUPED_SHADES> = Vec::new();
    for _ in 0..MAX_GROUPED_SHADES {
        let shade_id = r.read_u8()?;
        if shade_id != 0 {
            member_shade_ids
                .push(shade_id)
                .map_err(|_| MigrateError::BadRecord("linked_shades"))?;
        }
    }

    // 8-11 additive gates (all taken for the accepted v19+ range), parsed then
    // dropped so the cursor reaches the record end / trailing rolling code.
    if v >= 12 {
        r.read_u8()?; // 8 repeats — not modeled
    }
    if v >= 13 {
        r.read_u8()?; // 9 sortOrder — not modeled
    }
    if v >= 18 {
        r.read_bool()?; // 10 flipCommands — not modeled
    }
    if v >= 19 {
        r.read_u8()?; // 11 roomId — not modeled
    }

    // 12 v24+ places lastRollingCode last, \n-terminated (:763; writer :955).
    if v >= GROUP_ROLLING_TAIL_VERSION {
        last_rolling_code = r.read_u16()?;
    }

    // THE migration contract, shared with shades: stored value is the last code
    // SENT; the next transmit must be +1 (wrapping) or the motor desyncs.
    let next_code = RollingCode(last_rolling_code.wrapping_add(1));

    Ok(MigratedGroup {
        group_id,
        name,
        address,
        next_code,
        member_shade_ids,
    })
}

/// Read the fixed-width `name` field into the 32-byte model capacity.
///
/// [`Reader::read_str`] fills its `String<64>` buffer and applies the C++
/// `_rtrim`; the trimmed value is copied into `String<32>` (the C++ source is
/// `char[21]`, max 20 chars, so it always fits). An over-long value errors
/// rather than silently truncating, per this crate's divergence policy.
fn read_name(r: &mut Reader) -> Result<String<32>, MigrateError> {
    let mut wide: String<64> = String::new();
    r.read_str(&mut wide)?;
    let mut name: String<32> = String::new();
    name.push_str(wide.as_str())
        .map_err(|_| MigrateError::StringTooLong)?;
    Ok(name)
}
