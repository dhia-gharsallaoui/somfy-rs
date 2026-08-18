//! Tests for [`super`]: what an import carries across, the three things it
//! reports rather than applies silently, and every refusal.
//!
//! A separate file rather than an inline `#[cfg(test)] mod`, for one reason:
//! together they run past what this project keeps in a single file. `#[path]`
//! in `import.rs` attaches it. It is a module of the example, not an example of
//! its own — cargo discovers examples as `examples/*.rs` and
//! `examples/*/main.rs`, so a second file in this directory is never built as a
//! binary.

use super::*;
use somfy_config::Announced;
use somfy_migrate::MigratedShade;
use somfy_rts::RollingCode;

/// A shade as the backup parser hands it over, with everything valid. Tests
/// vary one field from this so what each is about is the line that differs.
fn migrated(name: &str, address: u32) -> MigratedShade {
    MigratedShade {
        shade_id: 7,
        name: hstr(name),
        address,
        next_code: RollingCode(42),
        kind_raw: ShadeKind::Blind as u8,
        tilt_mode_raw: TiltMode::Integrated as u8,
        up_time_ms: 30_000,
        down_time_ms: 29_000,
        tilt_time_ms: 5_000,
        position_centi: 0,
        tilt_position_centi: 0,
        my_position_centi: -100,
        room_id: 1,
        linked_addresses: heapless::Vec::new(),
        flags_raw: 0,
        // The width and protocol this controller speaks, so no test warns
        // about them by accident and the ones that do say so on their own
        // lines.
        bit_length: 56,
        proto_raw: TRANSMITTED_PROTOCOL,
    }
}

fn hstr<const N: usize>(text: &str) -> heapless::String<N> {
    let mut out = heapless::String::new();
    out.push_str(text).expect("fixture string fits");
    out
}

fn hvec<T: Clone, const N: usize>(items: &[T]) -> heapless::Vec<T, N> {
    let mut out = heapless::Vec::new();
    out.extend_from_slice(items).expect("fixture vec fits");
    out
}

/// Backup data carrying `shades` and nothing else.
fn data(shades: &[MigratedShade]) -> MigrationData {
    MigrationData {
        version: 25,
        server_id: hstr("Fixture"),
        rooms: heapless::Vec::new(),
        shades: hvec(shades),
        groups: heapless::Vec::new(),
        skipped_resyncs: 0,
    }
}

// -- what the import carries across --------------------------------------

/// The one field that cannot be corrected after the fact. The parser has
/// already turned the stored last-sent code into the next-to-send one; this
/// pins that nothing here touches it again.
#[test]
fn the_next_code_is_carried_across_verbatim() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.next_code = RollingCode(4242);
    let import = import(&data(&[shade])).expect("a valid shade");
    assert_eq!(import.shades[0].initial_code, RollingCode(4242));
}

#[test]
fn names_addresses_kinds_and_travel_times_are_carried_across() {
    let import = import(&data(&[migrated("Kitchen", 0x00_1001)])).expect("a valid shade");
    let config = &import.shades[0].config;
    assert_eq!(config.name.as_str(), "Kitchen");
    assert_eq!(config.address, 0x00_1001);
    assert_eq!(config.kind, ShadeKind::Blind);
    assert_eq!(config.tilt_mode, TiltMode::Integrated);
    assert_eq!(config.up_time_ms, 30_000);
    assert_eq!(config.down_time_ms, 29_000);
    assert_eq!(config.tilt_time_ms, 5_000);
    assert!(import.warnings.is_empty(), "nothing needed substituting");
}

/// Ids come from position, not from the ids the old controller assigned.
#[test]
fn the_table_is_in_the_backups_order_and_renumbered_from_zero() {
    let mut first = migrated("First", 0x00_1001);
    first.shade_id = 30;
    let mut second = migrated("Second", 0x00_1002);
    second.shade_id = 4;

    let import = import(&data(&[first, second])).expect("two valid shades");
    assert_eq!(import.shades[0].config.name.as_str(), "First");
    assert_eq!(import.shades[1].config.name.as_str(), "Second");
}

/// The table the tool writes has to be one the device will accept.
#[test]
fn an_imported_table_survives_the_records_own_decode() {
    use somfy_config::ShadeRecord;

    let import = import(&data(&[
        migrated("Kitchen", 0x00_1001),
        migrated("Salon", 0x00_1002),
    ]))
    .expect("two valid shades");

    let record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: heapless::Vec::new(),
        shades: import.shades,
    };
    assert_eq!(ShadeRecord::decode(&record.encode()), Ok(record));
}

// -- obligation 1: unknown kinds default and are reported -----------------

