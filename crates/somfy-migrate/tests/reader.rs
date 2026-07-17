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
fn empty_numeric_field_defaults_to_zero_like_cpp() {
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
fn read_bool_matches_cpp_writebool_format() {
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
fn unsigned_reads_wrap_like_cpp_static_cast() {
    // atoi("-1") = -1; static_cast<uint8_t>(-1) = 255. Rust `as u8` matches.
    let mut r = Reader::new(b"-1,300\n");
    assert_eq!(r.read_u8().unwrap(), 255);
    assert_eq!(r.read_u8().unwrap(), 44); // 300 & 0xFF
}

#[test]
fn read_var_str_unquoted_absorbs_commas_like_cpp() {
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
