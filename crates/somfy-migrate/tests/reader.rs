use heapless::String;
use somfy_migrate::{MigrateError, Reader};

// ---- Brief-specified tests (Step 2) ----

#[test]
fn reads_comma_separated_values() {
    let mut r = Reader::new(b"25,76,29\n");
    assert_eq!(r.read_u8().unwrap(), 25);
    assert_eq!(r.read_u8().unwrap(), 76);
    assert_eq!(r.read_u8().unwrap(), 29);
}

#[test]
fn read_str_rtrims_padding() {
    // C++ pads fixed-size string fields with spaces; _rtrim strips them.
    let mut r = Reader::new(b"Kitchen   ,next\n");
    let mut s: String<64> = String::new();
    r.read_str(&mut s).unwrap();
    assert_eq!(s.as_str(), "Kitchen");
    let mut n: String<64> = String::new();
    r.read_str(&mut n).unwrap();
    assert_eq!(n.as_str(), "next");
}

#[test]
fn read_var_str_honors_quotes() {
    // quoted strings may contain commas (C++ readVarString / CFG_TOK_QUOTE)
    let mut r = Reader::new(b"\"Living, room\",42\n");
    let mut s: String<64> = String::new();
    r.read_var_str(&mut s).unwrap();
    assert_eq!(s.as_str(), "Living, room");
    assert_eq!(r.read_u8().unwrap(), 42);
}

#[test]
fn read_float_as_centi_integer() {
    let mut r = Reader::new(b"42.50,-1.00,7,0.5\n");
    assert_eq!(r.read_f32_as_centi().unwrap(), 4250);
    assert_eq!(r.read_f32_as_centi().unwrap(), -100);
    assert_eq!(r.read_f32_as_centi().unwrap(), 700); // no frac part
    assert_eq!(r.read_f32_as_centi().unwrap(), 50); // one frac digit
}

#[test]
fn empty_numeric_field_defaults_to_zero() {
    let mut r = Reader::new(b",5\n");
    assert_eq!(r.read_u8().unwrap(), 0);
    assert_eq!(r.read_u8().unwrap(), 5);
}

#[test]
fn skip_record_end_advances_past_newline() {
    let mut r = Reader::new(b"1,junk,junk\n2\n");
    assert_eq!(r.read_u8().unwrap(), 1);
    r.skip_record_end().unwrap();
    assert_eq!(r.read_u8().unwrap(), 2);
}

#[test]
fn eof_is_an_error_not_a_panic() {
    let mut r = Reader::new(b"");
    assert!(matches!(r.read_u8(), Err(MigrateError::UnexpectedEof)));
}

// ---- Added: real-file tolerance & documented C++ divergences ----

#[test]
fn numeric_fields_tolerate_leading_space_padding() {
    // writeUInt16 uses "%5u" and writeUInt8 "%3u" => right-justified with
    // leading spaces (ConfigFile.cpp:223,228). atoi skips leading whitespace.
    let mut r = Reader::new(b"   42,  5\n");
    assert_eq!(r.read_u16().unwrap(), 42);
    assert_eq!(r.read_u8().unwrap(), 5);
}

#[test]
fn read_f32_tolerates_prec5_position_output() {
    // Positions are written with writeFloat(pos, 5) => "%12.5f"
    // (ConfigFile.cpp:236-239, 995-998). Extra fraction digits are truncated to
    // centi precision.
    let mut r = Reader::new(b"    42.50000,    -1.00000,     0.00000\n");
    assert_eq!(r.read_f32_as_centi().unwrap(), 4250);
    assert_eq!(r.read_f32_as_centi().unwrap(), -100);
    assert_eq!(r.read_f32_as_centi().unwrap(), 0);
}

#[test]
fn read_f32_truncates_beyond_two_fraction_digits() {
    // "33.33333" -> 33.33 -> 3333 (truncation, not rounding).
    let mut r = Reader::new(b"33.33333,99.999\n");
    assert_eq!(r.read_f32_as_centi().unwrap(), 3333);
    assert_eq!(r.read_f32_as_centi().unwrap(), 9999);
}

