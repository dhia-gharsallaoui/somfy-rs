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
        // The width this controller sends, so no test warns about it by
        // accident and the one that does warn says so on its own line.
        bit_length: TRANSMITTED_BIT_LENGTH,
        proto_raw: 1,
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

/// One shade can need both, and the report has to carry both — a warning
/// that hides another warning is the failure this whole rule is about.
#[test]
fn a_shade_needing_both_substitutions_is_warned_about_twice() {
    let mut shade = migrated("Gate", 0x00_1001);
    shade.kind_raw = 0x0B;
    shade.tilt_mode_raw = 0xFF;

    let import = import(&data(&[shade])).expect("the shade is imported");
    assert_eq!(
        import.warnings.len(),
        2,
        "both substitutions must be reported"
    );
}

// -- the width there is no field for -------------------------------------

/// A shade the old controller drove at 80 bits imports looking healthy and
/// will not move: this controller transmits one width for the whole
/// installation and `ShadeConfig` has no field to record another. Silence
/// here is a shade that ignores every command with nothing to say why.
#[test]
fn a_shade_paired_at_another_frame_width_is_warned_about() {
    let mut shade = migrated("Awning", 0x00_1001);
    shade.bit_length = 80;

    let import = import(&data(&[shade])).expect("the shade is imported, not dropped");
    assert_eq!(
        import.warnings,
        vec![Warning {
            index: 0,
            name: "Awning".to_string(),
            caveat: Caveat::FrameWidth(80),
        }]
    );
}

#[test]
fn the_width_this_controller_sends_is_not_warned_about() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.bit_length = TRANSMITTED_BIT_LENGTH;
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

/// Every refusal has to read as a sentence a person can act on, not as the
/// `Debug` spelling of an enum.
#[test]
fn every_refusal_says_something() {
    for refusal in [
        Refusal::Unreadable(MigrateError::BadNumber),
        Refusal::Unreadable(MigrateError::StringTooLong),
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
    ] {
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

// -- what was seen but not written ---------------------------------------

#[test]
fn linked_remotes_are_counted_but_not_written() {
    let mut shade = migrated("Kitchen", 0x00_1001);
    shade.linked_addresses = hvec(&[0x00_2001, 0x00_2002]);

    let import = import(&data(&[shade])).expect("valid");
    assert_eq!(import.linked_remotes, 2);
    assert_eq!(import.shades.len(), 1, "a linked remote is not a shade");
}

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
        "1".to_string(),                          // proto
        TRANSMITTED_BIT_LENGTH.to_string(),       // bitLength
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
    }

    // Whatever was flagged, the handling is the documented one.
    for warning in &import.warnings {
        let config = &import.shades[warning.index].config;
        match warning.caveat {
            Caveat::Kind(_) => assert_eq!(config.kind, ShadeKind::Roller),
            Caveat::TiltMode(_) => assert_eq!(config.tilt_mode, TiltMode::None),
            // Nothing to check in the config: the whole point of this one
            // is that there is no field it could have gone into.
            Caveat::FrameWidth(bits) => assert_ne!(bits, TRANSMITTED_BIT_LENGTH),
        }
    }

    // And the table is one the device will load: the record's own decode is
    // the same code the firmware runs.
    let record = ShadeRecord {
        seq: 0,
        shades: import.shades,
    };
    let decoded = ShadeRecord::decode(&record.encode()).expect("the table decodes");
    assert_eq!(decoded.shades.len(), parsed.shades.len());
}
