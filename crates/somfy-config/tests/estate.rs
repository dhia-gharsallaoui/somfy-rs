//! What the estate record accepts, and what it refuses.
//!
//! Same discipline as `shade.rs` next door: every refusal is reached by writing
//! the bytes a *foreign* writer would — reaching past the constructors that
//! would have prevented them — and then resealing the checksum, because a
//! record whose CRC no longer matches is refused for that reason first and
//! proves nothing about the rule under test.
//!
//! The rules being proved are the ones that would otherwise be discovered on a
//! device: a group at a sentinel address, two groups at one address, a shade
//! assigned to a room that is not there. Each of those is a value flash could
//! deliver and the registry would then refuse one entry at a time, in a log
//! line nobody reads.

use somfy_config::{
    EstateRecord, EstateRecordError, Members, Row, StoredGroup, StoredRoom, ESTATE_RECORD_LEN,
};
use somfy_domain::{GroupId, RoomId, ShadeId};
use somfy_rts::RollingCode;

// The layout, restated here rather than exported: a test that computed its
// offsets from the module under test could not catch the module moving them.
// These numbers are the ones in `estate.rs`'s own constants, and a change to
// either without the other is what this duplication is for.
const OFF_VERSION: usize = 4;
const OFF_ROOM_COUNT: usize = 6;
const OFF_GROUP_COUNT: usize = 7;
const OFF_ROOMS: usize = 16;
const ROOM_LEN: usize = 36;
const OFF_ROOM_OF: usize = OFF_ROOMS + 16 * ROOM_LEN;
const OFF_GROUPS: usize = OFF_ROOM_OF + 32;
const GROUP_LEN: usize = 44;
const OFF_CRC: usize = ESTATE_RECORD_LEN - 4;

fn hstr<const N: usize>(text: &str) -> heapless::String<N> {
    heapless::String::try_from(text).expect("fixture name fits")
}

fn room(name: &str) -> StoredRoom {
    StoredRoom { name: hstr(name) }
}

fn group(name: &str, address: u32, members: &[u8]) -> StoredGroup {
    StoredGroup {
        name: hstr(name),
        address,
        next_code: RollingCode(7),
        code_recovered: true,
        members: members
            .iter()
            .fold(Members::NONE, |set, row| set.with(ShadeId(*row))),
    }
}

/// A small, valid estate: two rooms, two shades in the first, one group.
fn estate() -> EstateRecord {
    let mut record = EstateRecord::empty(4);
    record.rooms.push(room("Downstairs")).expect("fits");
    record.rooms.push(room("Upstairs")).expect("fits");
    record.room_of[0] = Some(RoomId(0));
    record.room_of[1] = Some(RoomId(0));
    record
        .groups
        .push(group("Whole House", 0x00_9001, &[0, 1]))
        .expect("fits");
    record
}

/// Rewrite the checksum so a deliberately corrupted record is refused for the
/// reason under test rather than for its CRC.
fn reseal(bytes: &mut [u8; ESTATE_RECORD_LEN]) {
    let checksum = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..OFF_CRC]);
    bytes[OFF_CRC..].copy_from_slice(&checksum.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[test]
fn an_estate_survives_the_bytes_it_is_written_to() {
    let record = estate();
    assert_eq!(EstateRecord::decode(&record.encode()), Ok(record));
}

/// The property the ring depends on. A writer proves a write landed by reading
/// the slot back and comparing **bytes**, so two equal records must produce
/// identical ones — which is only true if every unused byte is written to a
/// fixed value rather than left as whatever was in the buffer.
#[test]
fn equal_estates_encode_identically() {
    assert_eq!(estate().encode(), estate().encode());
}

/// The rows are the ids. Nothing in the record stores one, so this is the whole
/// of the relationship and it is worth an assertion rather than a comment.
#[test]
fn the_visitors_hand_back_the_ids_the_registry_will_assign() {
    let bytes = estate().encode();
    let mut rooms: Vec<(u8, String)> = Vec::new();
    let mut assignments: Vec<(u8, u8)> = Vec::new();
    let mut groups: Vec<(u8, String)> = Vec::new();

    EstateRecord::for_each_room(&bytes, |id, room| rooms.push((id.0, room.name.to_string())))
        .expect("the rooms decode");
    EstateRecord::for_each_assignment(&bytes, |shade, room| assignments.push((shade.0, room.0)))
        .expect("the assignments decode");
    EstateRecord::for_each_group(&bytes, |id, group| {
        groups.push((id.0, group.name.to_string()))
    })
    .expect("the groups decode");

    assert_eq!(
        rooms,
        vec![(0, "Downstairs".to_string()), (1, "Upstairs".to_string())]
    );
    assert_eq!(assignments, vec![(0, 0), (1, 0)]);
    assert_eq!(groups, vec![(0, "Whole House".to_string())]);
}

/// An estate with nothing in it is a value an operator can mean — it is what
/// the interactive provisioning path writes — and it must not read as a blank
/// region, which is a different fact.
#[test]
fn an_empty_estate_is_not_a_blank_region() {
    let bytes = EstateRecord::empty(0).encode();
    assert_eq!(
        EstateRecord::decode(&bytes),
        Ok(EstateRecord::empty(0)),
        "an empty estate decodes as an empty estate"
    );
    assert_eq!(
        EstateRecord::header(&[0xFF; ESTATE_RECORD_LEN]),
        Err(EstateRecordError::Blank),
        "and an erased slot does not"
    );
}

// ---------------------------------------------------------------------------
// The record's own integrity
// ---------------------------------------------------------------------------

#[test]
fn foreign_bytes_are_reported_as_foreign_rather_than_half_read() {
    let mut bytes = estate().encode();
    bytes[0] = b'X'; // no longer RTSE
    reseal(&mut bytes);
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::Magic),
        "a region mounted at the wrong offset must be reported, not decoded"
    );
}