/// `0x05` is a garage door. It is a valid kind on the old controller and
/// not one this firmware models, so it becomes a roller — and says so.
#[test]
fn an_unmodelled_shade_kind_becomes_a_roller_and_is_warned_about() {
    let mut shade = migrated("Garage", 0x00_1001);
    shade.kind_raw = 0x05;

    let import = import(&data(&[shade])).expect("the shade is imported, not dropped");
    assert_eq!(import.shades.len(), 1, "the shade is kept");
    assert_eq!(import.shades[0].config.kind, ShadeKind::Roller);
    assert_eq!(
        import.warnings,
        vec![Warning {
            index: 0,
            name: "Garage".to_string(),
            caveat: Caveat::Kind(0x05),
        }]
    );
}

#[test]
fn an_unmodelled_tilt_mode_becomes_none_and_is_warned_about() {
    let mut shade = migrated("Store", 0x00_1001);
    shade.tilt_mode_raw = 0x09;

    let import = import(&data(&[shade])).expect("the shade is imported, not dropped");
    assert_eq!(import.shades[0].config.tilt_mode, TiltMode::None);
    assert_eq!(
        import.warnings,
        vec![Warning {
            index: 0,
            name: "Store".to_string(),
            caveat: Caveat::TiltMode(0x09),
        }]
    );
}

/// One shade can need every one of them, and the report has to carry every
/// one — a warning that hides another warning is the failure this whole rule
/// is about. Asserts the caveats themselves, not how many there are: a count
/// is equally satisfied by the same warning four times.
#[test]
fn a_shade_needing_every_caveat_is_warned_about_for_each() {
    let mut shade = migrated("Gate", 0x00_1001);
    shade.kind_raw = 0x0B;
    shade.tilt_mode_raw = 0xFF;
    // **Not 80.** Eighty is a width this controller transmits at now, so it
    // draws no caveat; the caveat is for a bit length that is not a width.
    shade.bit_length = 42;
    shade.proto_raw = 0x09;

    let import = import(&data(&[shade])).expect("the shade is imported");
    assert_eq!(
        import
            .warnings
            .iter()
            .map(|warning| warning.caveat)
            .collect::<Vec<_>>(),
        vec![
            Caveat::Kind(0x0B),
            Caveat::TiltMode(0xFF),
            Caveat::FrameWidth(42),
            Caveat::Protocol(0x09),
        ],
    );
}

// -- the width, which the device now honours per shade --------------------

/// **This is the test that changed when the transmitter learnt about width.**
///
/// It used to assert the opposite: that a shade paired at 80 bits imports with
/// a caveat, because the controller transmitted one width for the whole
/// installation and would not reach it. `PlannedTx` carries the width now, so
/// an 80-bit shade is one this firmware drives — and a caveat that fired here
/// would be a warning about nothing, which is worse than no warning at all
/// because it teaches an operator to ignore the list.
#[test]
fn either_of_the_protocols_two_widths_imports_without_a_caveat() {
    for bits in [56, 80] {
        let mut shade = migrated("Awning", 0x00_1001);
        shade.bit_length = bits;
        let import = import(&data(&[shade])).expect("the shade is imported, not dropped");
        assert!(
            import.warnings.is_empty(),
            "{bits}-bit drew a caveat: {:?}",
            import.warnings,
        );
        assert_eq!(
            import.shades[0].config.frame_width,
            FrameWidth::from_raw(bits).expect("a real width"),
            "the width was not carried into the record",
        );
    }
}

/// A bit length that is not a width at all is still a caveat, and now it is the
/// only width caveat there is: nothing can be stored faithfully, so the shade
/// falls back to `ShadeConfig::new`'s default.
#[test]
fn a_bit_length_that_is_not_a_frame_width_is_warned_about() {
    let mut shade = migrated("Awning", 0x00_1001);
    shade.bit_length = 42;

    let import = import(&data(&[shade])).expect("the shade is imported, not dropped");
    assert_eq!(
        import.warnings,
        vec![Warning {
            index: 0,
            name: "Awning".to_string(),
            caveat: Caveat::FrameWidth(42),
        }]
    );
    assert_eq!(import.shades[0].config.frame_width, FrameWidth::Bits56);
}

/// And the same for the protocol: `somfy-rts` encodes one, `ShadeConfig` has
/// no field to select another, so a shade set to a different one is
/// provisioned and inert.
#[test]
fn a_shade_using_another_radio_protocol_is_warned_about() {
    let mut shade = migrated("Relay", 0x00_1001);
    shade.proto_raw = 0x08;

    let import = import(&data(&[shade])).expect("the shade is imported, not dropped");
    assert_eq!(
        import.warnings,
        vec![Warning {
            index: 0,
            name: "Relay".to_string(),
            caveat: Caveat::Protocol(0x08),
        }]
    );
}

