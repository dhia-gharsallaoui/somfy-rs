//! Top-level backup assembly: header + record loops → [`MigrationData`].
//!
//! Ports the read order of C++ `ShadeConfigFile::loadFile`/`restoreFile`
//! (`src/ConfigFile.cpp:886-940`, `:515-563`), which mirrors the write order of
//! `save`/`backup` (`:315-383`): header, then `roomRecords` room records,
//! `shadeRecords` shade records, `groupRecords` group records, then the
//! repeater/settings/net/trans records this migrator does not model.
//!
//! ## Cleared-slot filtering
//!
//! The C++ writer never emits cleared slots — `save`/`backup` skip rooms with
//! `roomId == 0` (`:332`), shades with `shadeId == 255` (`:337`), and groups
//! with `groupId == 255` (`:342`) — and the record *counts* in the header are
//! the live-entity counts (`roomCount`/`shadeCount`/`groupCount`,
//! `Somfy.cpp:3705-3725`). On the read side, `loadFile`/`restore` load exactly
//! those counts and `clear()` any slots not present in the file
//! (`:913-918`, `:923-928`, `:542-547`). So a cleared sentinel record should
//! never appear; if one does (a hand-edited or corrupt backup), it is filtered
//! rather than surfaced as a live entity — matching the C++ intent.
//!
//! ## Trailing records
//!
//! The repeater (if `version >= 21`), settings and transceiver records that
//! follow the groups are not modeled. They are skipped by record end (`\n`),
//! one per record the header counts, trusting the separators rather than the
//! advisory record-size fields.
//!
//! The **net** record between them is modeled, because the broker settings live
//! in it — see [`parse_net_record`]. Reaching it is why the skips above are
//! counted rather than run to EOF: a trailer record has to be stepped over
//! exactly once, not swallowed along with everything after it.
//!
//! ## What tells the trailers apart from an on-flash config
//!
//! The header's record *sizes*. `ShadeConfigFile::save` (`:315-346`) writes the
//! on-flash `shades.cfg` with the settings, net and transceiver sizes set to
//! **zero** and none of those records emitted; `ShadeConfigFile::backup`
//! (`:347-383`) sizes and writes all three. So a size of zero means the record
//! is absent, which is exactly the test `restoreFile` itself applies before
//! seeking past one (`:583-596`).

use crate::header::{parse_header, BackupHeader};
use crate::reader::{MigrateError, Reader};
use crate::records::{
    parse_group_record, parse_net_record, parse_room_record, parse_shade_record, MigratedGroup,
    MigratedMqtt, MigratedRoom, MigratedShade,
};
use heapless::{String, Vec};

/// Rooms per backup — C++ `SOMFY_MAX_ROOMS` (`Somfy.h:10`).
const MAX_ROOMS: usize = 16;
/// Shades per backup — C++ `SOMFY_MAX_SHADES` (`Somfy.h:6`).
const MAX_SHADES: usize = 32;
/// Groups per backup — C++ `SOMFY_MAX_GROUPS` (`Somfy.h:7`).
const MAX_GROUPS: usize = 16;
/// `serverId` capacity — C++ `char serverId[10]` (`ConfigFile.h:28`).
const SERVER_ID_CAP: usize = 10;

/// Everything a file-only backup migration can recover from a C++ backup.
///
/// The three collections are the live entities in slot order, with cleared
/// sentinel slots filtered out (see the module docs). Rolling codes on shades
/// and groups already carry the `+1` migration contract from their record
/// parsers, so this struct is ready to hand to the domain layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationData {
    /// Backup format version (`19..=25`).
    pub version: u8,
    /// Server identifier from the header (max 10 bytes).
    pub server_id: String<SERVER_ID_CAP>,
    /// Live rooms in slot order.
    pub rooms: Vec<MigratedRoom, MAX_ROOMS>,
    /// Live shades in slot order.
    pub shades: Vec<MigratedShade, MAX_SHADES>,
    /// Live groups in slot order.
    pub groups: Vec<MigratedGroup, MAX_GROUPS>,
    /// The broker settings the backup carried, or `None` when none could be
    /// read.
    ///
    /// `None` has three causes and they are not the same fact: an on-flash
    /// `shades.cfg` rather than an exported backup (no net record at all), a
    /// backup below version 22 (a net record with no MQTT block), or a net
    /// record that did not read — see [`parse_backup`], which does not let that
    /// refuse the migration.
    ///
    /// A controller that simply had **no broker configured** is a fourth thing
    /// and is *not* `None`: it arrives as `Some` with an empty
    /// [`hostname`](MigratedMqtt::hostname), because "no broker" is a
    /// configuration somebody meant and the consumer is entitled to tell it
    /// apart from a backup that could not be read.
    pub mqtt: Option<MigratedMqtt>,
    /// Count of records whose fields did not align exactly, forcing the defensive
    /// resync to skip leftover content bytes before the record end.
    ///
    /// **Nonzero means record fields didn't align exactly — data MAY be
    /// misparsed** (e.g. an unescaped comma inside a name shifts every following
    /// field, which can produce a *plausible but wrong* rolling code). Plan 6 must
    /// warn and show the user the imported values for confirmation instead of
    /// silently applying them. A well-formed backup always yields `0`.
    pub skipped_resyncs: u16,
}