#[test]
fn read_bool_accepts_true_false_and_digit_forms() {
    // C++ writeBool emits "true"/"false" (ConfigFile.cpp:242), NOT "1"/"0" as
    // the brief claimed; readBool checks the first byte for t/T/1
    // (ConfigFile.cpp:282-288).
    let mut r = Reader::new(b"true,false,1,0\n");
    assert!(r.read_bool().unwrap());
    assert!(!r.read_bool().unwrap());
    assert!(r.read_bool().unwrap());
    assert!(!r.read_bool().unwrap());
}

#[test]
fn read_i8_handles_negative_values() {
    let mut r = Reader::new(b"-5,-1,127\n");
    assert_eq!(r.read_i8().unwrap(), -5);
    assert_eq!(r.read_i8().unwrap(), -1);
    assert_eq!(r.read_i8().unwrap(), 127);
}

#[test]
fn unsigned_reads_wrap_via_truncating_cast() {
    // atoi("-1") = -1; static_cast<uint8_t>(-1) = 255. Rust `as u8` matches.
    let mut r = Reader::new(b"-1,300\n");
    assert_eq!(r.read_u8().unwrap(), 255);
    assert_eq!(r.read_u8().unwrap(), 44); // 300 & 0xFF
}

#[test]
fn read_var_str_unquoted_includes_embedded_commas() {
    // DIVERGENCE FROM BRIEF: an unquoted readVarString does NOT stop at commas;
    // with quotes < 2 the comma is stored as content and the field ends only at
    // '\n'/EOF (ConfigFile.cpp:162-167, 168-169). Faithful port of the C++.
    let mut r = Reader::new(b"Living, room\n7\n");
    let mut s: String<64> = String::new();
    r.read_var_str(&mut s).unwrap();
    assert_eq!(s.as_str(), "Living, room");
    assert_eq!(r.read_u8().unwrap(), 7);
}

#[test]
fn read_str_reports_overlong_field() {
    // 65 'a's exceeds String<64>. C++ silently truncates; the migrator errors.
    let long = [b'a'; 65];
    let mut r = Reader::new(&long);
    let mut s: String<64> = String::new();
    assert!(matches!(
        r.read_str(&mut s),
        Err(MigrateError::StringTooLong)
    ));
}

#[test]
fn at_end_tracks_cursor() {
    let mut r = Reader::new(b"1,2\n");
    assert!(!r.at_end());
    r.read_u8().unwrap();
    r.read_u8().unwrap();
    assert!(r.at_end());
}

// ---- Added: record-boundary resync (port of seekChar defensive net) ----

#[test]
fn at_record_boundary_after_record_end() {
    // At start (pos 0) and after consuming a '\n' the cursor is on a boundary;
    // after consuming a ',' (mid-record) it is not.
    let mut r = Reader::new(b"1,2\n3\n");
    assert!(r.at_record_boundary()); // pos 0
    r.read_u8().unwrap(); // consumes '1' + ','
    assert!(!r.at_record_boundary()); // mid-record (last terminator was ',')
    r.read_u8().unwrap(); // consumes '2' + '\n'
    assert!(r.at_record_boundary()); // record end consumed
}

#[test]
fn resync_record_skips_unparsed_trailing_fields() {
    // A record with fields beyond what the caller read: resync advances to the
    // next record without the caller knowing the field count.
    let mut r = Reader::new(b"1,extra,extra\n2\n");
    assert_eq!(r.read_u8().unwrap(), 1); // only the first field is modeled
    r.resync_record().unwrap(); // skip the two trailing fields + '\n'
    assert_eq!(r.read_u8().unwrap(), 2);
}

#[test]
fn resync_record_is_noop_when_already_aligned() {
    // When the last read consumed the record end, resync must NOT consume the
    // following record (an unconditional skip_record_end would).
    let mut r = Reader::new(b"1\n2\n");
    assert_eq!(r.read_u8().unwrap(), 1); // consumes '1' + '\n' -> aligned
    r.resync_record().unwrap(); // no-op
    assert_eq!(r.read_u8().unwrap(), 2); // second record intact
}

#[test]
fn resync_record_at_eof_is_ok() {
    let mut r = Reader::new(b"1\n");
    assert_eq!(r.read_u8().unwrap(), 1);
    r.resync_record().unwrap();
    assert!(r.at_end());
}
