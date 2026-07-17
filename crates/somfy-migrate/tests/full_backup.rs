//! Full-backup pipeline tests: room + group records and `parse_backup`.
//!
//! Fixtures reproduce the exact record order C++ `ShadeConfigFile::backup`
//! emits (`src/ConfigFile.cpp:348-382`): header, room records
//! (`writeRoomRecord` :964-968), shade records (`writeShadeRecord` :970-1018),
//! group records (`writeGroupRecord` :941-957), then the repeater/settings/net/
//! trans records this migrator skips. The C++ writer pads every field to a fixed
//! width, but [`somfy_migrate::Reader`] tolerates padding (`atoi`/`_rtrim`), so
//! the readable fixtures omit it — [`padded_group_record_is_200_bytes`] covers
//! the padded form and cross-checks `GROUP_REC_SIZE` (`ConfigFile.cpp:13`).

use somfy_migrate::{
    parse_backup, parse_group_record, parse_room_record, BackupHeader, MigrateError, MigratedGroup,
    MigratedRoom, Reader,
};
use somfy_rts::RollingCode;

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

/// A modern (v25) header; `version` and the record counts drive `parse_backup`.
fn header(version: u8, rooms: u8, shades: u8, groups: u8) -> BackupHeader {
    BackupHeader {
        version,
        length: 76,
        room_record_size: 29,
        room_records: rooms,
        shade_record_size: 276,
        shade_records: shades,
        group_record_size: 200,
        group_records: groups,
        repeater_record_size: 77,
        repeater_records: 1,
        settings_record_size: 552,
        net_record_size: 318,
        trans_record_size: 78,
        server_id: heapless::String::new(),
    }
}

/// A room record: `roomId, name, sortOrder` (`writeRoomRecord` :964-968).
fn room_fields(id: &str, name: &str, sort: &str) -> Vec<String> {
    vec![id.into(), name.into(), sort.into()]
}

/// A v19+ shade record (34 fields) in `writeShadeRecord` order (:970-1018).
/// Only the identifying fields vary between fixtures; the rest are canonical.
fn shade_fields(id: &str, addr: &str, name: &str, rolling: &str, room: &str) -> Vec<String> {
    vec![
        id.into(),         // 0  shadeId
        "true".into(),     // 1  paired (skip)
        "1".into(),        // 2  shadeType (blind)
        addr.into(),       // 3  remoteAddress
        name.into(),       // 4  name
        "2".into(),        // 5  tiltType
        "1".into(),        // 6  proto
        "56".into(),       // 7  bitLength
        "30000".into(),    // 8  upTime
        "29000".into(),    // 9  downTime
        "5000".into(),     // 10 tiltTime
        "100".into(),      // 11 stepSize (skip)
        "0".into(),        // 12 linkedRemote0
        "0".into(),        // 13 linkedRemote1
        "0".into(),        // 14 linkedRemote2
        "0".into(),        // 15 linkedRemote3
        "0".into(),        // 16 linkedRemote4
        "0".into(),        // 17 linkedRemote5
        "0".into(),        // 18 linkedRemote6
        rolling.into(),    // 19 lastRollingCode
        "0".into(),        // 20 flags
        "-1.00000".into(), // 21 myPos
        "-1.00000".into(), // 22 myTiltPos (skip)
        "50.00000".into(), // 23 currentPos
        "0.00000".into(),  // 24 currentTiltPos
        "false".into(),    // 25 flipCommands (skip)
        "false".into(),    // 26 flipPosition (skip)
        "1".into(),        // 27 repeats (skip)
        "2".into(),        // 28 sortOrder (skip)
        "0".into(),        // 29 gpioUp (skip)
        "0".into(),        // 30 gpioDown (skip)
        "0".into(),        // 31 gpioMy (skip)
        "0".into(),        // 32 gpioFlags (skip)
        room.into(),       // 33 roomId
    ]
}

