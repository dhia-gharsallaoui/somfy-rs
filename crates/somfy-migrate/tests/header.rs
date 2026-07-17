//! Header parser tests.
//!
//! Fixtures reproduce the exact byte layout the C++ `ConfigFile::writeHeader`
//! emits (`src/ConfigFile.cpp:45-62`), cross-checked against the version gates
//! in `readHeader` (`:63-93`). The modern (v21+) layout carries the
//! `repeaterRecordSize`/`repeaterRecords` pair; v19/v20 backups were written
//! before that pair existed (`readHeader` gates it at `version >= 21`, `:81-84`)
//! and therefore omit it. `serverId` is the final field and is terminated by
//! the record end (`\n`), not a separator (`writeString(..., CFG_REC_END)`,
//! `:60`).

use heapless::String;
use somfy_migrate::{parse_header, MigrateError, Reader};

// A modern (v25) header as writeHeader emits it (ConfigFile.cpp:45-62):
// version,length,roomSize,roomRecs,shadeSize,shadeRecs,groupSize,groupRecs,
// repeaterSize,repeaterRecs,settingsSize,netSize,transSize,serverId
const V25: &[u8] = b"25,76,29,2,276,3,200,1,77,0,552,318,78,MyServer1\n";

#[test]
fn parses_v25_header() {
    let mut r = Reader::new(V25);
    let h = parse_header(&mut r).unwrap();
    assert_eq!(h.version, 25);
    assert_eq!(h.length, 76);
    assert_eq!(h.room_record_size, 29);
    assert_eq!(h.room_records, 2);
    assert_eq!(h.shade_record_size, 276);
    assert_eq!(h.shade_records, 3);
    assert_eq!(h.group_record_size, 200);
    assert_eq!(h.group_records, 1);
    assert_eq!(h.repeater_record_size, 77);
    assert_eq!(h.repeater_records, 0);
    assert_eq!(h.settings_record_size, 552);
    assert_eq!(h.net_record_size, 318);
    assert_eq!(h.trans_record_size, 78);
    assert_eq!(h.server_id.as_str(), "MyServer1");
}

#[test]
fn v19_header_without_repeater_fields() {
    // v19-20 layout: no repeaterSize/repeaterRecs pair (readHeader :81-84 gates
    // the repeater reads on version >= 21).
    let v19: &[u8] = b"19,60,29,1,276,2,200,0,552,318,78,SrvXYZ\n";
    let mut r = Reader::new(v19);
    let h = parse_header(&mut r).unwrap();
    assert_eq!(h.version, 19);
    assert_eq!(h.room_records, 1);
    assert_eq!(h.shade_records, 2);
    assert_eq!(h.group_records, 0);
    // Repeater fields absent in the wire format -> default to 0.
    assert_eq!(h.repeater_record_size, 0);
    assert_eq!(h.repeater_records, 0);
    assert_eq!(h.settings_record_size, 552);
    assert_eq!(h.net_record_size, 318);
    assert_eq!(h.trans_record_size, 78);
    assert_eq!(h.server_id.as_str(), "SrvXYZ");
}

#[test]
fn v20_header_without_repeater_fields() {
    // v20 shares the v19 layout: still below the repeater gate.
    let v20: &[u8] = b"20,60,29,1,276,2,200,0,552,318,78,Srv20\n";
    let mut r = Reader::new(v20);
    let h = parse_header(&mut r).unwrap();
    assert_eq!(h.version, 20);
    assert_eq!(h.repeater_record_size, 0);
    assert_eq!(h.repeater_records, 0);
    assert_eq!(h.settings_record_size, 552);
    assert_eq!(h.server_id.as_str(), "Srv20");
}

#[test]
fn old_versions_are_rejected() {
    let mut r = Reader::new(b"13,40,276,2\n");
    assert!(matches!(
        parse_header(&mut r),
        Err(MigrateError::UnsupportedVersion(13))
    ));
}

#[test]
fn version_18_is_rejected() {
    // Floor is v19; the version just below it is still rejected.
    let mut r = Reader::new(b"18,60,29,1,276,2,200,0,552,318,78,Srv18\n");
    assert!(matches!(
        parse_header(&mut r),
        Err(MigrateError::UnsupportedVersion(18))
    ));
}

#[test]
fn version_26_ceiling_is_rejected() {
    // Ceiling is v25 (SHADE_HDR_VER); a future v26 could append fields and
    // silently misalign the record parsers, so it is rejected up front.
    let mut r = Reader::new(b"26,80,29,1,276,2,200,0,77,0,552,318,78,Srv26\n");
    assert!(matches!(
        parse_header(&mut r),
        Err(MigrateError::UnsupportedVersion(26))
    ));
}

#[test]
fn truncated_header_is_eof() {
    // Header ends before serverId is read.
    let mut r = Reader::new(b"25,76,29,2,276,3,200,1,77,0");
    assert_eq!(parse_header(&mut r), Err(MigrateError::UnexpectedEof));
}

#[test]
fn server_id_too_long_errors() {
    // serverId buffer is char[10] in C++; an over-long value must error rather
    // than silently truncate (StringTooLong divergence policy).
    let mut r = Reader::new(b"25,76,29,2,276,3,200,1,77,0,552,318,78,ThisNameIsWayTooLong\n");
    assert_eq!(parse_header(&mut r), Err(MigrateError::StringTooLong));
}

#[test]
fn server_id_padding_is_rtrimmed() {
    // Fixed-width serverId field arrives space-padded; read_str/_rtrim strip it.
    let mut r = Reader::new(b"25,76,29,2,276,3,200,1,77,0,552,318,78,Srv       \n");
    let h = parse_header(&mut r).unwrap();
    assert_eq!(h.server_id.as_str(), "Srv");
}

#[test]
fn cursor_positioned_after_header() {
    // After the header line the cursor should sit at the next record.
    let mut buf: String<64> = String::new();
    buf.push_str("25,76,29,2,276,3,200,1,77,0,552,318,78,Srv\n")
        .unwrap();
    buf.push_str("nextrecord").unwrap();
    let mut r = Reader::new(buf.as_bytes());
    parse_header(&mut r).unwrap();
    let mut s: String<64> = String::new();
    r.read_str(&mut s).unwrap();
    assert_eq!(s.as_str(), "nextrecord");
}
