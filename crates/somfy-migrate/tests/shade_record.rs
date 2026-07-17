//! Shade-record parser tests.
//!
//! Fixtures follow the exact `writeShadeRecord` field order
//! (`src/ConfigFile.cpp:970-1018`); each field is annotated by index in
//! [`base_fields`]. The C++ writer emits fixed-width, space-padded values, but
//! [`somfy_migrate::Reader`] tolerates padding (`atoi`/`_rtrim`), so the readable
//! fixtures omit it — [`parses_real_fixed_width_record`] covers the padded form
//! and cross-checks the 276-byte `SHADE_REC_SIZE` (`ConfigFile.cpp:12`).

use somfy_migrate::{parse_shade_record, BackupHeader, MigrateError, MigratedShade, Reader};
use somfy_rts::RollingCode;

// Field indices into base_fields(), mirroring writeShadeRecord order.
const I_NAME: usize = 4;
const I_LR0: usize = 12; // first of 7 linked-remote address slots (12..=18)
const I_ROLLING: usize = 19;
const I_MYPOS: usize = 21;
const I_CURPOS: usize = 23;

/// Canonical v19+ shade record (34 comma-separated fields, `\n`-terminated).
fn base_fields() -> Vec<&'static str> {
    vec![
        "3",           // 0  shadeId       -> shade_id
        "true",        // 1  paired        (skip)
        "1",           // 2  shadeType     -> kind_raw (blind)
        "1234567",     // 3  remoteAddress -> address
        "Living Room", // 4  name
        "2",           // 5  tiltType      -> tilt_mode_raw (integrated)
        "1",           // 6  proto         -> proto_raw
        "56",          // 7  bitLength     -> bit_length
        "30000",       // 8  upTime        -> up_time_ms
        "29000",       // 9  downTime      -> down_time_ms
        "5000",        // 10 tiltTime      -> tilt_time_ms
        "100",         // 11 stepSize      (skip)
        "111",         // 12 linkedRemote0 -> linked_addresses[0]
        "222",         // 13 linkedRemote1 -> linked_addresses[1]
        "0",           // 14 linkedRemote2 (empty)
        "0",           // 15 linkedRemote3 (empty)
        "0",           // 16 linkedRemote4 (empty)
        "0",           // 17 linkedRemote5 (empty)
        "0",           // 18 linkedRemote6 (empty)
        "41",          // 19 lastRollingCode -> next_code = 42
        "7",           // 20 flags         -> flags_raw
        "42.50000",    // 21 myPos         -> my_position_centi = 4250
        "10.00000",    // 22 myTiltPos     (skip)
        "55.25000",    // 23 currentPos    -> position_centi = 5525
        "0.00000",     // 24 currentTiltPos-> tilt_position_centi = 0
        "false",       // 25 flipCommands  (skip)
        "false",       // 26 flipPosition  (skip)
        "1",           // 27 repeats       (skip)
        "2",           // 28 sortOrder     (skip)
        "0",           // 29 gpioUp        (skip)
        "0",           // 30 gpioDown      (skip)
        "0",           // 31 gpioMy        (skip)
        "0",           // 32 gpioFlags     (skip)
        "4",           // 33 roomId        -> room_id
    ]
}

/// A modern header; only `version` matters to the shade parser.
fn header(version: u8) -> BackupHeader {
    BackupHeader {
        version,
        length: 76,
        room_record_size: 29,
        room_records: 0,
        shade_record_size: 276,
        shade_records: 1,
        group_record_size: 200,
        group_records: 0,
        repeater_record_size: 77,
        repeater_records: 0,
        settings_record_size: 552,
        net_record_size: 318,
        trans_record_size: 78,
        server_id: heapless::String::new(),
    }
}

fn encode(fields: &[&str]) -> Vec<u8> {
    let mut out = fields.join(",").into_bytes();
    out.push(b'\n');
    out
}

fn parse(fields: &[&str]) -> MigratedShade {
    let bytes = encode(fields);
    let mut r = Reader::new(&bytes);
    parse_shade_record(&mut r, &header(25)).expect("record parses")
}

// --- Contract tests (locked by the brief) --------------------------------

#[test]
fn rolling_code_is_stored_plus_one() {
    // The C++ file stores the LAST-SENT code; somfy-rs holds NEXT-to-send.
    let shade = parse(&base_fields()); // lastRollingCode = 41
    assert_eq!(shade.next_code, RollingCode(42));
}

#[test]
fn rolling_code_wraps_at_max() {
    let mut f = base_fields();
    f[I_ROLLING] = "65535";
    let shade = parse(&f);
    assert_eq!(shade.next_code, RollingCode(0));
}

#[test]
fn rolling_code_zero_becomes_one() {
    // A never-transmitted shade (lastRollingCode 0) must start at 1, not replay 0.
    let mut f = base_fields();
    f[I_ROLLING] = "0";
    assert_eq!(parse(&f).next_code, RollingCode(1));
}

