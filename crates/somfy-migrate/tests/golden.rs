//! Golden migration tests: the format-truth checkpoint for `parse_backup`.
//!
//! Two layers, mirroring the golden-capture discipline used by `somfy-rts`:
//!
//! 1. [`pipeline_locks_full_migration_data`] — a **checked-in, always-run**
//!    round-trip. It assembles a complete v25 backup from the discovered field
//!    map (the same synthetic shape the Task-7 `full_backup` suite builds) and
//!    asserts the parsed [`MigrationData`] equals an explicitly constructed
//!    expected value, **field for field**. Unlike the piecemeal assertions in
//!    `full_backup.rs`, this pins the *entire* struct: any change to a field
//!    mapping, a default, or the record order breaks it. This is the pipeline
//!    lock.
//!
//! 2. [`real_device_backup_satisfies_structural_invariants`] — the
//!    `#[ignore]`d real-backup authority. It parses a backup exported from a
//!    running C++ device and asserts structural invariants that must hold for
//!    any real device, without hard-coding device-specific values. It stays
//!    ignored (and the suite stays green) until a real capture is placed at
//!    `tests/fixtures/real_device.backup` — see that directory's README. The
//!    file is gitignored: it carries radio addresses and rolling codes.

use somfy_migrate::{parse_backup, MigratedGroup, MigratedRoom, MigratedShade, MigrationData};
use somfy_rts::RollingCode;

// ---------------------------------------------------------------------------
// Layer 1: always-run pipeline lock
// ---------------------------------------------------------------------------

/// Build a heapless `String` from a fixture literal that is known to fit.
fn hstr<const N: usize>(text: &str) -> heapless::String<N> {
    let mut out = heapless::String::new();
    out.push_str(text).expect("fixture string fits capacity");
    out
}

/// Build a heapless `Vec` from a fixture slice that is known to fit.
fn hvec<T: Clone, const N: usize>(items: &[T]) -> heapless::Vec<T, N> {
    let mut out = heapless::Vec::new();
    out.extend_from_slice(items)
        .expect("fixture vec fits capacity");
    out
}

/// Assemble a complete v25 backup byte-for-byte in the C++ `backup` write order
/// (`src/ConfigFile.cpp:348-382`): header, room records, shade records, group
/// records, then the repeater/settings/net/trans trailer the migrator skips.
/// Fields are unpadded — [`somfy_migrate::Reader`] tolerates the C++ fixed-width
/// padding (`atoi`/`_rtrim`), so the readable form decodes identically.
fn synthetic_v25_backup() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut line = |fields: &[&str]| {
        out.extend_from_slice(fields.join(",").as_bytes());
        out.push(b'\n');
    };

    // Header (writeHeader order, :47-60). Repeater size/count pair present for
    // version >= 21 only (:81-84).
    line(&[
        "25",        // version
        "76",        // header length
        "29",        // room record size
        "1",         // room record count
        "276",       // shade record size
        "1",         // shade record count
        "200",       // group record size
        "1",         // group record count
        "77",        // repeater record size (v>=21)
        "1",         // repeater record count (v>=21)
        "552",       // settings record size
        "318",       // net record size
        "78",        // trans record size
        "SrvBackup", // serverId
    ]);

    // Room record (writeRoomRecord :964-968): roomId, name, sortOrder.
    line(&["1", "Living Room", "0"]);

    // Shade record (writeShadeRecord :970-1018), 34 fields.
    line(&[
        "10",       // 0  shadeId
        "true",     // 1  paired (skip)
        "1",        // 2  shadeType -> kind_raw
        "1111111",  // 3  remoteAddress -> address
        "Blind A",  // 4  name
        "2",        // 5  tiltType -> tilt_mode_raw
        "1",        // 6  proto -> proto_raw
        "56",       // 7  bitLength
        "30000",    // 8  upTime
        "29000",    // 9  downTime
        "5000",     // 10 tiltTime
        "100",      // 11 stepSize (skip)
        "0",        // 12 linkedRemote0
        "0",        // 13 linkedRemote1
        "0",        // 14 linkedRemote2
        "0",        // 15 linkedRemote3
        "0",        // 16 linkedRemote4
        "0",        // 17 linkedRemote5
        "0",        // 18 linkedRemote6
        "41",       // 19 lastRollingCode -> next_code (+1)
        "0",        // 20 flags
        "-1.00000", // 21 myPos -> my_position_centi (-100)
        "-1.00000", // 22 myTiltPos (skip)
        "50.00000", // 23 currentPos -> position_centi (5000)
        "0.00000",  // 24 currentTiltPos -> tilt_position_centi (0)
        "false",    // 25 flipCommands (skip)
        "false",    // 26 flipPosition (skip)
        "1",        // 27 repeats (skip)
        "2",        // 28 sortOrder (skip)
        "0",        // 29 gpioUp (skip)
        "0",        // 30 gpioDown (skip)
        "0",        // 31 gpioMy (skip)
        "0",        // 32 gpioFlags (skip)
        "1",        // 33 roomId -> room_id (\n-terminated)
    ]);

    // Group record (v25 writeGroupRecord :941-957). linkedShades is 32 slots;
    // non-zero members first, then 0-padding. lastRollingCode is the trailing,
    // \n-terminated field for v24+.
    let mut group: Vec<&str> = vec![
        "3",           // groupId
        "0",           // groupType (skip)
        "9000000",     // remoteAddress -> address
        "Whole House", // name
        "1",           // proto (skip)
        "56",          // bitLength (skip)
    ];
    let members = ["10", "11"];
    for slot in 0..32 {
        group.push(members.get(slot).copied().unwrap_or("0"));
    }
    group.push("1"); // repeats (skip)
    group.push("3"); // sortOrder (skip)
    group.push("false"); // flipCommands (skip)
    group.push("2"); // roomId (skip)
    group.push("7"); // lastRollingCode -> next_code (+1)
    line(&group);

    // Trailer records present in a `backup` (:378-381) but not modeled — skipped
    // to EOF by record end.
    line(&["       0", "       0", "       0", "       0"]); // repeater (4 addrs)
    line(&["settings", "record", "data"]);
    line(&["net", "record", "data"]);
    line(&["trans", "record", "data"]);

    out
}

