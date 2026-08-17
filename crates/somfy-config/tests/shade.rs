//! The persisted shade table, through its public API only.
//!
//! The tamper tests that need the record's private offsets live beside it in
//! `src/shade.rs`; everything here is reachable by a provisioning tool and by
//! the firmware, which is what makes it the contract.

use somfy_config::{
    Announced, LinkedRemote, ShadeError, ShadeRecord, ShadeRecordError, StoredShade, TravelField,
};
use somfy_config::{MAX_LINKED_REMOTES, MAX_LINKS, SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY};
use somfy_domain::{
    DomainError, FrameWidth, RadioProtocol, ShadeConfig, ShadeId, ShadeKind, TiltMode, MAX_SHADES,
};
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
        announced: Announced::NONE,
        links: heapless::Vec::new(),
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
        announced: Announced::NONE,
        links: heapless::Vec::new(),
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
    let entry = 20 + 56; // header, then the second entry
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

// ---------------------------------------------------------------------------
// The announced-shade bitmap
// ---------------------------------------------------------------------------

/// The bitmap is what gives `MqttConfig::retire_shade` a caller, so it has to
/// survive the round trip exactly — and independently of the table, because the
/// case it exists for is the one where the two disagree.
#[test]
fn the_announced_set_round_trips_and_is_not_derived_from_the_table() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    // Shade 4 was announced and no longer exists: an orphan waiting to be
    // cleared, and this is the only fact that can name it.
    written.announced = Announced::NONE.with(ShadeId(0)).with(ShadeId(4));

    let decoded = ShadeRecord::decode(&written.encode()).expect("decodes");
    assert_eq!(decoded.announced, written.announced);
    assert!(decoded.announced.contains(ShadeId(4)));
    assert_eq!(decoded.shades.len(), 1);
}

/// Every id the registry has, and the ones past it. The bound is the whole
/// content of the type: a shift by an id past the word is undefined where it is
/// not a panic.
#[test]
fn the_bitmap_holds_every_registry_id_and_ignores_the_ones_past_it() {
    let mut set = Announced::NONE;
    for id in 0..SHADE_TABLE_CAPACITY as u8 {
        set = set.with(ShadeId(id));
        assert!(set.contains(ShadeId(id)));
    }
    assert_eq!(set.ids().count(), SHADE_TABLE_CAPACITY);

    for id in [SHADE_TABLE_CAPACITY as u8, 200, u8::MAX] {
        assert!(!set.contains(ShadeId(id)));
        assert_eq!(set.with(ShadeId(id)), set);
        assert_eq!(set.without(ShadeId(id)), set);
    }
}

/// A stored word with bits above the registry's capacity names slots this build
/// has no shade for and no topic to clear, so they are dropped rather than
/// carried into a claim it cannot act on.
#[test]
fn bits_above_the_registry_are_dropped_on_the_way_in() {
    let set = Announced::from_bits(u32::MAX);
    assert_eq!(set.ids().count(), SHADE_TABLE_CAPACITY);
    for id in 0..SHADE_TABLE_CAPACITY as u8 {
        assert!(set.contains(ShadeId(id)));
    }
}

/// Removing one id leaves the rest alone. A retirement clears one shade, not
/// the estate.
#[test]
fn clearing_one_id_leaves_the_others() {
    let set = Announced::NONE
        .with(ShadeId(0))
        .with(ShadeId(5))
        .with(ShadeId(31));
    let after = set.without(ShadeId(5));
    let ids: std::vec::Vec<ShadeId> = after.ids().collect();
    assert_eq!(ids, std::vec![ShadeId(0), ShadeId(31)]);
}

// ---------------------------------------------------------------------------
// Frame width and radio protocol
// ---------------------------------------------------------------------------

/// The two fields that used to be parsed, reported and dropped. A shade the old
/// controller drove another way imported looking healthy and never moved;
/// carrying them is what lets the device say so instead.
#[test]
fn the_frame_width_and_protocol_survive_the_round_trip() {
    for (width, protocol) in [
        (FrameWidth::Bits56, RadioProtocol::Rts),
        (FrameWidth::Bits80, RadioProtocol::Rts),
        (FrameWidth::Bits56, RadioProtocol::Rtw),
        (FrameWidth::Bits80, RadioProtocol::GpRemote),
    ] {
        let mut entry = shade("Kitchen", 0x00_1001, 1);
        entry.config.frame_width = width;
        entry.config.protocol = protocol;
        let decoded = ShadeRecord::decode(&record(&[entry]).encode()).expect("decodes");
        assert_eq!(decoded.shades[0].config.frame_width, width);
        assert_eq!(decoded.shades[0].config.protocol, protocol);
    }
}

