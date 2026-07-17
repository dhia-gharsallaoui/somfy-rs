//! Shade-record parser — the migration-critical rolling-code carrier.
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
    /// Room assignment — C++ `roomId` `uint8` (`:879`), reinterpreted as `i8`
    /// (values `0..=15` are unchanged; a `255` sentinel would surface as `-1`).
    pub room_id: i8,
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
/// |27 | `roomId` (:879, v>=19)      | i8     | → `room_id` (`\n`-terminated) |
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
    // this read realigns the cursor to the start of the next record.
    let mut room_id = 0i8;
    if v >= 19 {
        room_id = r.read_i8()?;
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