#[test]
fn the_protocol_this_controller_speaks_is_not_warned_about() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.proto_raw = TRANSMITTED_PROTOCOL;
    assert!(import(&data(&[shade])).expect("valid").warnings.is_empty());
}

/// A warning names the shade it is about, and the index is the id the shade
/// will have — not its position among the warnings.
#[test]
fn a_warning_names_the_shade_it_is_about() {
    let good = migrated("Kitchen", 0x00_1001);
    let mut odd = migrated("Garage", 0x00_1002);
    odd.kind_raw = 0x06;

    let import = import(&data(&[good, odd])).expect("both shades import");
    assert_eq!(import.warnings.len(), 1);
    assert_eq!(import.warnings[0].index, 1);
    assert_eq!(import.warnings[0].name, "Garage");
}

// -- obligation 2: misalignment reaches the caller ------------------------

#[test]
fn a_misaligned_backup_is_imported_but_reported_as_misaligned() {
    let mut misaligned = data(&[migrated("Kitchen", 0x00_1001)]);
    misaligned.skipped_resyncs = 3;

    let import = import(&misaligned).expect("the values are still imported");
    assert_eq!(import.skipped_resyncs, 3);
    assert!(import.misaligned());
}

#[test]
fn a_backup_whose_records_aligned_is_not_reported_as_misaligned() {
    let import = import(&data(&[migrated("Kitchen", 0x00_1001)])).expect("valid");
    assert!(!import.misaligned());
}

// -- refusals ------------------------------------------------------------

#[test]
fn a_backup_with_no_shades_is_refused() {
    assert_eq!(import(&data(&[])), Err(Refusal::NoShades));
}

#[test]
fn a_shade_with_no_name_is_refused() {
    assert_eq!(
        import(&data(&[migrated("", 0x00_1001)])),
        Err(Refusal::Unnamed { index: 0 }),
    );
}

/// Straight through `ShadeConfig::new`, so the tool refuses exactly what the
/// device would refuse rather than a rule restated here.
#[test]
fn a_shade_at_a_sentinel_address_is_refused_and_named() {
    use somfy_domain::DomainError;

    assert_eq!(
        import(&data(&[migrated("Kitchen", 0)])),
        Err(Refusal::Shade {
            index: 0,
            name: "Kitchen".to_string(),
            error: ShadeError::Domain(DomainError::InvalidAddress),
        }),
    );
}

#[test]
fn a_shade_with_a_zero_travel_time_is_refused_and_named() {
    use somfy_config::TravelField;

    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.down_time_ms = 0;

    assert_eq!(
        import(&data(&[shade])),
        Err(Refusal::Shade {
            index: 0,
            name: "Kitchen".to_string(),
            error: ShadeError::TravelTimeZero {
                field: TravelField::Down,
            },
        }),
    );
}

/// The device refuses the whole table for a repeated address, so importing
/// one would write a file that cannot be loaded.
#[test]
fn two_shades_at_one_address_are_refused() {
    assert_eq!(
        import(&data(&[
            migrated("Kitchen", 0x00_1001),
            migrated("Salon", 0x00_1001),
        ])),
        Err(Refusal::DuplicateAddress {
            index: 1,
            first: 0,
            name: "Salon".to_string(),
            address: 0x00_1001,
        }),
    );
}

/// Both routes to "too many shades" — the parser's, which is the one a
/// person meets, and this module's, which only fires if the two capacities
/// ever stop matching — have to say the same thing.
#[test]
fn both_too_many_shades_refusals_name_the_same_limit() {
    let limit = SHADE_TABLE_CAPACITY.to_string();
    for refusal in [
        Refusal::TooManyShades,
        Refusal::Unreadable(MigrateError::BadRecord("too_many_shades")),
    ] {
        assert!(
            refusal.to_string().contains(&limit),
            "must name the {limit}-shade limit: {refusal}"
        );
    }
}

/// Which variant a refusal is, and how many there are.
///
/// The pair exists so [`every_refusal_says_something`] cannot silently stop
/// covering the enum. Adding a variant breaks this `match` (a compile error);
/// fixing the match without adding a sample fails the coverage assertion. The
/// `MigrateError` half needs no equivalent — [`Refusal`]'s own `Display`
/// matches it exhaustively, so a new one breaks there first.
fn refusal_variant(refusal: &Refusal) -> usize {
    match refusal {
        Refusal::Unreadable(_) => 0,
        Refusal::NoShades => 1,
        Refusal::TooManyShades => 2,
        Refusal::Unnamed { .. } => 3,
        Refusal::Shade { .. } => 4,
        Refusal::DuplicateAddress { .. } => 5,
        Refusal::TooManyLinks { .. } => 6,
        Refusal::Link { .. } => 7,
    }
}
const REFUSAL_VARIANTS: usize = 6;