/// Parse a complete C++ backup buffer into [`MigrationData`].
///
/// Reads the header (rejecting versions outside `19..=25`), then the room,
/// shade, and group records in the C++ `save`/`backup` order, resyncing to each
/// record boundary defensively after every record (a faithful port of the C++
/// `seekChar(CFG_REC_END)` net). Cleared sentinel records are filtered. Any
/// trailing repeater/settings/net/trans records are skipped to EOF.
///
/// # Errors
///
/// - [`MigrateError::UnsupportedVersion`] if the header version is not `19..=25`.
/// - [`MigrateError::UnexpectedEof`] if any declared record is truncated or the
///   header record counts exceed the records actually present.
/// - [`MigrateError::StringTooLong`] / [`MigrateError::BadRecord`] on a
///   malformed field, or if the live-entity count exceeds the C++ slot capacity.
pub fn parse_backup(data: &[u8]) -> Result<MigrationData, MigrateError> {
    let mut r = Reader::new(data);
    let header = parse_header(&mut r)?;

    let mut skipped_resyncs: u16 = 0;
    let rooms = parse_rooms(&mut r, &header, &mut skipped_resyncs)?;
    let shades = parse_shades(&mut r, &header, &mut skipped_resyncs)?;
    let groups = parse_groups(&mut r, &header, &mut skipped_resyncs)?;

    // The repeater and settings records sit between the groups and the net
    // record, so they are stepped over one at a time rather than run to EOF —
    // the net record is the one trailer this migrator reads. These are expected
    // extra records, not misalignments, so they never touch the counter.
    for _ in 0..header.repeater_records {
        r.skip_record_end()?;
    }
    if header.settings_record_size > 0 {
        r.skip_record_end()?;
    }
    // **Best effort, and deliberately.** A failure inside the net record does not
    // refuse the backup: the broker settings are recoverable by hand — they are
    // on the old controller's screen — while the rolling codes above are not,
    // and refusing the whole migration over an unreadable trailer would trade
    // the irrecoverable value for the recoverable one. A header that claims a
    // net record the file does not have is the ordinary case here, not a
    // corruption: `full_backup.rs` has carried a fixture like that since the
    // record counts were advisory.
    let mqtt = parse_net_record(&mut r, &header).ok().flatten();

    // Whatever is left — the transceiver record, and the net record's Ethernet
    // tail if a backup ever turns out to be shorter there than the writer this
    // was read against.
    while !r.at_end() {
        r.skip_record_end()?;
    }

    Ok(MigrationData {
        version: header.version,
        server_id: header.server_id,
        rooms,
        shades,
        groups,
        mqtt,
        skipped_resyncs,
    })
}

/// Bump `skipped` when a defensive [`Reader::resync_record`] had to skip leftover
/// content bytes — i.e. the record did not align exactly (see
/// [`MigrationData::skipped_resyncs`]).
fn note_resync(r: &mut Reader, skipped: &mut u16) -> Result<(), MigrateError> {
    if r.resync_record()? > 0 {
        *skipped = skipped.saturating_add(1);
    }
    Ok(())
}

fn parse_rooms(
    r: &mut Reader,
    header: &BackupHeader,
    skipped: &mut u16,
) -> Result<Vec<MigratedRoom, MAX_ROOMS>, MigrateError> {
    let mut rooms = Vec::new();
    for _ in 0..header.room_records {
        let room = parse_room_record(r, header)?;
        note_resync(r, skipped)?;
        // roomId 0 is a cleared slot; the C++ writer never emits it (:332).
        if room.room_id != 0 {
            rooms
                .push(room)
                .map_err(|_| MigrateError::BadRecord("too_many_rooms"))?;
        }
    }
    Ok(rooms)
}

fn parse_shades(
    r: &mut Reader,
    header: &BackupHeader,
    skipped: &mut u16,
) -> Result<Vec<MigratedShade, MAX_SHADES>, MigrateError> {
    let mut shades = Vec::new();
    for _ in 0..header.shade_records {
        let shade = parse_shade_record(r, header)?;
        note_resync(r, skipped)?;
        // shadeId 255 is a cleared slot; the C++ writer never emits it (:337).
        if shade.shade_id != 255 {
            shades
                .push(shade)
                .map_err(|_| MigrateError::BadRecord("too_many_shades"))?;
        }
    }
    Ok(shades)
}

fn parse_groups(
    r: &mut Reader,
    header: &BackupHeader,
    skipped: &mut u16,
) -> Result<Vec<MigratedGroup, MAX_GROUPS>, MigrateError> {
    let mut groups = Vec::new();
    for _ in 0..header.group_records {
        let group = parse_group_record(r, header)?;
        note_resync(r, skipped)?;
        // groupId 255 is a cleared slot; the C++ writer never emits it (:342).
        if group.group_id != 255 {
            groups
                .push(group)
                .map_err(|_| MigrateError::BadRecord("too_many_groups"))?;
        }
    }
    Ok(groups)
}