/// A v24/v25 group record in `writeGroupRecord` order (:941-957): the rolling
/// code is the final, `\n`-terminated field. `members` are the up-to-32 linked
/// shade ids (non-zero); the remaining slots are written as `0`.
fn group_fields_v25(
    id: &str,
    group_type: &str,
    addr: &str,
    name: &str,
    members: &[&str],
    rolling: &str,
) -> Vec<String> {
    let mut f: Vec<String> = vec![
        id.into(),         // 1 groupId
        group_type.into(), // 2 groupType (skip)
        addr.into(),       // 3 remoteAddress
        name.into(),       // 4 name
        "1".into(),        // 5 proto (skip)
        "56".into(),       // 6 bitLength (skip)
    ];
    // 7 linkedShades: 32 slots, non-zero first then padded with 0.
    for j in 0..32 {
        f.push(members.get(j).copied().unwrap_or("0").into());
    }
    f.push("1".into()); // 8 repeats (skip)
    f.push("3".into()); // 9 sortOrder (skip)
    f.push("false".into()); // 10 flipCommands (skip)
    f.push("2".into()); // 11 roomId (skip)
    f.push(rolling.into()); // 12 lastRollingCode (\n-terminated)
    f
}

/// A v23 group record: `lastRollingCode` sits *before* the linked shades
/// (`readGroupRecord` :747), and the record has no trailing rolling code.
fn group_fields_v23(
    id: &str,
    addr: &str,
    name: &str,
    members: &[&str],
    rolling: &str,
) -> Vec<String> {
    let mut f: Vec<String> = vec![
        id.into(),      // groupId
        "0".into(),     // groupType (skip)
        addr.into(),    // remoteAddress
        name.into(),    // name
        "1".into(),     // proto (skip)
        "56".into(),    // bitLength (skip)
        rolling.into(), // lastRollingCode (v23 position, :747)
    ];
    for j in 0..32 {
        f.push(members.get(j).copied().unwrap_or("0").into());
    }
    f.push("1".into()); // repeats (skip)
    f.push("3".into()); // sortOrder (skip)
    f.push("false".into()); // flipCommands (skip)
    f.push("2".into()); // roomId (skip, \n-terminated for v23 layout)
    f
}

/// A v19–v22 group record: the file carries NO rolling code at all
/// (`readGroupRecord` sources it from NVS only, :764-767). `roomId` is the final,
/// `\n`-terminated field; there is neither a mid-record (v23) nor a trailing
/// (v24+) `lastRollingCode`.
fn group_fields_v19(id: &str, addr: &str, name: &str, members: &[&str]) -> Vec<String> {
    let mut f: Vec<String> = vec![
        id.into(),   // groupId
        "0".into(),  // groupType (skip)
        addr.into(), // remoteAddress
        name.into(), // name
        "1".into(),  // proto (skip)
        "56".into(), // bitLength (skip)
    ];
    // linkedShades: 32 slots, immediately after bitLength (no rolling code here).
    for j in 0..32 {
        f.push(members.get(j).copied().unwrap_or("0").into());
    }
    f.push("1".into()); // repeats (skip)
    f.push("3".into()); // sortOrder (skip)
    f.push("false".into()); // flipCommands (skip)
    f.push("2".into()); // roomId (skip, terminal \n-terminated field)
    f
}

fn line(fields: &[String]) -> Vec<u8> {
    let joined: Vec<&str> = fields.iter().map(|s| s.as_str()).collect();
    let mut out = joined.join(",").into_bytes();
    out.push(b'\n');
    out
}

fn header_line(version: u8, rooms: u8, shades: u8, groups: u8) -> Vec<u8> {
    // writeHeader order (ConfigFile.cpp:47-60). The repeater pair
    // (repeaterSize,repeaterRecs) only exists for version >= 21; readHeader
    // gates its read on the same version (:81-84), so v19/v20 lines must omit it
    // or parse_header misaligns. Fields not consulted use plausible sizes.
    let repeater = if version >= 21 { "77,1," } else { "" };
    format!("{version},76,29,{rooms},276,{shades},200,{groups},{repeater}552,318,78,SrvBackup\n")
        .into_bytes()
}