/// Every refusal has to read as a sentence a person can act on, not as the
/// `Debug` spelling of an enum — and every refusal has to be in the list.
#[test]
fn every_refusal_says_something() {
    let samples = [
        Refusal::Unreadable(MigrateError::UnexpectedEof),
        Refusal::Unreadable(MigrateError::BadNumber),
        Refusal::Unreadable(MigrateError::StringTooLong),
        Refusal::Unreadable(MigrateError::UnsupportedVersion(18)),
        Refusal::Unreadable(MigrateError::BadRecord("too_many_groups")),
        Refusal::NoShades,
        Refusal::TooManyShades,
        Refusal::Unnamed { index: 0 },
        Refusal::Shade {
            index: 0,
            name: "Kitchen".to_string(),
            error: ShadeError::Domain(somfy_domain::DomainError::NameTooLong),
        },
        Refusal::DuplicateAddress {
            index: 1,
            first: 0,
            name: "Salon".to_string(),
            address: 0x00_1001,
        },
    ];

    for variant in 0..REFUSAL_VARIANTS {
        assert!(
            samples.iter().any(|r| refusal_variant(r) == variant),
            "refusal variant {variant} has no sample here, so nothing checks what it says"
        );
    }

    for refusal in &samples {
        let said = refusal.to_string();
        assert!(said.len() > 20, "{refusal:?} says too little: {said:?}");
        assert!(
            said.chars().next().is_some_and(char::is_lowercase),
            "{refusal:?} should continue the sentence it is printed after: {said:?}"
        );
    }
}

/// A refusal stops the whole import — no partial table reaches the caller,
/// because a partial table is a renumbered one.
#[test]
fn a_refusal_yields_no_shades_at_all() {
    let result = import(&data(&[
        migrated("Kitchen", 0x00_1001),
        migrated("Broken", 0),
        migrated("Salon", 0x00_1002),
    ]));

    // No table on the error side, by the return type — so what is left to
    // check is that the message points at the shade to go and fix, and not at
    // the first or the last one, which is what an off-by-one here would look
    // like to the person reading it.
    let Err(Refusal::Shade { index, name, .. }) = result else {
        panic!("a shade at the sentinel address must be refused: {result:?}");
    };
    assert_eq!((index, name.as_str()), (1, "Broken"));
}

// -- linked remotes: written, and still not shades ------------------------

/// The wall remotes are now carried across, because they are the only thing
/// that can correct a position estimate: RTS is one-way, so a shade whose
/// remotes are unknown drifts every time somebody presses the switch on the
/// wall, and nothing anywhere says why.
///
/// The half this test kept from the one it replaces is the important half: a
/// linked remote shares a motor, not an identity, and must never become a
/// shade of its own.
#[test]
fn linked_remotes_are_written_and_are_still_not_shades() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.linked_addresses = hvec(&[0x00_2001, 0x00_2002]);

    let import = import(&data(&[shade])).expect("valid");
    assert_eq!(import.shades.len(), 1, "a linked remote is not a shade");
    assert_eq!(
        import.links.as_slice(),
        &[
            LinkedRemote {
                shade: ShadeId(0),
                address: 0x00_2001,
            },
            LinkedRemote {
                shade: ShadeId(0),
                address: 0x00_2002,
            },
        ],
    );
}

/// And the whole way to the bytes: an imported table with links round-trips
/// through the record, so the device gets what the tool showed.
#[test]
fn imported_links_survive_the_records_round_trip() {
    use somfy_config::ShadeRecord;

    let mut first = migrated("Kitchen", 0x00_1001);
    first.linked_addresses = hvec(&[0x00_2001]);
    let mut second = migrated("Salon", 0x00_1002);
    second.linked_addresses = hvec(&[0x00_2002, 0x00_2003]);

    let import = import(&data(&[first, second])).expect("valid");
    let record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: import.links,
        shades: import.shades,
    };
    assert_eq!(ShadeRecord::decode(&record.encode()), Ok(record));
}

/// A remote linked to the shade it is a remote *for* is the shade's own
/// address arriving twice. The domain refuses it, so the tool does.
#[test]
fn a_remote_at_the_shades_own_address_is_refused() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.linked_addresses = hvec(&[0x00_1001]);

    assert_eq!(
        import(&data(&[shade])),
        Err(Refusal::Link {
            index: 0,
            name: "Kitchen".to_string(),
            address: 0x00_1001,
            error: DomainError::DuplicateAddress,
        }),
    );
}

/// A sentinel address is not a remote, and importing one would put a link in
/// the record that the device then refuses — taking the whole table with it.
#[test]
fn a_sentinel_linked_address_is_refused() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.linked_addresses = hvec(&[0x00_2001, 0]);

    assert_eq!(
        import(&data(&[shade])),
        Err(Refusal::Link {
            index: 0,
            name: "Kitchen".to_string(),
            address: 0,
            error: DomainError::InvalidAddress,
        }),
    );
}

