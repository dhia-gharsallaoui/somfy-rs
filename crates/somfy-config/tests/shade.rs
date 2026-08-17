//! The persisted shade table, through its public API only.
//!
//! The tamper tests that need the record's private offsets live beside it in
//! `src/shade.rs`; everything here is reachable by a provisioning tool and by
//! the firmware, which is what makes it the contract.

use somfy_config::{ShadeError, ShadeRecord, ShadeRecordError, StoredShade, TravelField};
use somfy_config::{SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY};
use somfy_domain::{DomainError, ShadeConfig, ShadeKind, TiltMode, MAX_SHADES};
use somfy_rts::RollingCode;

/// A shade built the way a provisioning tool builds one: through the domain's
/// own constructor, then the fields it does not take.
fn shade(name: &str, address: u32, code: u16) -> StoredShade {
    let config = ShadeConfig::new(name, address).expect("a valid shade config");
    StoredShade::new(config, RollingCode(code)).expect("a valid stored shade")
}

fn record(shades: &[StoredShade]) -> ShadeRecord {
    let mut record = ShadeRecord {
        seq: 0,
        shades: heapless::Vec::new(),
    };
    for entry in shades {
        record.shades.push(entry.clone()).expect("fits");
    }
    record
}

#[test]
fn a_table_of_shades_round_trips_through_its_bytes() {
    let mut kitchen = shade("Kitchen", 0x00_1001, 1);
    kitchen.config.kind = ShadeKind::Blind;
    kitchen.config.tilt_mode = TiltMode::TiltMotor;
    kitchen.config.up_time_ms = 12_500;
    kitchen.config.down_time_ms = 13_750;
    kitchen.config.tilt_time_ms = 1_200;

    // A name a person would actually type, including the characters that are
    // unusable in a topic and perfectly good here.
    let salon = shade("Salon / Porte-fenêtre", 0x00_1002, 42_000);

    let written = record(&[kitchen, salon]);
    let decoded = ShadeRecord::decode(&written.encode());

    assert_eq!(decoded, Ok(written));
}

/// A record saying "no shades" is a value an operator can mean — every shade
/// removed — and it is not the same fact as a blank region.
#[test]
fn an_empty_table_round_trips_and_is_not_blank() {
    let empty = record(&[]);
    assert_eq!(ShadeRecord::decode(&empty.encode()), Ok(empty));
}

/// The registry's capacity is the bound that matters, so the record has to
/// carry a full one.
#[test]
fn a_full_registry_fits_in_one_record() {
    assert_eq!(SHADE_TABLE_CAPACITY, MAX_SHADES);

    let mut full = ShadeRecord {
        seq: 7,
        shades: heapless::Vec::new(),
    };
    for index in 0..MAX_SHADES {
        // Distinct addresses, and none of them a real one.
        full.shades
            .push(shade("Shade", 0x00_2000 + index as u32, index as u16 + 1))
            .expect("the record holds a full registry");
    }

    assert_eq!(ShadeRecord::decode(&full.encode()), Ok(full));
}

#[test]
fn an_erased_slot_is_blank_rather_than_damaged() {
    assert_eq!(
        ShadeRecord::decode(&[0xFF; SHADE_RECORD_LEN]),
        Err(ShadeRecordError::Blank),
    );
}

#[test]
fn foreign_bytes_are_reported_as_the_wrong_format() {
    let mut bytes = [0u8; SHADE_RECORD_LEN];
    bytes[0..4].copy_from_slice(b"RTSC");
    assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Magic));
}

#[test]
fn a_single_flipped_bit_fails_the_checksum() {
    let written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    let mut bytes = written.encode();
    bytes[64] ^= 0x01;
    assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
}

/// Every byte is covered, including the ones after the last live entry — a
/// later format cannot put a field there and have this version accept it.
#[test]
fn the_unused_tail_of_a_record_is_covered_by_the_checksum() {
    let written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    let mut bytes = written.encode();
    bytes[SHADE_RECORD_LEN - 8] ^= 0x01;
    assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
}

/// A zero travel time makes the position estimate teleport and every Step a
/// no-op — a shade that looks configured and tracks nothing. Refused at the
/// point it is entered, like every other rule in this crate.
#[test]
fn a_zero_travel_time_is_refused() {
    let mut config = ShadeConfig::new("Kitchen", 0x00_1001).expect("valid");
    config.up_time_ms = 0;
    assert_eq!(
        StoredShade::new(config.clone(), RollingCode(1)),
        Err(ShadeError::TravelTimeZero {
            field: TravelField::Up
        }),
    );

    config.up_time_ms = 10_000;
    config.down_time_ms = 0;
    assert_eq!(
        StoredShade::new(config, RollingCode(1)),
        Err(ShadeError::TravelTimeZero {
            field: TravelField::Down
        }),
    );
}

/// A tilt time of zero is allowed: no command drives the tilt axis, and a
/// shade with no tilt motor has no tilt travel to state.
#[test]
fn a_zero_tilt_time_is_accepted() {
    let mut config = ShadeConfig::new("Kitchen", 0x00_1001).expect("valid");
    config.tilt_time_ms = 0;
    assert!(StoredShade::new(config, RollingCode(1)).is_ok());
}