// ---------------------------------------------------------------------------
// Room record unit tests
// ---------------------------------------------------------------------------

fn parse_room(fields: &[String], version: u8) -> MigratedRoom {
    let bytes = line(fields);
    let mut r = Reader::new(&bytes);
    parse_room_record(&mut r, &header(version, 0, 0, 0)).expect("room parses")
}

#[test]
fn room_record_maps_id_and_name() {
    let room = parse_room(&room_fields("4", "Living Room", "0"), 25);
    assert_eq!(room.room_id, 4);
    assert_eq!(room.name.as_str(), "Living Room");
}

#[test]
fn room_name_padding_is_rtrimmed() {
    let room = parse_room(&room_fields("2", "Den                 ", "1"), 25);
    assert_eq!(room.name.as_str(), "Den");
}

#[test]
fn room_layout_is_version_invariant() {
    // readRoomRecord has no version gates (:789-798): v19 and v25 decode alike.
    let f = room_fields("7", "Office", "0");
    assert_eq!(parse_room(&f, 19), parse_room(&f, 25));
}

// ---------------------------------------------------------------------------
// Group record unit tests
// ---------------------------------------------------------------------------

fn parse_group(fields: &[String], version: u8) -> MigratedGroup {
    let bytes = line(fields);
    let mut r = Reader::new(&bytes);
    parse_group_record(&mut r, &header(version, 0, 0, 1)).expect("group parses")
}

#[test]
fn group_record_maps_core_fields() {
    let g = parse_group(
        &group_fields_v25("5", "0", "9000000", "Downstairs", &["1", "2", "3"], "40"),
        25,
    );
    assert_eq!(g.group_id, 5);
    assert_eq!(g.name.as_str(), "Downstairs");
    assert_eq!(g.address, 9000000);
    assert_eq!(g.member_shade_ids.as_slice(), &[1, 2, 3]);
}

#[test]
fn group_is_a_virtual_remote_with_rolling_code_plus_one() {
    // Groups are their own remotes: apply the SAME +1 migration contract.
    let g = parse_group(
        &group_fields_v25("1", "0", "9000001", "All", &["1"], "41"),
        25,
    );
    assert_eq!(g.next_code, RollingCode(42));
}

#[test]
fn group_rolling_code_wraps_at_max() {
    let g = parse_group(
        &group_fields_v25("1", "0", "9000001", "All", &["1"], "65535"),
        25,
    );
    assert_eq!(g.next_code, RollingCode(0));
}

#[test]
fn group_member_ids_are_compacted() {
    // readGroupRecord drops 0 slots and preserves order (:750-754).
    let g = parse_group(
        &group_fields_v25("1", "0", "9000001", "Mix", &["0", "10", "0", "20"], "5"),
        25,
    );
    assert_eq!(g.member_shade_ids.as_slice(), &[10, 20]);
}

#[test]
fn group_all_members_populated() {
    let all: Vec<&str> = (1..=32u8).map(|_| "9").collect();
    let g = parse_group(
        &group_fields_v25("1", "0", "9000001", "Full", &all, "1"),
        25,
    );
    assert_eq!(g.member_shade_ids.len(), 32);
}

#[test]
fn group_v23_reads_rolling_code_mid_record() {
    // In v23 the rolling code precedes the linked shades (readGroupRecord :747),
    // and there is no trailing rolling code; the +1 contract still applies.
    let g = parse_group(
        &group_fields_v23("2", "9000002", "Legacy", &["4", "5"], "100"),
        23,
    );
    assert_eq!(g.group_id, 2);
    assert_eq!(g.member_shade_ids.as_slice(), &[4, 5]);
    assert_eq!(g.next_code, RollingCode(101));
}