/// The record's pool is shared across the whole table, so a big enough
/// installation runs out. Refused rather than truncated: a dropped link is a
/// wall remote that silently stops correcting a shade, which is the failure
/// this whole field exists to end.
#[test]
fn more_links_than_the_record_holds_are_refused_rather_than_dropped() {
    // Seven remotes each is the domain's per-shade limit, so this fills the
    // shared pool without ever breaking the per-shade one.
    let mut shades = std::vec::Vec::new();
    let mut address = 0x00_2000u32;
    for index in 0..(MAX_LINKS / MAX_LINKED_REMOTES + 1) {
        let mut shade = migrated("Shade", 0x00_1001 + index as u32);
        let mut linked: heapless::Vec<u32, 7> = heapless::Vec::new();
        for _ in 0..MAX_LINKED_REMOTES {
            linked.push(address).unwrap();
            address += 1;
        }
        shade.linked_addresses = linked;
        shades.push(shade);
    }

    let refusal = import(&data(&shades)).expect_err("the pool is not big enough");
    assert!(
        matches!(refusal, Refusal::TooManyLinks { held, .. } if held == MAX_LINKS),
        "{refusal:?}",
    );
}

// -- what was seen but not written ---------------------------------------

#[test]
fn groups_are_counted_but_not_written() {
    use somfy_migrate::MigratedGroup;

    let mut with_group = data(&[migrated("Kitchen", 0x00_1001)]);
    with_group
        .groups
        .push(MigratedGroup {
            group_id: 1,
            name: hstr("Whole House"),
            address: 0x00_9001,
            next_code: RollingCode(1),
            member_shade_ids: heapless::Vec::new(),
        })
        .expect("fits");

    let import = import(&with_group).expect("valid");
    assert_eq!(import.groups, 1);
    assert_eq!(import.shades.len(), 1, "a group is not a shade");
}

/// A favourite is real behaviour in the domain (`Shade::my_pos`) that this
/// record has no field for, so it has to be counted and said — but counted,
/// not warned per shade, or the two caveats that mean a shade will not move at
/// all would be buried under it.
#[test]
fn favourites_are_counted_but_not_written() {
    let mut with = migrated("Kitchen", 0x00_1001);
    with.my_position_centi = 5_000; // 50.00%
    let mut without = migrated("Salon", 0x00_1002);
    without.my_position_centi = -100; // the unset sentinel

    let import = import(&data(&[with, without])).expect("both import");
    assert_eq!(import.favourites, 1);
    assert!(
        import.warnings.is_empty(),
        "a favourite is a count, not a per-shade warning"
    );
}

/// Fully closed is a favourite too — the boundary between "set" and the
/// negative unset sentinel is zero, and an off-by-one here would drop it.
#[test]
fn a_favourite_at_fully_open_is_still_a_favourite() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.my_position_centi = 0;
    assert_eq!(import(&data(&[shade])).expect("valid").favourites, 1);
}

#[test]
fn rooms_are_counted_but_not_written() {
    use somfy_migrate::MigratedRoom;

    let mut with_room = data(&[migrated("Kitchen", 0x00_1001)]);
    with_room
        .rooms
        .push(MigratedRoom {
            room_id: 1,
            name: hstr("Living Room"),
        })
        .expect("fits");

    let import = import(&with_room).expect("valid");
    assert_eq!(import.rooms, 1);
    assert_eq!(import.shades.len(), 1, "a room is not a shade");
}

#[test]
fn the_backups_version_is_carried_into_the_report() {
    let mut old = data(&[migrated("Kitchen", 0x00_1001)]);
    old.version = 19;
    assert_eq!(import(&old).expect("valid").version, 19);
}

// -- the whole path, from a backup's bytes -------------------------------
//
// Everything above starts from parsed data, which is where the mapping
// rules live. These start from bytes, because two of the refusals a person
// will actually meet — a backup too large for the table, and one whose
// records did not align — are decided by the parser and only *reported*
// here, and a report nobody has read is not a report.