/// A width that is neither of the protocol's two is reported, not defaulted to
/// 56 — a motor paired at the other width is deaf to every frame the
/// substitution would produce.
#[test]
fn a_frame_width_that_is_not_a_frame_width_is_reported() {
    let mut bytes = record(&[shade("Kitchen", 0x00_1001, 1)]).encode();
    bytes[20 + 21] = 64;
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::Width { index: 0, raw: 64 }),
    );
}

/// And the same for a protocol byte outside the set.
#[test]
fn a_protocol_byte_outside_the_set_is_reported() {
    let mut bytes = record(&[shade("Kitchen", 0x00_1001, 1)]).encode();
    bytes[20 + 22] = 0x7F;
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::Protocol {
            index: 0,
            raw: 0x7F
        }),
    );
}

// ---------------------------------------------------------------------------
// Linked remotes
// ---------------------------------------------------------------------------

/// The pool round-trips, and the links reach the shades they belong to.
///
/// RTS is one-way: a wall remote's press is the only event that can put a
/// position estimate back on the truth, and it can only do that if the
/// remote's address survives a reboot.
#[test]
fn linked_remotes_round_trip_and_name_their_shade() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1), shade("Salon", 0x00_1002, 2)]);
    for link in [
        LinkedRemote {
            shade: ShadeId(0),
            address: 0x00_2001,
        },
        LinkedRemote {
            shade: ShadeId(1),
            address: 0x00_2002,
        },
        LinkedRemote {
            shade: ShadeId(1),
            address: 0x00_2003,
        },
    ] {
        written.links.push(link).expect("fits");
    }

    let bytes = written.encode();
    assert_eq!(ShadeRecord::decode(&bytes), Ok(written.clone()));

    let mut visited: std::vec::Vec<LinkedRemote> = std::vec::Vec::new();
    let header = ShadeRecord::for_each_link(&bytes, |link| visited.push(link)).expect("decodes");
    assert_eq!(header.links, 3);
    assert_eq!(visited.as_slice(), written.links.as_slice());
}

/// The pool is the whole record's, not one shade's, and it is exactly what is
/// left over after the table. The figure is asserted so it cannot drift from
/// the layout that produced it.
#[test]
fn the_pool_is_what_is_left_of_the_record() {
    // Was 58 before the version-4 calibration block took 128 bytes of it. The
    // figure moves with the layout, which is the point of asserting it here.
    assert_eq!(MAX_LINKS, 26);
    assert_eq!(MAX_LINKED_REMOTES, 7);
    // The reason the bound is shared, stated as the arithmetic rather than as
    // a claim: seven remotes on every one of thirty-two shades is more links
    // than a 2048-byte slot has room for, whatever else it gives up.
    assert_eq!(SHADE_TABLE_CAPACITY * MAX_LINKED_REMOTES, 224);
}

/// A full pool fits, and a full pool is still a record the device reads.
#[test]
fn a_full_pool_round_trips() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    // One shade would break the per-shade bound long before the pool filled,
    // so the links are spread across a table wide enough to hold them.
    written.shades.clear();
    let shades_needed = MAX_LINKS.div_ceil(MAX_LINKED_REMOTES);
    for index in 0..shades_needed {
        written
            .shades
            .push(shade("S", 0x00_1001 + index as u32, 1))
            .expect("fits");
    }
    let mut address = 0x00_5000u32;
    'fill: for index in 0..shades_needed {
        for _ in 0..MAX_LINKED_REMOTES {
            if written
                .links
                .push(LinkedRemote {
                    shade: ShadeId(index as u8),
                    address,
                })
                .is_err()
            {
                break 'fill;
            }
            address += 1;
        }
    }
    assert_eq!(written.links.len(), MAX_LINKS);
    assert_eq!(ShadeRecord::decode(&written.encode()), Ok(written));
}