#[test]
fn group_v19_fabricates_rolling_code_and_stays_aligned() {
    // THE named migration risk: a v19-22 group record carries NO rolling code
    // (both gates false — v != 23 at :747, v < 24 at :763; the C++ sources it
    // from NVS only, :764-767). `next_code` is therefore FABRICATED as
    // RollingCode(1) (stored 0 -> +1). `roomId` is the terminal, \n-terminated
    // field, so a following record must still parse (the resync/alignment net).
    let g1 = group_fields_v19("2", "9000002", "OldGroup", &["4", "5"]);
    let g2 = group_fields_v19("3", "9000003", "OtherGroup", &["6"]);
    let mut bytes = line(&g1);
    bytes.extend(line(&g2));

    let mut r = Reader::new(&bytes);
    let h = header(19, 0, 0, 2);
    let a = parse_group_record(&mut r, &h).unwrap();
    let b = parse_group_record(&mut r, &h).unwrap();

    // First record fields align despite the absent rolling code.
    assert_eq!(a.group_id, 2);
    assert_eq!(a.name.as_str(), "OldGroup");
    assert_eq!(a.address, 9000002);
    assert_eq!(a.member_shade_ids.as_slice(), &[4, 5]);
    assert_eq!(a.next_code, RollingCode(1), "fabricated: no code in file");

    // Second record parsed cleanly => roomId's \n realigned the cursor.
    assert_eq!(b.group_id, 3);
    assert_eq!(b.name.as_str(), "OtherGroup");
    assert_eq!(b.member_shade_ids.as_slice(), &[6]);
    assert_eq!(b.next_code, RollingCode(1));
    assert!(r.at_end(), "both v19 group records consumed exactly");
}

#[test]
fn full_backup_with_v19_group_fabricates_code() {
    // The same risk through the whole pipeline: a v19 backup's group surfaces a
    // fabricated RollingCode(1), and the trailing record is still skipped cleanly.
    let mut bytes = header_line(19, 0, 0, 1);
    bytes.extend(line(&group_fields_v19(
        "4",
        "9000004",
        "Legacy",
        &["7", "8"],
    )));
    bytes.extend(b"       0,       0,       0,       0\n"); // repeater
    let m = parse_backup(&bytes).expect("v19 backup parses");
    assert_eq!(m.version, 19);
    assert_eq!(m.groups.len(), 1);
    assert_eq!(m.groups[0].group_id, 4);
    assert_eq!(m.groups[0].member_shade_ids.as_slice(), &[7, 8]);
    assert_eq!(m.groups[0].next_code, RollingCode(1));
}

#[test]
fn padded_group_record_is_200_bytes() {
    // Build the record with the exact C++ field widths (writeUInt8 %3u,
    // writeUInt16 %5u, writeUInt32 %10u, writeBool width 5, name padded to 20)
    // and confirm it equals GROUP_REC_SIZE (ConfigFile.cpp:13).
    let mut s = String::new();
    s.push_str(&format!("{:3},", 5u32)); // groupId
    s.push_str(&format!("{:3},", 0u32)); // groupType
    s.push_str(&format!("{:10},", 9000000u32)); // remoteAddress
    s.push_str(&format!("{:<20},", "Downstairs")); // name
    s.push_str(&format!("{:3},", 1u32)); // proto
    s.push_str(&format!("{:3},", 56u32)); // bitLength
    for j in 0..32u32 {
        let v = if j < 3 { j + 1 } else { 0 };
        s.push_str(&format!("{:3},", v)); // linkedShades
    }
    s.push_str(&format!("{:3},", 1u32)); // repeats
    s.push_str(&format!("{:3},", 3u32)); // sortOrder
    s.push_str("false,"); // flipCommands (width 5 + sep)
    s.push_str(&format!("{:3},", 2u32)); // roomId
    s.push_str(&format!("{:5}\n", 40u32)); // lastRollingCode + CFG_REC_END
    let bytes = s.into_bytes();
    assert_eq!(bytes.len(), 200, "must equal C++ GROUP_REC_SIZE");

    let mut r = Reader::new(&bytes);
    let g = parse_group_record(&mut r, &header(25, 0, 0, 1)).unwrap();
    assert_eq!(g.group_id, 5);
    assert_eq!(g.name.as_str(), "Downstairs");
    assert_eq!(g.address, 9000000);
    assert_eq!(g.member_shade_ids.as_slice(), &[1, 2, 3]);
    assert_eq!(g.next_code, RollingCode(41));
    assert!(r.at_end(), "the 200-byte record consumes exactly to EOF");
}