/// One shade's fields in wire order. The values a test does not vary are
/// fixed here so a fixture line reads as the thing it is testing.
fn shade_fields(name: &str, address: u32, last_code: u16) -> Vec<String> {
    let mut fields: Vec<String> = vec![
        "7".to_string(),                          // shadeId
        "true".to_string(),                       // paired
        (ShadeKind::Blind as u8).to_string(),     // shadeType
        address.to_string(),                      // remoteAddress
        name.to_string(),                         // name
        (TiltMode::Integrated as u8).to_string(), // tiltType
        TRANSMITTED_PROTOCOL.to_string(),         // proto
        "56".to_string(),                         // bitLength
        "30000".to_string(),                      // upTime
        "29000".to_string(),                      // downTime
        "5000".to_string(),                       // tiltTime
        "100".to_string(),                        // stepSize
    ];
    fields.extend((0..7).map(|_| "0".to_string())); // 7 linked-remote slots
    fields.push(last_code.to_string()); // lastRollingCode
    fields.extend(
        [
            "0",        // flags
            "-1.00000", // myPos
            "-1.00000", // myTiltPos
            "0.00000",  // currentPos
            "0.00000",  // currentTiltPos
            "false",    // flipCommands
            "false",    // flipPosition
            "1",        // repeats
            "2",        // sortOrder
            "0",        // gpioUp
            "0",        // gpioDown
            "0",        // gpioMy
            "0",        // gpioFlags
            "1",        // roomId — the last field, ends the record
        ]
        .iter()
        .map(|field| field.to_string()),
    );
    fields
}

/// Assemble a backup's bytes: header, then the shade records, then the
/// trailing records this import does not read. No rooms and no groups, so a
/// fixture is only as long as what it is about. Fields are unpadded — a
/// real export is fixed-width, and the parser tolerates both.
fn backup(version: u8, shades: &[Vec<String>]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut line = |fields: &[String]| {
        out.extend_from_slice(fields.join(",").as_bytes());
        out.push(b'\n');
    };

    let mut header: Vec<String> = vec![
        version.to_string(),
        "76".to_string(),  // header length
        "29".to_string(),  // room record size
        "0".to_string(),   // room record count
        "276".to_string(), // shade record size
        shades.len().to_string(),
        "200".to_string(), // group record size
        "0".to_string(),   // group record count
    ];
    if version >= 21 {
        header.push("77".to_string()); // repeater record size
        header.push("0".to_string()); // repeater record count
    }
    header.extend(
        ["552", "318", "78", "Fixture"]
            .iter()
            .map(|field| field.to_string()),
    );
    line(&header);

    for shade in shades {
        line(shade);
    }
    line(&["settings".to_string(), "record".to_string()]);
    line(&["net".to_string(), "record".to_string()]);
    line(&["trans".to_string(), "record".to_string()]);
    out
}

/// The end-to-end version of the field that costs a re-pairing to get
/// wrong. The file stores the **last code sent**; what reaches the record
/// must be the next one, and this is the only test that watches the whole
/// distance from a byte in a file to a byte in the flash image.
#[test]
fn a_stored_last_sent_code_reaches_the_record_as_the_next_one() {
    let bytes = backup(25, &[shade_fields("Kitchen", 0x00_1001, 41)]);
    let import = read_backup(&bytes).expect("a well-formed backup");
    assert_eq!(import.shades[0].initial_code, RollingCode(42));

    // And on to the bytes. A round-trip through `encode`/`decode` alone would
    // pass even if the two agreed on a wrong offset, so this reads the code
    // out of the image at the position the record's own docs give it:
    // entry 0 starts after the 20-byte header, and the code is 4 bytes into an
    // entry, after the address.
    let image = somfy_config::ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: heapless::Vec::new(),
        shades: import.shades,
    }
    .encode();
    assert_eq!(&image[20 + 4..20 + 6], &42u16.to_le_bytes());
}

/// The one piece of arithmetic in the whole contract, at the only value where
/// it can be wrong: a controller that sent 65535 has next-to-send 0, and a
/// seed of 0 has to survive being written and read back like any other.
#[test]
fn a_code_that_wrapped_past_the_last_one_arrives_as_zero() {
    use somfy_config::ShadeRecord;

    let bytes = backup(25, &[shade_fields("Kitchen", 0x00_1001, u16::MAX)]);
    let import = read_backup(&bytes).expect("a well-formed backup");
    assert_eq!(import.shades[0].initial_code, RollingCode(0));

    let record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: heapless::Vec::new(),
        shades: import.shades,
    };
    let decoded = ShadeRecord::decode(&record.encode()).expect("the table decodes");
    assert_eq!(decoded.shades[0].initial_code, RollingCode(0));
}

/// The backup order is the id order, end to end.
#[test]
fn a_backup_of_several_shades_imports_in_file_order() {
    let bytes = backup(
        25,
        &[
            shade_fields("Kitchen", 0x00_1001, 1),
            shade_fields("Salon", 0x00_1002, 2),
            shade_fields("Study", 0x00_1003, 3),
        ],
    );
    let import = read_backup(&bytes).expect("a well-formed backup");
    let names: Vec<&str> = import
        .shades
        .iter()
        .map(|shade| shade.config.name.as_str())
        .collect();
    assert_eq!(names, ["Kitchen", "Salon", "Study"]);
}