/// The address rules are the domain's, not this crate's, and they are reached
/// through the domain's own constructor rather than restated here.
#[test]
fn the_sentinel_addresses_are_refused() {
    for address in [0, 0x00FF_FFFF, 0x0100_0000] {
        let config = ShadeConfig::new("Kitchen", 0x00_1001).expect("valid");
        let mut broken = config.clone();
        broken.address = address;
        assert_eq!(
            StoredShade::new(broken, RollingCode(1)),
            Err(ShadeError::Domain(DomainError::InvalidAddress)),
            "address {address:#08X} must be refused",
        );
    }
}

/// Two shades at one address is a table that does not say what that address's
/// travel times are — and the registry would refuse the second one anyway,
/// silently dropping a shade the operator provisioned.
#[test]
fn two_shades_at_the_same_address_are_refused() {
    let written = record(&[shade("Kitchen", 0x00_1001, 1), shade("Salon", 0x00_1001, 2)]);
    assert_eq!(
        ShadeRecord::decode(&written.encode()),
        Err(ShadeRecordError::DuplicateAddress {
            index: 1,
            address: 0x00_1001,
        }),
    );
}

/// The streaming reader is what the firmware uses, so it has to agree with
/// `decode` about what a good record contains — and about the order, which is
/// the order shade ids follow.
#[test]
fn visiting_a_record_yields_the_same_shades_in_the_same_order() {
    let written = record(&[
        shade("Kitchen", 0x00_1001, 1),
        shade("Salon", 0x00_1002, 2),
        shade("Bureau", 0x00_1003, 3),
    ]);
    let bytes = written.encode();

    let mut visited: Vec<(usize, StoredShade)> = Vec::new();
    let header = ShadeRecord::for_each(&bytes, |index, shade| visited.push((index, shade)))
        .expect("the record decodes");

    assert_eq!(header.seq, written.seq);
    assert_eq!(header.count, 3);
    assert_eq!(visited.len(), 3);
    for (index, shade) in visited {
        assert_eq!(index, shade.config.address as usize - 0x00_1001);
        assert_eq!(shade, written.shades[index]);
    }
}

/// **The rule that keeps a bad field from renaming an installation.** The
/// registry assigns the lowest free slot, so loading the survivors of a refused
/// table would shift every id after the gap — and a shade's id is what Home
/// Assistant's entity is named after.
#[test]
fn one_refused_entry_visits_nothing_at_all() {
    let written = record(&[
        shade("Kitchen", 0x00_1001, 1),
        shade("Salon", 0x00_1002, 2),
        shade("Bureau", 0x00_1003, 3),
    ]);
    // Reach past the constructor the way a foreign writer would: a shade kind
    // this firmware does not model, in the middle entry.
    let mut bytes = written.encode();
    let entry = 12 + 56; // header, then the second entry
    bytes[entry + 6] = 0x05; // garage, which `ShadeKind::from_raw` refuses
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());

    let mut visited = 0;
    let result = ShadeRecord::for_each(&bytes, |_, _| visited += 1);

    assert_eq!(
        result,
        Err(ShadeRecordError::Kind {
            index: 1,
            raw: 0x05
        }),
    );
    assert_eq!(
        visited, 0,
        "the first entry decodes cleanly and must still not be placed",
    );
    // And the whole-record reader agrees, because it is the same walk.
    assert!(ShadeRecord::decode(&bytes).is_err());
}

/// The header reader must not decode entries — it is what the ring's scan uses
/// on every slot — but it must still refuse bytes that are not a record.
#[test]
fn the_header_reader_answers_without_decoding_shades() {
    let written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    let mut bytes = written.encode();
    bytes[8..12].copy_from_slice(&9u32.to_le_bytes());
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());

    let header = ShadeRecord::header(&bytes).expect("a whole record");
    assert_eq!(header.seq, 9);
    assert_eq!(header.count, 1);

    assert_eq!(
        ShadeRecord::header(&[0xFF; SHADE_RECORD_LEN]),
        Err(ShadeRecordError::Blank),
    );
    bytes[100] ^= 0x01;
    assert_eq!(
        ShadeRecord::header(&bytes),
        Err(ShadeRecordError::Checksum),
        "a header must not be believed on bytes whose checksum fails",
    );
}

/// The seed is carried verbatim, because it is the number a motor compares
/// against. Rounding it, defaulting it or nudging it would be the same failure
/// as re-seeding: a code at or below the motor's is rejected as a replay.
#[test]
fn the_initial_rolling_code_survives_the_round_trip_exactly() {
    for code in [0, 1, 42, u16::MAX - 1, u16::MAX] {
        let written = record(&[shade("Kitchen", 0x00_1001, code)]);
        let decoded = ShadeRecord::decode(&written.encode()).expect("decodes");
        assert_eq!(decoded.shades[0].initial_code, RollingCode(code));
    }
}

/// Equal records encode to identical bytes, which is what lets a writer prove
/// a write landed by reading it back.
#[test]
fn equal_records_encode_identically() {
    let one = record(&[shade("Kitchen", 0x00_1001, 1)]);
    let two = record(&[shade("Kitchen", 0x00_1001, 1)]);
    assert_eq!(one.encode(), two.encode());
}

/// The sequence number is what orders records around the ring, so it has to
/// survive the round trip and to be the only thing separating two otherwise
/// identical records.
#[test]
fn the_sequence_number_round_trips() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    written.seq = u32::MAX;
    let decoded = ShadeRecord::decode(&written.encode()).expect("decodes");
    assert_eq!(decoded.seq, u32::MAX);
}