#[test]
fn positions_arrive_as_centi_percent() {
    let shade = parse(&base_fields());
    assert_eq!(shade.position_centi, 5525); // currentPos 55.25000
    assert_eq!(shade.my_position_centi, 4250); // myPos 42.50000
    assert_eq!(shade.tilt_position_centi, 0); // currentTiltPos 0.00000
}

#[test]
fn unset_my_position_is_minus_100_centi() {
    // C++ myPos sentinel -1.0 -> -100 centi.
    let mut f = base_fields();
    f[I_MYPOS] = "-1.00000";
    assert_eq!(parse(&f).my_position_centi, -100);
}

#[test]
fn linked_remotes_parse_with_addresses() {
    // Two populated slots (111, 222), five empty -> only the non-zero addresses.
    let shade = parse(&base_fields());
    assert_eq!(shade.linked_addresses.as_slice(), &[111, 222]);
}

#[test]
fn cursor_stays_aligned_for_next_record() {
    // Two consecutive records must both parse; the second proves the first read
    // consumed exactly its record (roomId's \n realigns the cursor).
    let first = base_fields();
    let mut second = base_fields();
    second[0] = "9"; // distinct shadeId
    second[3] = "7654321"; // distinct address

    let mut bytes = encode(&first);
    bytes.extend_from_slice(&encode(&second));

    let mut r = Reader::new(&bytes);
    let h = header(25);
    let a = parse_shade_record(&mut r, &h).unwrap();
    let b = parse_shade_record(&mut r, &h).unwrap();

    assert_eq!(a.shade_id, 3);
    assert_eq!(b.shade_id, 9);
    assert_eq!(b.address, 7654321);
    assert!(r.at_end(), "cursor should rest at end after both records");
}

// --- Full field mapping ---------------------------------------------------

#[test]
fn every_modeled_field_maps() {
    let shade = parse(&base_fields());
    assert_eq!(shade.shade_id, 3);
    assert_eq!(shade.name.as_str(), "Living Room");
    assert_eq!(shade.address, 1234567);
    assert_eq!(shade.next_code, RollingCode(42));
    assert_eq!(shade.kind_raw, 1);
    assert_eq!(shade.tilt_mode_raw, 2);
    assert_eq!(shade.up_time_ms, 30000);
    assert_eq!(shade.down_time_ms, 29000);
    assert_eq!(shade.tilt_time_ms, 5000);
    assert_eq!(shade.position_centi, 5525);
    assert_eq!(shade.tilt_position_centi, 0);
    assert_eq!(shade.my_position_centi, 4250);
    assert_eq!(shade.room_id, 4);
    assert_eq!(shade.linked_addresses.as_slice(), &[111, 222]);
    assert_eq!(shade.flags_raw, 7);
    assert_eq!(shade.bit_length, 56);
    assert_eq!(shade.proto_raw, 1);
}

// --- Linked-remote edge cases --------------------------------------------

#[test]
fn linked_remotes_all_seven_populated() {
    let mut f = base_fields();
    let addrs = ["10", "20", "30", "40", "50", "60", "70"];
    for (i, a) in addrs.iter().enumerate() {
        f[I_LR0 + i] = a;
    }
    let shade = parse(&f);
    assert_eq!(
        shade.linked_addresses.as_slice(),
        &[10, 20, 30, 40, 50, 60, 70]
    );
}

#[test]
fn linked_remotes_all_empty_yields_none() {
    let mut f = base_fields();
    for i in 0..7 {
        f[I_LR0 + i] = "0";
    }
    assert!(parse(&f).linked_addresses.is_empty());
}

#[test]
fn linked_remotes_skip_interior_empty_slots() {
    // A 0 slot between populated slots is dropped; order is preserved.
    let mut f = base_fields();
    let slots = ["0", "500", "0", "600", "0", "0", "700"];
    for (i, a) in slots.iter().enumerate() {
        f[I_LR0 + i] = a;
    }
    assert_eq!(parse(&f).linked_addresses.as_slice(), &[500, 600, 700]);
}

// --- Name handling --------------------------------------------------------

#[test]
fn name_padding_is_rtrimmed() {
    let mut f = base_fields();
    f[I_NAME] = "Den       "; // trailing fixed-width padding
    assert_eq!(parse(&f).name.as_str(), "Den");
}

// --- Version gating -------------------------------------------------------

#[test]
fn v19_and_v25_parse_identically() {
    // The shade layout is invariant across the accepted v19..=25 range (the
    // highest gate, roomId, is version >= 19), so both versions decode the same.
    let bytes = encode(&base_fields());
    let mut r19 = Reader::new(&bytes);
    let mut r25 = Reader::new(&bytes);
    let a = parse_shade_record(&mut r19, &header(19)).unwrap();
    let b = parse_shade_record(&mut r25, &header(25)).unwrap();
    assert_eq!(a, b);
}