/// A record carrying more than it should — the shape an unescaped comma in
/// a name produces — leaves the parser resyncing, and that must reach the
/// caller as the flag that makes the tool ask before it writes.
#[test]
fn a_record_that_did_not_align_arrives_flagged_rather_than_refused() {
    let mut fields = shade_fields("Kitchen", 0x00_1001, 41);
    fields.push("stray".to_string());
    let import = read_backup(&backup(25, &[fields])).expect("the values still import");

    assert!(
        import.misaligned(),
        "a record with leftover fields must be reported as misaligned"
    );
    assert_eq!(import.skipped_resyncs, 1);
    assert_eq!(import.shades.len(), 1, "the shade is imported, not dropped");
}

/// The other side of that boundary, and the one that actually pins the two
/// capacities together: a backup holding exactly what the table holds must
/// import whole and encode into one record. `both_too_many_shades_refusals_
/// name_the_same_limit` cannot catch a divergence, because both messages are
/// *formatted* from this crate's constant — this can.
#[test]
fn a_backup_of_exactly_the_table_capacity_imports_whole() {
    use somfy_config::ShadeRecord;

    let shades: Vec<Vec<String>> = (0..SHADE_TABLE_CAPACITY)
        .map(|n| shade_fields("Shade", 0x00_1000 + n as u32, n as u16))
        .collect();
    let import = read_backup(&backup(25, &shades)).expect("exactly the capacity fits");
    assert_eq!(import.shades.len(), SHADE_TABLE_CAPACITY);

    let record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: heapless::Vec::new(),
        shades: import.shades,
    };
    assert_eq!(ShadeRecord::decode(&record.encode()), Ok(record));
}

/// More shades than the table holds is refused whole, and the refusal says
/// what the limit is rather than naming an internal parser state.
#[test]
fn a_backup_larger_than_the_table_is_refused_by_the_limit() {
    let shades: Vec<Vec<String>> = (0..=SHADE_TABLE_CAPACITY)
        .map(|n| shade_fields("Shade", 0x00_1000 + n as u32, 1))
        .collect();
    let refusal = read_backup(&backup(25, &shades)).expect_err("one more than fits");

    assert_eq!(
        refusal,
        Refusal::Unreadable(MigrateError::BadRecord("too_many_shades")),
    );
    assert!(
        refusal
            .to_string()
            .contains(&SHADE_TABLE_CAPACITY.to_string()),
        "the refusal must name the limit: {refusal}"
    );
}

#[test]
fn a_truncated_backup_is_refused() {
    let mut bytes = backup(25, &[shade_fields("Kitchen", 0x00_1001, 41)]);
    bytes.truncate(bytes.len() / 2);
    assert_eq!(
        read_backup(&bytes),
        Err(Refusal::Unreadable(MigrateError::UnexpectedEof)),
    );
}

/// A version outside the window the parser was checked against is refused
/// rather than read: an unrecognised layout misaligns every record, and a
/// misaligned record is a plausible wrong rolling code.
#[test]
fn a_backup_of_an_unreadable_version_is_refused_by_version() {
    let below = MIN_SUPPORTED_VERSION - 1;
    let bytes = backup(below, &[shade_fields("Kitchen", 0x00_1001, 41)]);
    let refusal = read_backup(&bytes).expect_err("below the supported window");

    assert_eq!(
        refusal,
        Refusal::Unreadable(MigrateError::UnsupportedVersion(below)),
    );
    assert!(
        refusal.to_string().contains(&below.to_string()),
        "the refusal must name the version it saw: {refusal}"
    );
}

#[test]
fn something_that_is_not_a_backup_at_all_is_refused() {
    assert!(read_backup(b"this is not a backup\n").is_err());
    assert!(read_backup(b"").is_err());
}

// -- the real thing ------------------------------------------------------