// ---------------------------------------------------------------------------
// Full backup integration
// ---------------------------------------------------------------------------

/// Assemble a complete v25 backup: header, 2 rooms, 2 shades, 1 group, then the
/// repeater/settings/net/trans records that `parse_backup` must skip.
fn full_backup_bytes() -> Vec<u8> {
    let mut bytes = header_line(25, 2, 2, 1);
    bytes.extend(line(&room_fields("1", "Kitchen", "0")));
    bytes.extend(line(&room_fields("2", "Bedroom", "1")));
    bytes.extend(line(&shade_fields("10", "1111111", "Blind A", "41", "1")));
    bytes.extend(line(&shade_fields("11", "2222222", "Blind B", "99", "2")));
    bytes.extend(line(&group_fields_v25(
        "3",
        "0",
        "9000000",
        "Whole House",
        &["10", "11"],
        "7",
    )));
    // Trailing records the migrator skips (present in a `backup`, ConfigFile.cpp:378-381).
    bytes.extend(b"       0,       0,       0,       0\n"); // repeater (4 addrs)
    bytes.extend(b"settings,record,data\n"); // settings record
    bytes.extend(b"net,record,data\n"); // net record
    bytes.extend(b"trans,record,data\n"); // trans record
    bytes
}

#[test]
fn full_backup_parses_all_entities() {
    let data = full_backup_bytes();
    let m = parse_backup(&data).expect("backup parses");

    assert_eq!(m.version, 25);
    assert_eq!(m.server_id.as_str(), "SrvBackup");
    // A well-formed backup aligns every record exactly: no data-skipping resyncs.
    assert_eq!(m.skipped_resyncs, 0);

    // Rooms
    assert_eq!(m.rooms.len(), 2);
    assert_eq!(m.rooms[0].room_id, 1);
    assert_eq!(m.rooms[0].name.as_str(), "Kitchen");
    assert_eq!(m.rooms[1].room_id, 2);
    assert_eq!(m.rooms[1].name.as_str(), "Bedroom");

    // Shades (rolling code +1 preserved through the pipeline)
    assert_eq!(m.shades.len(), 2);
    assert_eq!(m.shades[0].shade_id, 10);
    assert_eq!(m.shades[0].name.as_str(), "Blind A");
    assert_eq!(m.shades[0].next_code, RollingCode(42));
    assert_eq!(m.shades[0].room_id, 1);
    assert_eq!(m.shades[1].shade_id, 11);
    assert_eq!(m.shades[1].next_code, RollingCode(100));

    // Group (virtual remote, +1 rolling code, membership intact)
    assert_eq!(m.groups.len(), 1);
    assert_eq!(m.groups[0].group_id, 3);
    assert_eq!(m.groups[0].name.as_str(), "Whole House");
    assert_eq!(m.groups[0].address, 9000000);
    assert_eq!(m.groups[0].next_code, RollingCode(8));
    assert_eq!(m.groups[0].member_shade_ids.as_slice(), &[10, 11]);
}

#[test]
fn backup_without_trailing_records_parses() {
    // A minimal file ending right after the last group record still parses.
    let mut bytes = header_line(25, 1, 1, 1);
    bytes.extend(line(&room_fields("1", "Hall", "0")));
    bytes.extend(line(&shade_fields("5", "3333333", "Shade", "0", "1")));
    bytes.extend(line(&group_fields_v25(
        "2",
        "0",
        "9000000",
        "Grp",
        &["5"],
        "3",
    )));
    let m = parse_backup(&bytes).expect("parses without trailing records");
    assert_eq!(m.rooms.len(), 1);
    assert_eq!(m.shades.len(), 1);
    assert_eq!(m.groups.len(), 1);
    assert_eq!(m.shades[0].next_code, RollingCode(1)); // 0 -> 1
    assert_eq!(m.groups[0].next_code, RollingCode(4));
}