// --- Deleted-slot form ----------------------------------------------------

#[test]
fn deleted_shade_slot_parses() {
    // writeShadeRecord (:993-1007) writes cleared values for shadeId 255:
    // flags 0 and myPos/myTiltPos -1, currentPos/currentTiltPos 0.
    let mut f = base_fields();
    f[0] = "255"; // shadeId
    f[20] = "0"; // flags
    f[I_MYPOS] = "-1.00000"; // myPos
    f[22] = "-1.00000"; // myTiltPos
    f[I_CURPOS] = "0.00000"; // currentPos
    f[24] = "0.00000"; // currentTiltPos
    let shade = parse(&f);
    assert_eq!(shade.shade_id, 255);
    assert_eq!(shade.flags_raw, 0);
    assert_eq!(shade.my_position_centi, -100);
    assert_eq!(shade.position_centi, 0);
}

// --- Truncation -----------------------------------------------------------

#[test]
fn truncated_record_is_eof() {
    // Cut the record off partway through (no room for the float block onward).
    let full = base_fields();
    let short = &full[..15];
    let bytes: Vec<u8> = short.join(",").into_bytes(); // no trailing terminator
    let mut r = Reader::new(&bytes);
    assert_eq!(
        parse_shade_record(&mut r, &header(25)),
        Err(MigrateError::UnexpectedEof)
    );
}

// --- Fixed-width fidelity -------------------------------------------------

/// Build the record exactly as C++ `writeShadeRecord` serializes it: every value
/// space-padded to the width its `write*` primitive uses (`ConfigFile.cpp`
/// `writeUInt8` `%3u`, `writeUInt16` `%5u`, `writeUInt32` `%10u`,
/// `writeBool` padded to 5, `writeFloat(_,5)` `%12.5f`, `writeString(name,21)`
/// padded to 20). The full v25 record is `SHADE_REC_SIZE` = 276 bytes.
fn padded_v25_record() -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!("{:3},", 3u32)); // shadeId
    s.push_str("true ,"); // paired (writeBool -> width 5)
    s.push_str(&format!("{:3},", 1u32)); // shadeType
    s.push_str(&format!("{:10},", 1234567u32)); // remoteAddress
    s.push_str(&format!("{:<20},", "Living Room")); // name (left-justified, width 20)
    s.push_str(&format!("{:3},", 2u32)); // tiltType
    s.push_str(&format!("{:3},", 1u32)); // proto
    s.push_str(&format!("{:3},", 56u32)); // bitLength
    s.push_str(&format!("{:10},", 30000u32)); // upTime
    s.push_str(&format!("{:10},", 29000u32)); // downTime
    s.push_str(&format!("{:10},", 5000u32)); // tiltTime
    s.push_str(&format!("{:5},", 100u32)); // stepSize
    for a in [111u32, 222, 0, 0, 0, 0, 0] {
        s.push_str(&format!("{:10},", a)); // linkedRemotes
    }
    s.push_str(&format!("{:5},", 41u32)); // lastRollingCode
    s.push_str(&format!("{:3},", 7u32)); // flags
    s.push_str(&format!("{:12.5},", 42.5f64)); // myPos
    s.push_str(&format!("{:12.5},", 10.0f64)); // myTiltPos
    s.push_str(&format!("{:12.5},", 55.25f64)); // currentPos
    s.push_str(&format!("{:12.5},", 0.0f64)); // currentTiltPos
    s.push_str("false,false,"); // flipCommands, flipPosition (width 5)
    s.push_str(&format!("{:3},", 1u32)); // repeats
    s.push_str(&format!("{:3},", 2u32)); // sortOrder
    s.push_str(&format!("{:3},{:3},{:3},{:3},", 0, 0, 0, 0)); // gpioUp/Down/My/Flags
    s.push_str(&format!("{:3}\n", 4u32)); // roomId + CFG_REC_END
    s.into_bytes()
}

#[test]
fn parses_real_fixed_width_record() {
    let bytes = padded_v25_record();
    assert_eq!(bytes.len(), 276, "must equal C++ SHADE_REC_SIZE");

    let mut r = Reader::new(&bytes);
    let shade = parse_shade_record(&mut r, &header(25)).unwrap();

    // Padding must not corrupt any value.
    assert_eq!(shade.shade_id, 3);
    assert_eq!(shade.name.as_str(), "Living Room");
    assert_eq!(shade.address, 1234567);
    assert_eq!(shade.next_code, RollingCode(42));
    assert_eq!(shade.my_position_centi, 4250);
    assert_eq!(shade.position_centi, 5525);
    assert_eq!(shade.linked_addresses.as_slice(), &[111, 222]);
    assert_eq!(shade.room_id, 4);
    assert!(r.at_end(), "the 276-byte record consumes exactly to EOF");
}