/// The checksum is checked before any field is interpreted, so a torn write is
/// reported as a torn write rather than as whatever its half-written header
/// happens to spell.
#[test]
fn a_torn_write_is_a_checksum_failure_and_not_a_version_failure() {
    let mut bytes = estate().encode();
    bytes[OFF_VERSION] = 0xEE; // a version this build has no reader for…
                               // …and deliberately no reseal, so both faults are present at once.
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::Checksum)
    );
}

#[test]
fn a_version_this_build_cannot_read_is_reported_as_such() {
    let mut bytes = estate().encode();
    bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&2u16.to_le_bytes());
    reseal(&mut bytes);
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::Version(2)),
        "a later format must be reported so it can be migrated, not erased"
    );
}

#[test]
fn a_count_past_the_capacity_is_refused_before_anything_is_read() {
    for (offset, expected) in [
        (OFF_ROOM_COUNT, EstateRecordError::RoomCount(17)),
        (OFF_GROUP_COUNT, EstateRecordError::GroupCount(17)),
    ] {
        let mut bytes = estate().encode();
        bytes[offset] = 17;
        reseal(&mut bytes);
        assert_eq!(EstateRecord::decode(&bytes), Err(expected));
    }
}

/// Stored lengths come off a device, so they are checked rather than trusted —
/// the same rule the shade record applies to a shade's name.
#[test]
fn a_name_length_past_the_field_is_refused_for_a_room_and_for_a_group() {
    for (offset, at) in [
        (OFF_ROOMS, Row::Room(0)),
        (OFF_GROUPS + 6, Row::Group(0)), // GROUP_NAME_LEN
    ] {
        let mut bytes = estate().encode();
        bytes[offset] = 33;
        reseal(&mut bytes);
        assert_eq!(
            EstateRecord::decode(&bytes),
            Err(EstateRecordError::NameLength { at, len: 33 })
        );
    }
}

#[test]
fn a_name_that_is_not_utf8_is_refused_rather_than_shown() {
    let mut bytes = estate().encode();
    bytes[OFF_ROOMS + 4] = 0xFF; // the first byte of the first room's name
    reseal(&mut bytes);
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::NotUtf8 { at: Row::Room(0) })
    );
}

// ---------------------------------------------------------------------------
// The rules that are about the estate rather than the bytes
// ---------------------------------------------------------------------------

/// A shade assigned to a room the record does not have. Reported rather than
/// dropped: the shade belongs to *some* room, and quietly leaving it in none
/// rearranges somebody's installation with nothing saying so.
#[test]
fn a_shade_assigned_to_a_room_that_is_not_there_is_refused() {
    let mut bytes = estate().encode();
    bytes[OFF_ROOM_OF + 1] = 5; // only rooms 0 and 1 exist
    reseal(&mut bytes);
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::RoomIndex {
            shade: ShadeId(1),
            room: 5,
        })
    );
}

/// A group is a virtual remote, so it is held to a remote's address rule. Both
/// sentinels, because both are refused for the same reason at the other end:
/// nothing can transmit as them.
#[test]
fn a_group_at_a_sentinel_address_is_refused() {
    for address in [0u32, 0xFF_FFFF] {
        let mut bytes = estate().encode();
        bytes[OFF_GROUPS..OFF_GROUPS + 4].copy_from_slice(&address.to_le_bytes());
        reseal(&mut bytes);
        assert_eq!(
            EstateRecord::decode(&bytes),
            Err(EstateRecordError::GroupAddress {
                group: GroupId(0),
                address,
            })
        );
    }
}

