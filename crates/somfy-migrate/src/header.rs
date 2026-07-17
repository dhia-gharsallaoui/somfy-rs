//! Versioned backup-header parser.
//!
//! Ports the fixed field layout written by C++ `ConfigFile::writeHeader`
//! (`src/ConfigFile.cpp:45-62`) and cross-checked against the version gates in
//! `readHeader` (`:63-93`). `writeHeader` in the current firmware always emits
//! every field; the version gates in `readHeader` describe how *older* backups
//! were laid out, so a migrator that must read historical files reproduces those
//! gates rather than the writer.
//!
//! ## Accepted layout
//!
//! This migrator only understands the modern layout, so it rejects backups below
//! version 19 with [`MigrateError::UnsupportedVersion`] (the `>= 19` room-field
//! gate at `ConfigFile.cpp:69` marks where the current record shape begins).
//! Within the accepted range the only wire-format difference is the repeater
//! pair, which `readHeader` gates on `version >= 21` (`:81-84`): v19/v20 backups
//! omit `repeaterRecordSize`/`repeaterRecords` entirely, so those fields default
//! to `0` here. The final field, `serverId`, is terminated by the record end
//! (`\n`) rather than a separator (`writeString(..., CFG_REC_END)`, `:60`);
//! [`Reader::read_str`] stops on either, so it consumes it correctly.

use crate::reader::{MigrateError, Reader};
use heapless::String;

/// Lowest backup version this migrator can parse.
///
/// Below this the record layout differs (older `readHeader` branches at
/// `ConfigFile.cpp:73-79` read narrower fields); such backups are rejected
/// rather than mis-parsed.
pub const MIN_SUPPORTED_VERSION: u8 = 19;

/// Highest backup version this migrator has been verified against — the current
/// firmware writer version `SHADE_HDR_VER` (`ConfigFile.cpp:10`).
///
/// A future version could append fields to a record and silently misalign every
/// record parser below it (the record readers here reproduce the exact v19..=25
/// field layouts). Rejecting an unknown-future version at the single header
/// choke point guards the whole pipeline rather than mis-parsing it.
pub const MAX_SUPPORTED_VERSION: u8 = 25;

/// First version whose header carries the repeater record pair
/// (`ConfigFile.cpp:81-84`).
const REPEATER_MIN_VERSION: u8 = 21;

/// Capacity of the `serverId` field: C++ `char serverId[10]`
/// (`ConfigFile.h:28`).
const SERVER_ID_CAP: usize = 10;

/// Decoded backup header.
///
/// Field names and widths mirror C++ `config_header_t` (`ConfigFile.h:15-29`)
/// and are read in `writeHeader` order (`ConfigFile.cpp:47-60`). Record sizes
/// are the on-disk byte length of each record type; record counts are how many
/// of each follow the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupHeader {
    /// Backup format version (`>= 19`).
    pub version: u8,
    /// Header length in bytes as recorded by the writer.
    pub length: u8,
    /// Byte length of one room record.
    pub room_record_size: u16,
    /// Number of room records following the header.
    pub room_records: u8,
    /// Byte length of one shade record.
    pub shade_record_size: u16,
    /// Number of shade records.
    pub shade_records: u8,
    /// Byte length of one group record.
    pub group_record_size: u16,
    /// Number of group records.
    pub group_records: u8,
    /// Byte length of one repeater record (`0` for v19/v20 backups).
    pub repeater_record_size: u16,
    /// Number of repeater records (`0` for v19/v20 backups).
    pub repeater_records: u8,
    /// Byte length of the settings record.
    pub settings_record_size: u16,
    /// Byte length of the network record.
    pub net_record_size: u16,
    /// Byte length of the transceiver record.
    pub trans_record_size: u16,
    /// Server identifier (max 10 bytes, C++ `char serverId[10]`).
    pub server_id: String<SERVER_ID_CAP>,
}

/// Parse a backup header from the front of `r`.
///
/// Reads the fields in exact C++ `writeHeader` order (`ConfigFile.cpp:47-60`).
/// The version is read first: a value outside the supported `19..=25` range
/// (`MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION`) yields
/// [`MigrateError::UnsupportedVersion`] before any further field is consumed.
/// The repeater pair is only read for `version >= 21` (mirroring the
/// `readHeader` gate at `ConfigFile.cpp:81-84`); for older accepted versions
/// those fields are left at `0`. On success the cursor sits at the first byte of
/// the record that follows the header line.
///
/// # Errors
///
/// - [`MigrateError::UnsupportedVersion`] if `version < 19` or `version > 25`.
/// - [`MigrateError::UnexpectedEof`] if the header is truncated.
/// - [`MigrateError::StringTooLong`] if `serverId` exceeds 10 bytes.
/// - [`MigrateError::BadRecord`] if `serverId` is not valid UTF-8.
pub fn parse_header(r: &mut Reader) -> Result<BackupHeader, MigrateError> {
    let version = r.read_u8()?;
    if !(MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION).contains(&version) {
        return Err(MigrateError::UnsupportedVersion(version));
    }
    let length = r.read_u8()?;
    let room_record_size = r.read_u16()?;
    let room_records = r.read_u8()?;
    let shade_record_size = r.read_u16()?;
    let shade_records = r.read_u8()?;
    let group_record_size = r.read_u16()?;
    let group_records = r.read_u8()?;

    let (repeater_record_size, repeater_records) = if version >= REPEATER_MIN_VERSION {
        (r.read_u16()?, r.read_u8()?)
    } else {
        (0, 0)
    };

    let settings_record_size = r.read_u16()?;
    let net_record_size = r.read_u16()?;
    let trans_record_size = r.read_u16()?;
    let server_id = read_server_id(r)?;

    Ok(BackupHeader {
        version,
        length,
        room_record_size,
        room_records,
        shade_record_size,
        shade_records,
        group_record_size,
        group_records,
        repeater_record_size,
        repeater_records,
        settings_record_size,
        net_record_size,
        trans_record_size,
        server_id,
    })
}

/// Read the record-end-terminated `serverId` into a 10-byte string.
///
/// [`Reader::read_str`] fills a `String<64>` (its fixed buffer) and applies the
/// C++ `_rtrim`; the trimmed value is then copied into the 10-byte capacity that
/// matches the C++ buffer. An over-long value errors ([`MigrateError::StringTooLong`])
/// rather than silently truncating, per this crate's divergence policy.
fn read_server_id(r: &mut Reader) -> Result<String<SERVER_ID_CAP>, MigrateError> {
    let mut wide: String<64> = String::new();
    r.read_str(&mut wide)?;
    let mut server_id: String<SERVER_ID_CAP> = String::new();
    server_id
        .push_str(wide.as_str())
        .map_err(|_| MigrateError::StringTooLong)?;
    Ok(server_id)
}