#[test]
fn empty_collections_parse() {
    // Zero rooms/shades/groups: header counts all 0, only trailing records.
    let mut bytes = header_line(25, 0, 0, 0);
    bytes.extend(b"       0,       0,       0,       0\n"); // repeater
    let m = parse_backup(&bytes).expect("empty backup parses");
    assert!(m.rooms.is_empty());
    assert!(m.shades.is_empty());
    assert!(m.groups.is_empty());
}

#[test]
fn cleared_sentinel_records_are_filtered() {
    // Defensive: a roomId 0 / shadeId 255 / groupId 255 record is a cleared
    // slot (C++ save never writes these; restore clear()s such slots). If one
    // appears it must not surface as a live entity.
    let mut bytes = header_line(25, 1, 1, 1);
    bytes.extend(line(&room_fields("0", "", "0"))); // cleared room
    bytes.extend(line(&shade_fields("255", "0", "", "0", "0"))); // cleared shade
    bytes.extend(line(&group_fields_v25("255", "0", "0", "", &[], "0"))); // cleared group
    let m = parse_backup(&bytes).expect("parses");
    assert!(m.rooms.is_empty(), "roomId 0 filtered");
    assert!(m.shades.is_empty(), "shadeId 255 filtered");
    assert!(m.groups.is_empty(), "groupId 255 filtered");
}

#[test]
fn record_with_extra_trailing_field_is_resynced() {
    // Simulate a record carrying a field beyond what this parser models (e.g. a
    // future minor addition). The defensive resync must skip it so the following
    // record still aligns — a port of the C++ seekChar(CFG_REC_END) net.
    let mut bytes = header_line(25, 2, 0, 0);
    // First room with an EXTRA trailing field appended after sortOrder.
    let mut room1 = room_fields("1", "Kitchen", "0");
    room1.push("999".into()); // unmodeled trailing field
    bytes.extend(line(&room1));
    bytes.extend(line(&room_fields("2", "Bedroom", "1")));

    let m = parse_backup(&bytes).expect("resyncs past the extra field");
    assert_eq!(m.rooms.len(), 2);
    assert_eq!(m.rooms[0].room_id, 1);
    assert_eq!(m.rooms[0].name.as_str(), "Kitchen");
    assert_eq!(m.rooms[1].room_id, 2);
    assert_eq!(m.rooms[1].name.as_str(), "Bedroom");
    // The extra trailing field on room 1 forced a resync that skipped real bytes;
    // the well-formed room 2 did not. The counter surfaces the misalignment so
    // Plan 6 can warn instead of silently trusting the parse.
    assert_eq!(
        m.skipped_resyncs, 1,
        "one record needed a data-skipping resync"
    );
}

// ---------------------------------------------------------------------------
// Malformed inputs
// ---------------------------------------------------------------------------

#[test]
fn truncated_file_is_eof() {
    // Cut the backup off midway through the first shade record.
    let full = full_backup_bytes();
    let cut = full.len() - 120;
    let truncated = &full[..cut];
    assert_eq!(parse_backup(truncated), Err(MigrateError::UnexpectedEof));
}

#[test]
fn shade_count_larger_than_records_is_eof() {
    // Header claims 3 shades but only 1 record (and nothing) follows: the third
    // read runs out of input rather than panicking.
    let mut bytes = header_line(25, 0, 3, 0);
    bytes.extend(line(&shade_fields("1", "1000", "Only", "0", "1")));
    assert_eq!(parse_backup(&bytes), Err(MigrateError::UnexpectedEof));
}

#[test]
fn version_13_is_unsupported() {
    let bytes = header_line(13, 0, 0, 0);
    assert_eq!(
        parse_backup(&bytes),
        Err(MigrateError::UnsupportedVersion(13))
    );
}

#[test]
fn version_26_ceiling_is_unsupported() {
    // A future v26 could append fields and silently misalign: reject it up front.
    let bytes = header_line(26, 0, 0, 0);
    assert_eq!(
        parse_backup(&bytes),
        Err(MigrateError::UnsupportedVersion(26))
    );
}