/// A link naming a row the record does not have is reported, not skipped:
/// the remote belongs to *some* shade, and guessing which would attach a wall
/// remote to the wrong motor's estimate.
#[test]
fn a_link_naming_a_missing_shade_is_reported() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    written
        .links
        .push(LinkedRemote {
            shade: ShadeId(4),
            address: 0x00_2001,
        })
        .expect("fits");
    assert_eq!(
        ShadeRecord::decode(&written.encode()),
        Err(ShadeRecordError::LinkShade { index: 0, shade: 4 }),
    );
}

/// A shade's own address is not a link to it. The domain refuses it as a
/// duplicate, so the record does too — otherwise flash could deliver a table
/// the registry then rejects one shade at a time.
#[test]
fn a_link_at_the_shades_own_address_is_reported() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    written
        .links
        .push(LinkedRemote {
            shade: ShadeId(0),
            address: 0x00_1001,
        })
        .expect("fits");
    assert_eq!(
        ShadeRecord::decode(&written.encode()),
        Err(ShadeRecordError::Link {
            index: 0,
            error: DomainError::DuplicateAddress,
        }),
    );
}

/// The same remote twice on one shade. Harmless in itself and refused anyway,
/// because the domain refuses it and the two must not disagree.
#[test]
fn the_same_remote_linked_twice_to_one_shade_is_reported() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    for _ in 0..2 {
        written
            .links
            .push(LinkedRemote {
                shade: ShadeId(0),
                address: 0x00_2001,
            })
            .expect("fits");
    }
    assert_eq!(
        ShadeRecord::decode(&written.encode()),
        Err(ShadeRecordError::Link {
            index: 1,
            error: DomainError::DuplicateAddress,
        }),
    );
}

/// One remote may drive two shades — a wall switch that runs a pair of
/// blinds — so the same address on two different rows is not a duplicate.
#[test]
fn one_remote_may_drive_two_shades() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1), shade("Salon", 0x00_1002, 2)]);
    for row in [0u8, 1] {
        written
            .links
            .push(LinkedRemote {
                shade: ShadeId(row),
                address: 0x00_2001,
            })
            .expect("fits");
    }
    assert_eq!(ShadeRecord::decode(&written.encode()), Ok(written));
}

/// An eighth remote on one shade is past the domain's own bound, so the record
/// refuses it rather than handing the registry a link it will drop.
#[test]
fn more_than_seven_remotes_on_one_shade_is_reported() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    for index in 0..=MAX_LINKED_REMOTES {
        written
            .links
            .push(LinkedRemote {
                shade: ShadeId(0),
                address: 0x00_2001 + index as u32,
            })
            .expect("fits");
    }
    assert_eq!(
        ShadeRecord::decode(&written.encode()),
        Err(ShadeRecordError::Link {
            index: MAX_LINKED_REMOTES,
            error: DomainError::RegistryFull,
        }),
    );
}

/// A sentinel address is not a remote.
#[test]
fn a_sentinel_link_address_is_reported() {
    let mut bytes = record(&[shade("Kitchen", 0x00_1001, 1)]).encode();
    // One live pool word, and the word itself all zero — which is what a
    // sentinel address looks like once the row is row 0.
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::Link {
            index: 0,
            error: DomainError::InvalidAddress,
        }),
    );
}

/// A header claiming more links than the pool holds is refused before any word
/// is read, so a corrupt count cannot walk off the end of the record.
#[test]
fn a_link_count_past_the_pool_is_reported() {
    let mut bytes = record(&[shade("Kitchen", 0x00_1001, 1)]).encode();
    let claimed = MAX_LINKS as u16 + 1;
    bytes[16..18].copy_from_slice(&claimed.to_le_bytes());
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::LinkCount(claimed)),
    );
}

/// All or nothing covers the links too: a table whose entries are all good and
/// whose pool is not places **no** shades, so a caller cannot end up with a
/// half-linked installation it then has to undo.
#[test]
fn one_bad_link_visits_no_shades_at_all() {
    let mut written = record(&[shade("Kitchen", 0x00_1001, 1)]);
    written
        .links
        .push(LinkedRemote {
            shade: ShadeId(9),
            address: 0x00_2001,
        })
        .expect("fits");
    let bytes = written.encode();

    let mut visited = 0;
    assert!(ShadeRecord::for_each(&bytes, |_, _| visited += 1).is_err());
    assert_eq!(visited, 0, "a bad link must not leave shades placed");
}