/// Two groups at one address means the record does not say which rolling code
/// belongs to that remote — and two counters at one address overtaking each
/// other is the failure this project was started over.
#[test]
fn two_groups_at_one_address_are_refused() {
    let mut record = estate();
    record
        .groups
        .push(group("Upstairs", 0x00_9002, &[]))
        .expect("fits");
    let mut bytes = record.encode();
    // Reach past the import, which would have refused this, and write the
    // first group's address over the second's.
    bytes[OFF_GROUPS + GROUP_LEN..OFF_GROUPS + GROUP_LEN + 4]
        .copy_from_slice(&0x00_9001u32.to_le_bytes());
    reseal(&mut bytes);
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::DuplicateAddress {
            group: GroupId(1),
            address: 0x00_9001,
        })
    );
}

/// A flag bit this version does not define is a record written by something
/// that knew more than this build does, so it is reported rather than masked
/// off — masking it would silently discard whatever the bit meant.
#[test]
fn an_unknown_group_flag_is_refused() {
    let mut bytes = estate().encode();
    bytes[OFF_GROUPS + 7] = 0b0000_0010; // GROUP_FLAGS, bit 1
    reseal(&mut bytes);
    assert_eq!(
        EstateRecord::decode(&bytes),
        Err(EstateRecordError::GroupFlags {
            group: GroupId(0),
            raw: 0b0000_0010,
        })
    );
}

/// All or nothing. Rooms and groups take their ids from position too, so
/// loading the survivors of a bad record would renumber the rest and move every
/// membership off the group it belonged to.
#[test]
fn one_bad_row_visits_nothing_at_all() {
    let mut record = estate();
    record
        .groups
        .push(group("Upstairs", 0x00_9002, &[]))
        .expect("fits");
    let mut bytes = record.encode();
    bytes[OFF_GROUPS + GROUP_LEN..OFF_GROUPS + GROUP_LEN + 4].copy_from_slice(&0u32.to_le_bytes());
    reseal(&mut bytes);

    // The bad row is a *group*, and what is asserted is that the perfectly good
    // rooms above it are still not placed — by any of the three walks, since
    // each of them validates the whole record before visiting anything.
    let mut visited = 0;
    assert!(EstateRecord::for_each_room(&bytes, |_, _| visited += 1).is_err());
    assert!(EstateRecord::for_each_assignment(&bytes, |_, _| visited += 1).is_err());
    assert!(EstateRecord::for_each_group(&bytes, |_, _| visited += 1).is_err());
    assert_eq!(
        visited, 0,
        "the rooms are perfectly good and must still not be placed"
    );
}

/// The rolling-code warning, carried in the record rather than only in a
/// terminal. A group imported from a backup too old to contain its code is
/// stored with the flag clear, and it has to survive the round trip or the
/// warning is lost the moment the tool exits.
#[test]
fn a_fabricated_rolling_code_is_still_flagged_after_a_round_trip() {
    let mut record = EstateRecord::empty(0);
    let mut fabricated = group("Whole House", 0x00_9001, &[]);
    fabricated.next_code = RollingCode(1);
    fabricated.code_recovered = false;
    record.groups.push(fabricated).expect("fits");

    let decoded = EstateRecord::decode(&record.encode()).expect("decodes");
    assert!(!decoded.groups[0].code_recovered);
    assert_eq!(decoded.groups[0].next_code, RollingCode(1));
}

// ---------------------------------------------------------------------------
// The membership bitmap
// ---------------------------------------------------------------------------

/// A row past the shade table names no slot, so nothing can have joined it. The
/// bound is the whole content of this type — a shift past the end of the word
/// would be undefined rather than merely wrong.
#[test]
fn a_row_past_the_shade_table_is_never_a_member() {
    let full = Members::from_bits(u32::MAX);
    assert_eq!(full.len(), 32);
    assert!(!full.contains(ShadeId(32)));
    assert!(!full.contains(ShadeId(255)));
    assert_eq!(
        full.with(ShadeId(200)),
        full,
        "an out-of-range add is a no-op"
    );
}

#[test]
fn the_empty_set_is_empty_and_a_populated_one_is_not() {
    assert!(Members::NONE.is_empty());
    assert_eq!(Members::NONE.len(), 0);
    let one = Members::NONE.with(ShadeId(3));
    assert!(!one.is_empty());
    assert_eq!(one.ids().collect::<Vec<_>>(), vec![ShadeId(3)]);
}