/// The exact [`MigrationData`] `synthetic_v25_backup` must decode to. Every field
/// is pinned, including the ones the record parsers derive (rolling-code `+1`,
/// centi-percent positions, the `-1.0` myPos sentinel as `-100`, compacted group
/// members). If any mapping drifts, this expectation fails.
fn expected_migration_data() -> MigrationData {
    MigrationData {
        version: 25,
        server_id: hstr("SrvBackup"),
        rooms: hvec(&[MigratedRoom {
            room_id: 1,
            name: hstr("Living Room"),
        }]),
        shades: hvec(&[MigratedShade {
            shade_id: 10,
            name: hstr("Blind A"),
            address: 1_111_111,
            next_code: RollingCode(42), // 41 + 1
            kind_raw: 1,
            tilt_mode_raw: 2,
            up_time_ms: 30_000,
            down_time_ms: 29_000,
            tilt_time_ms: 5_000,
            position_centi: 5_000,   // 50.00000
            tilt_position_centi: 0,  // 0.00000
            my_position_centi: -100, // -1.00000 sentinel
            room_id: 1,
            linked_addresses: hvec(&[]),
            flags_raw: 0,
            bit_length: 56,
            proto_raw: 1,
        }]),
        groups: hvec(&[MigratedGroup {
            group_id: 3,
            name: hstr("Whole House"),
            address: 9_000_000,
            next_code: RollingCode(8), // 7 + 1
            member_shade_ids: hvec(&[10, 11]),
        }]),
    }
}

#[test]
fn pipeline_locks_full_migration_data() {
    let bytes = synthetic_v25_backup();
    let parsed = parse_backup(&bytes).expect("synthetic backup parses");
    assert_eq!(parsed, expected_migration_data());
}

// ---------------------------------------------------------------------------
// Layer 2: ignored real-device golden test
// ---------------------------------------------------------------------------

/// Upper bound of the 24-bit Somfy RTS remote-address space (inclusive).
const MAX_RTS_ADDRESS: u32 = 0xFF_FFFF;

#[test]
#[ignore = "requires a real device backup — see fixtures README"]
fn real_device_backup_satisfies_structural_invariants() {
    // Read at runtime (not include_bytes!) so a missing fixture never breaks the
    // build — only this ignored test fails, and only when explicitly run.
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real_device.backup");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\n\
             Export a backup from the device (Settings -> Backup) and place it there.\n\
             See tests/fixtures/README.md.",
            path.display()
        )
    });

    let data = parse_backup(&bytes).expect("real device backup parses");

    // Version inside the supported window. parse_backup already enforces this,
    // but pinning it documents the golden contract at the assertion site.
    assert!(
        (19..=25).contains(&data.version),
        "version {} outside supported 19..=25",
        data.version
    );

    // A real device has at least one shade.
    assert!(!data.shades.is_empty(), "real backup has no shades");

    for shade in &data.shades {
        assert!(
            (1..=MAX_RTS_ADDRESS).contains(&shade.address),
            "shade {} address {:#x} outside 1..=0xFFFFFF",
            shade.shade_id,
            shade.address
        );
        assert!(
            !shade.name.is_empty(),
            "shade {} has an empty name",
            shade.shade_id
        );
    }

    // Groups are virtual remotes: same address/name invariants when present.
    for group in &data.groups {
        assert!(
            (1..=MAX_RTS_ADDRESS).contains(&group.address),
            "group {} address {:#x} outside 1..=0xFFFFFF",
            group.group_id,
            group.address
        );
        assert!(
            !group.name.is_empty(),
            "group {} has an empty name",
            group.group_id
        );
    }

    // Rooms, when present, carry non-empty names.
    for room in &data.rooms {
        assert!(
            !room.name.is_empty(),
            "room {} has an empty name",
            room.room_id
        );
    }
}