/// Import a backup exported from the controller this project replaces.
///
/// `#[ignore]`d for the same reason `somfy-migrate`'s golden test is: the
/// file is a real installation's radio addresses and rolling codes, so it
/// is gitignored and simply absent on most machines. Run it with
/// `cargo test -p somfy-config --example provision_shades -- --ignored`
/// after placing one at the path below — see
/// `crates/somfy-migrate/tests/fixtures/README.md`.
///
/// **It asserts shapes and counts and never a value.** A failure message
/// naming an address would put that address in a CI log, which is the one
/// thing the file's whole handling is arranged to prevent, so every
/// assertion below identifies a shade by its index.
#[test]
#[ignore = "requires a real device backup — see somfy-migrate's fixtures README"]
fn a_real_backup_imports_to_the_shape_the_parser_reports() {
    use somfy_config::ShadeRecord;
    use somfy_migrate::parse_backup;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../somfy-migrate/tests/fixtures/real_device.backup");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\n\
             Export a backup from the device and place it there.\n\
             See crates/somfy-migrate/tests/fixtures/README.md.",
            path.display()
        )
    });

    let parsed = parse_backup(&bytes).expect("the real backup parses");
    let import = read_backup(&bytes).expect("the real backup imports");

    // Every shade the parser found is a shade in the table: none dropped,
    // none invented, and the order is the file's.
    assert_eq!(
        import.shades.len(),
        parsed.shades.len(),
        "every parsed shade must reach the table"
    );
    assert!(!import.shades.is_empty(), "a real backup has shades");
    assert_eq!(import.groups, parsed.groups.len());
    assert_eq!(import.version, parsed.version);

    // A real export aligns exactly, so this table needs no confirmation.
    assert!(
        !import.misaligned(),
        "record misalignment detected — {} records did not align",
        import.skipped_resyncs
    );

    for (index, shade) in import.shades.iter().enumerate() {
        let config = &shade.config;
        assert!(
            (1..0xFF_FFFF).contains(&config.address),
            "shade {index} address outside 1..0xFFFFFF"
        );
        assert!(!config.name.is_empty(), "shade {index} has no name");
        assert!(config.name.len() <= 32, "shade {index} name does not fit");
        assert!(config.up_time_ms > 0, "shade {index} has no up time");
        assert!(config.down_time_ms > 0, "shade {index} has no down time");

        // The mapping itself, against the only data that can settle it — and
        // *comparing* rather than printing, so the assertion is a boolean and
        // the message is an index. The rolling code is why this file is worth
        // handling carefully at all: one transposed here is a shade that has
        // to be paired again by hand.
        let from_parser = &parsed.shades[index];
        assert!(
            shade.initial_code == from_parser.next_code,
            "shade {index} rolling code was not carried across verbatim"
        );
        assert!(
            config.address == from_parser.address,
            "shade {index} address was not carried across verbatim"
        );
        assert!(
            config.name.as_str() == from_parser.name.as_str(),
            "shade {index} name was not carried across verbatim"
        );
        assert!(
            config.up_time_ms == from_parser.up_time_ms
                && config.down_time_ms == from_parser.down_time_ms
                && config.tilt_time_ms == from_parser.tilt_time_ms,
            "shade {index} travel times were not carried across verbatim"
        );
    }

    // Whatever was flagged, the handling is the documented one.
    for warning in &import.warnings {
        let config = &import.shades[warning.index].config;
        match warning.caveat {
            Caveat::Kind(_) => assert_eq!(config.kind, ShadeKind::Roller),
            Caveat::TiltMode(_) => assert_eq!(config.tilt_mode, TiltMode::None),
            // A width caveat means the byte was not a width at all, so the
            // record carries the constructor's default.
            Caveat::FrameWidth(bits) => {
                assert!(FrameWidth::from_raw(bits).is_none());
                assert_eq!(config.frame_width, FrameWidth::Bits56);
            }
            // Nothing to check in the config for this one: there is no field it
            // could have gone into. What is checked instead is that it did not
            // fire at all — see below.
            Caveat::Protocol(raw) => assert_ne!(raw, TRANSMITTED_PROTOCOL),
        }
    }

    // [`TRANSMITTED_PROTOCOL`] is a *derived* constant — the value the reader
    // substitutes when a record has no protocol field at all — and this is the
    // only place it meets real data. A shade on a working installation of the
    // kind this firmware replaces is drivable by it, so the protocol caveat
    // should not fire. If it does, either a genuinely mixed installation is
    // being imported (and those shades really will not move) or the derivation
    // is wrong; both are worth stopping for. The width caveat is checked with
    // it, because a real backup should carry real widths. Indices only, as
    // everywhere here.
    let undrivable: Vec<usize> = import
        .warnings
        .iter()
        .filter(|warning| matches!(warning.caveat, Caveat::FrameWidth(_) | Caveat::Protocol(_)))
        .map(|warning| warning.index)
        .collect();
    assert!(
        undrivable.is_empty(),
        "shades {undrivable:?} imported as undrivable — either this installation is mixed, \
         carries a bit length that is not a frame width, or TRANSMITTED_PROTOCOL is derived wrong"
    );

    // And the table is one the device will load: the record's own decode is
    // the same code the firmware runs, and the codes have to survive it.
    let record = ShadeRecord {
        seq: 0,
        announced: Announced::NONE,
        links: heapless::Vec::new(),
        shades: import.shades,
    };
    let decoded = ShadeRecord::decode(&record.encode()).expect("the table decodes");
    assert_eq!(decoded.shades.len(), parsed.shades.len());
    assert!(
        decoded.shades == record.shades,
        "the encoded table did not decode back to itself"
    );
}
