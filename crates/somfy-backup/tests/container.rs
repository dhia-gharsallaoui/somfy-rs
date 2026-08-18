//! The `RTSB` container: what it carries, what it refuses, and the one thing
//! it must never carry.
//!
//! The interesting tests here are not the round trip. They are:
//!
//! - **`a_backup_carries_no_secret_anywhere_in_its_bytes`** — the rule the whole
//!   format is arranged around. An export is an unauthenticated `GET`, so a
//!   container with a passphrase in it would be the LAN API reading the
//!   passphrase out, which is precisely what `somfy_api::settings` is built to
//!   prevent.
//! - **`the_live_rolling_code_survives_a_round_trip`** — the reason a backup is
//!   worth having at all. A lost rolling code costs a physical re-pairing at
//!   each motor.
//! - The five refusals, each broken deliberately and each observed.

use core::net::Ipv4Addr;

use somfy_backup::{
    checksum, decode, encode, looks_like_backup, write_header, Backup, BackupError, BackupMeta,
    Codes, MetaField, TableError, BACKUP_LEN, HEADER_LEN, MAX_CODES, OFF_CODES, OFF_ESTATE,
    OFF_SHADES,
};
use somfy_config::{
    Announced, EstateRecord, ShadeRecord, StoredGroup, StoredRoom, StoredShade, ESTATE_RECORD_LEN,
    SHADE_RECORD_LEN,
};
use somfy_domain::{RoomId, ShadeConfig, ShadeKind, TiltMode};
use somfy_rts::RollingCode;

/// A passphrase chosen to be findable in a byte haystack. Synthetic.
const SECRET_PSK: &str = "ZZZ-wifi-secret-never-leaves-ZZZ";
/// The same, for the broker.
const SECRET_PASSWORD: &str = "QQQ-broker-secret-never-leaves-QQQ";

/// An address in the documentation half of the space, never a real one.
const SYNTHETIC_ADDRESS: u32 = 0x00_AB_CD_01;

fn a_shade(address: u32, name: &str, code: RollingCode) -> StoredShade {
    let mut config = ShadeConfig::new(name, address).expect("a legal shade");
    config.kind = ShadeKind::Roller;
    config.tilt_mode = TiltMode::None;
    config.up_time_ms = 10_000;
    config.down_time_ms = 10_000;
    config.tilt_time_ms = 0;
    StoredShade::new(config, code).expect("a legal stored shade")
}

fn a_shade_record() -> [u8; SHADE_RECORD_LEN] {
    let mut record = ShadeRecord {
        seq: 7,
        announced: Announced::NONE,
        shades: heapless::Vec::new(),
        links: heapless::Vec::new(),
    };
    record
        .shades
        .push(a_shade(SYNTHETIC_ADDRESS, "Kitchen", RollingCode(4_242)))
        .expect("capacity");
    record
        .shades
        .push(a_shade(SYNTHETIC_ADDRESS + 1, "Study", RollingCode(9)))
        .expect("capacity");
    record.encode()
}

fn an_estate_record() -> [u8; ESTATE_RECORD_LEN] {
    let mut record = EstateRecord::empty(3);
    record
        .rooms
        .push(StoredRoom {
            name: "Ground floor".try_into().expect("fits"),
        })
        .expect("capacity");
    record.room_of[0] = Some(RoomId(0));
    record
        .groups
        .push(StoredGroup {
            name: "All".try_into().expect("fits"),
            address: SYNTHETIC_ADDRESS + 2,
            next_code: RollingCode(11),
            code_recovered: true,
            members: somfy_config::Members::NONE.with(somfy_domain::ShadeId(0)),
        })
        .expect("capacity");
    record.encode()
}

/// The rolling codes a two-shade installation would export, with the second
/// deliberately *ahead* of the seed in the table so a test can tell the two
/// apart. That difference is the whole reason the block exists.
fn a_codes() -> Codes {
    let mut codes = Codes::new();
    assert!(codes.push(SYNTHETIC_ADDRESS, 5_000));
    assert!(codes.push(SYNTHETIC_ADDRESS + 1, 61_000));
    codes
}

fn a_meta() -> BackupMeta {
    BackupMeta {
        ssid: Some("example-network".try_into().expect("fits")),
        psk_was_set: true,
        broker: Some(Ipv4Addr::new(192, 0, 2, 10)),
        broker_password_was_set: true,
    }
}

/// Build a container around a header a test has patched.
///
/// The checksum is computed over the patched header, so every test using this
/// is exercising the field it patched rather than the checksum — which is what
/// makes `one_flipped_byte_anywhere_fails_the_checksum` a separate test with a
/// separate cause.
fn reassemble(
    header: &[u8; HEADER_LEN],
    shades: &[u8; SHADE_RECORD_LEN],
    estate: &[u8; ESTATE_RECORD_LEN],
) -> [u8; BACKUP_LEN] {
    let crc = checksum(header, shades, estate);
    let mut bytes = [0u8; BACKUP_LEN];
    bytes[..HEADER_LEN].copy_from_slice(header);
    bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN].copy_from_slice(shades);
    bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN].copy_from_slice(estate);
    bytes[BACKUP_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

// ---------------------------------------------------------------------------
// The rule the format exists to keep
// ---------------------------------------------------------------------------

#[test]
fn a_backup_carries_no_secret_anywhere_in_its_bytes() {
    // The two secrets are not passed in — there is no parameter for either,
    // which is the structural half of the rule. This searches the bytes anyway,
    // because the point is the *file*, and a future field could carry one
    // without anybody noticing at the call site.
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    assert!(
        !bytes
            .windows(SECRET_PSK.len())
            .any(|window| window == SECRET_PSK.as_bytes()),
        "the Wi-Fi passphrase reached a backup",
    );
    assert!(
        !bytes
            .windows(SECRET_PASSWORD.len())
            .any(|window| window == SECRET_PASSWORD.as_bytes()),
        "the broker password reached a backup",
    );
}

#[test]
fn it_says_which_secrets_were_set_so_a_person_knows_what_to_retype() {
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let backup = decode(&bytes).expect("a well-formed container");
    assert_eq!(backup.meta.ssid.as_deref(), Some("example-network"));
    assert!(backup.meta.psk_was_set);
    assert_eq!(backup.meta.broker, Some(Ipv4Addr::new(192, 0, 2, 10)));
    assert!(backup.meta.broker_password_was_set);
}

#[test]
fn an_open_network_is_a_configuration_and_not_a_missing_passphrase() {
    // `psk_was_set: false` with an SSID present is an open network. It has to be
    // distinguishable from "there is a passphrase and this file does not carry
    // it", because the first needs no action and the second needs typing.
    let meta = BackupMeta {
        ssid: Some("example-open".try_into().expect("fits")),
        psk_was_set: false,
        broker: None,
        broker_password_was_set: false,
    };
    let bytes = encode(&meta, &a_codes(), &a_shade_record(), &an_estate_record());
    let backup = decode(&bytes).expect("a well-formed container");
    assert_eq!(backup.meta.ssid.as_deref(), Some("example-open"));
    assert!(!backup.meta.psk_was_set);
    assert_eq!(backup.meta.broker, None);
}

#[test]
fn a_device_with_nothing_provisioned_still_exports_a_readable_backup() {
    // A freshly flashed board with shades and no network is an ordinary state,
    // and its backup is worth as much as any other — it carries the rolling
    // codes.
    let bytes = encode(
        &BackupMeta::default(),
        &Codes::new(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let backup = decode(&bytes).expect("a well-formed container");
    assert_eq!(backup.meta, BackupMeta::default());
}

// ---------------------------------------------------------------------------
// The reason a backup is worth having
// ---------------------------------------------------------------------------

#[test]
fn the_live_rolling_code_survives_a_round_trip() {
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let Backup { shades, .. } = decode(&bytes).expect("a well-formed container");

    let record = ShadeRecord::decode(shades).expect("the embedded record decodes");
    assert_eq!(record.shades.len(), 2);
    assert_eq!(record.shades[0].initial_code, RollingCode(4_242));
    assert_eq!(record.shades[1].initial_code, RollingCode(9));
    assert_eq!(record.shades[0].config.address, SYNTHETIC_ADDRESS);
}

#[test]
fn the_two_records_are_carried_byte_for_byte() {
    // Which is what makes the decoder on the far side the same one the boot path
    // already uses, rather than a second reader that could disagree with it.
    let shades = a_shade_record();
    let estate = an_estate_record();
    let bytes = encode(&a_meta(), &a_codes(), &shades, &estate);
    assert_eq!(
        &bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN],
        &shades[..]
    );
    assert_eq!(
        &bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN],
        &estate[..]
    );
}

#[test]
fn the_estate_survives_a_round_trip_beside_the_shades() {
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let Backup { estate, .. } = decode(&bytes).expect("a well-formed container");
    let record = EstateRecord::decode(estate).expect("the embedded record decodes");
    assert_eq!(record.rooms.len(), 1);
    assert_eq!(record.rooms[0].name.as_str(), "Ground floor");
    assert_eq!(record.groups.len(), 1);
    assert_eq!(record.groups[0].next_code, RollingCode(11));
    assert_eq!(record.room_of[0], Some(RoomId(0)));
}

#[test]
fn the_pieces_an_exporter_streams_agree_with_the_whole() {
    // The firmware never assembles a container: it emits the header, then the
    // two records out of flash, then the checksum, and it computes that
    // checksum over the three pieces. This is the assertion that those two
    // paths produce the same file.
    let meta = a_meta();
    let shades = a_shade_record();
    let estate = an_estate_record();

    let mut header = [0u8; HEADER_LEN];
    write_header(&meta, &a_codes(), &mut header);
    let crc = checksum(&header, &shades, &estate);

    let whole = encode(&meta, &a_codes(), &shades, &estate);
    assert_eq!(&whole[..HEADER_LEN], &header[..]);
    assert_eq!(&whole[BACKUP_LEN - 4..], &crc.to_le_bytes()[..]);
}

#[test]
fn the_live_codes_travel_beside_the_table_and_are_not_the_seeds_in_it() {
    // The whole point of the separate block. The table's `initial_code` is the
    // seed a shade was provisioned with; the block is where the counter has
    // actually reached. A restore that read the seeds instead would plant codes
    // the motors have long since passed, and every one of those shades would
    // stop obeying until somebody re-paired it at the window.
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let backup = decode(&bytes).expect("a well-formed container");

    let table = ShadeRecord::decode(backup.shades).expect("the embedded record decodes");
    let live: heapless::Vec<(u32, u16), 4> = backup.codes.iter().collect();

    assert_eq!(live.len(), 2);
    assert_eq!(live[0], (SYNTHETIC_ADDRESS, 5_000));
    assert_eq!(live[1], (SYNTHETIC_ADDRESS + 1, 61_000));
    // And they differ from the seeds, which is what makes this test able to
    // fail if the two are ever confused.
    assert_ne!(table.shades[0].initial_code.0, live[0].1);
    assert_ne!(table.shades[1].initial_code.0, live[1].1);
}

#[test]
fn a_full_registry_of_codes_fits_the_block() {
    // Thirty-two, which is `somfy_domain::MAX_SHADES`. A thirty-third is
    // refused rather than dropped, because a dropped rolling code is a shade
    // that stops obeying and the caller has to be told.
    let mut codes = Codes::new();
    for index in 0..MAX_CODES {
        assert!(codes.push(SYNTHETIC_ADDRESS + index as u32, index as u16));
    }
    assert!(!codes.push(SYNTHETIC_ADDRESS + 99, 1));
    assert_eq!(codes.len(), MAX_CODES);

    let bytes = encode(&a_meta(), &codes, &a_shade_record(), &an_estate_record());
    let backup = decode(&bytes).expect("a well-formed container");
    assert_eq!(backup.codes.len(), MAX_CODES);
    assert_eq!(
        backup.codes.iter().last(),
        Some((SYNTHETIC_ADDRESS + 31, 31))
    );
}

#[test]
fn a_device_with_no_codes_yet_exports_an_empty_block() {
    // A board whose shades have been provisioned but never driven. Ordinary,
    // and it must not read as damage.
    let bytes = encode(
        &a_meta(),
        &Codes::new(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let backup = decode(&bytes).expect("a well-formed container");
    assert!(backup.codes.is_empty());
}

// ---------------------------------------------------------------------------
// The round trip through the reader the boot path uses
//
// **These are the tests that were missing, and a live board found what they
// would have caught.** Everything above exercises `decode`, which checks the
// container and never looks inside the two records it carries — so a device
// could produce a file the host accepted and then refuse itself. The asymmetry
// was real: a board whose estate region has never been written exports a blank
// estate slot, and the boot path read `Blank` as damage.
//
// `Backup::tables` is that reading, moved here so a host test runs the same
// code the device does.
// ---------------------------------------------------------------------------

/// What the exporter writes into a record slot when the region it reads has
/// nothing in it: `firmware::restore::read_region`'s `out.fill(0xFF)`.
///
/// Restated here rather than imported, because the firmware does not compile on
/// a host. It is one line and it is the whole subject of these tests.
const NEVER_WRITTEN: u8 = 0xFF;

#[test]
fn a_device_with_no_estate_can_restore_its_own_backup() {
    // **The bug a live ESP32-S3 found.** The board had four shades and had never
    // been given a room or a group, so its `estate` region was blank; the export
    // was well formed, the host accepted it, and the device refused its own file
    // as `backupDamaged`.
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &[NEVER_WRITTEN; ESTATE_RECORD_LEN],
    );
    let backup = decode(&bytes).expect("the container itself is well formed");
    let shades = backup.shade_table().expect("and so are its contents");
    let estate = backup.estate_table().expect("and so are its contents");

    assert_eq!(shades.shades.len(), 2);
    // A blank slot reads as an empty estate — the same reading the boot path
    // gives a blank *region*, which is what that device was running.
    assert!(estate.rooms.is_empty());
    assert!(estate.groups.is_empty());
}

#[test]
fn a_device_with_no_shades_can_restore_its_own_backup() {
    // The same rule on the other record. A freshly flashed board that has been
    // put on Wi-Fi and has no shades yet must still round-trip.
    let bytes = encode(
        &a_meta(),
        &Codes::new(),
        &[NEVER_WRITTEN; SHADE_RECORD_LEN],
        &an_estate_record(),
    );
    let backup = decode(&bytes).expect("the container itself is well formed");
    let shades = backup.shade_table().expect("and so are its contents");
    let estate = backup.estate_table().expect("and so are its contents");

    assert!(shades.shades.is_empty());
    assert!(shades.links.is_empty());
    assert_eq!(estate.rooms.len(), 1);
}

#[test]
fn a_device_with_nothing_at_all_can_restore_its_own_backup() {
    let bytes = encode(
        &BackupMeta::default(),
        &Codes::new(),
        &[NEVER_WRITTEN; SHADE_RECORD_LEN],
        &[NEVER_WRITTEN; ESTATE_RECORD_LEN],
    );
    let backup = decode(&bytes).expect("well formed");
    let shades = backup.shade_table().expect("readable");
    let estate = backup.estate_table().expect("readable");
    assert!(shades.shades.is_empty());
    assert!(estate.rooms.is_empty());
}

#[test]
fn a_provisioned_device_round_trips_every_field_through_the_boot_path_reader() {
    // The whole journey a real export makes: encoded as the device streams it,
    // decoded as the boot path reads it, and checked on values rather than on
    // shape.
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let backup = decode(&bytes).expect("well formed");
    let shades = backup.shade_table().expect("readable");
    let estate = backup.estate_table().expect("readable");

    assert_eq!(shades.shades.len(), 2);
    assert_eq!(shades.shades[0].config.address, SYNTHETIC_ADDRESS);
    assert_eq!(shades.shades[0].config.name.as_str(), "Kitchen");
    assert_eq!(estate.rooms.len(), 1);
    assert_eq!(estate.groups.len(), 1);
    assert_eq!(estate.room_of[0], Some(RoomId(0)));
    // And the live codes, which are what a backup is worth carrying.
    let live: heapless::Vec<(u32, u16), 4> = backup.codes.iter().collect();
    assert_eq!(live[0], (SYNTHETIC_ADDRESS, 5_000));
}

#[test]
fn a_record_that_is_damaged_rather_than_blank_is_still_refused() {
    // The rule above must not have widened into "any unreadable record is an
    // empty one". A record with a real magic and a broken checksum is damage,
    // and damage is refused — with the field naming which of the two it was, so
    // the log line can say.
    let mut shades = a_shade_record();
    shades[64] ^= 0x01;
    let bytes = encode(&a_meta(), &a_codes(), &shades, &an_estate_record());
    let backup = decode(&bytes).expect("the container's own checksum still covers this");
    assert!(matches!(backup.shade_table(), Err(TableError::Shades(_))));

    let mut estate = an_estate_record();
    estate[64] ^= 0x01;
    let bytes = encode(&a_meta(), &a_codes(), &a_shade_record(), &estate);
    let backup = decode(&bytes).expect("well formed container");
    assert!(matches!(backup.estate_table(), Err(TableError::Estate(_))));
}

// ---------------------------------------------------------------------------
// The refusals, each broken deliberately
// ---------------------------------------------------------------------------

#[test]
fn an_erased_staging_region_is_blank_rather_than_corrupt() {
    let blank = [0xFFu8; BACKUP_LEN];
    assert_eq!(decode(&blank), Err(BackupError::Blank));
}

#[test]
fn the_wrong_file_is_refused_by_its_magic() {
    let mut bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    bytes[0] = b'X';
    assert_eq!(decode(&bytes), Err(BackupError::Magic));
    assert!(!looks_like_backup(&bytes));
}

#[test]
fn the_cheap_prefix_check_agrees_with_the_full_decode() {
    let bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    assert!(looks_like_backup(&bytes));
    assert!(looks_like_backup(&bytes[..4]));
    // Fewer than four bytes cannot say, and answers "no" — which is the safe
    // direction for a check whose only job is to refuse early.
    assert!(!looks_like_backup(&bytes[..3]));
    assert!(!looks_like_backup(b""));
}

#[test]
fn a_container_from_a_later_release_names_its_version() {
    let mut bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    bytes[4] = 2;
    bytes[5] = 0;
    // The checksum is now wrong too, but the version is reported first: the
    // action for a version this build cannot read is "different firmware", and
    // reporting a checksum failure would send the operator to re-download the
    // file instead.
    assert_eq!(decode(&bytes), Err(BackupError::Version(2)));
}

#[test]
fn a_truncated_upload_is_reported_as_truncated_and_not_as_corrupt() {
    let mut bytes = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    bytes[12..16].copy_from_slice(&(BACKUP_LEN as u32 - 100).to_le_bytes());
    assert_eq!(
        decode(&bytes),
        Err(BackupError::Length(BACKUP_LEN as u32 - 100)),
    );
}

#[test]
fn one_flipped_byte_anywhere_fails_the_checksum() {
    // Including inside the embedded records, which the container's own checksum
    // covers as well as their own. Two independent checks over the same bytes
    // is deliberate: the container's catches damage in transit and each
    // record's catches damage in the flash it came from.
    for at in [HEADER_LEN + 5, OFF_ESTATE + 5, BACKUP_LEN - 8] {
        let mut bytes = encode(
            &a_meta(),
            &a_codes(),
            &a_shade_record(),
            &an_estate_record(),
        );
        bytes[at] ^= 0x01;
        assert_eq!(decode(&bytes), Err(BackupError::Checksum), "at {at}");
    }
}

#[test]
fn an_unknown_flag_bit_is_refused_rather_than_masked() {
    let meta = a_meta();
    let shades = a_shade_record();
    let estate = an_estate_record();
    let mut header = [0u8; HEADER_LEN];
    write_header(&meta, &a_codes(), &mut header);
    header[6] |= 0b1000_0000;
    let crc = checksum(&header, &shades, &estate);

    let mut bytes = [0u8; BACKUP_LEN];
    bytes[..HEADER_LEN].copy_from_slice(&header);
    bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN].copy_from_slice(&shades);
    bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN].copy_from_slice(&estate);
    bytes[BACKUP_LEN - 4..].copy_from_slice(&crc.to_le_bytes());

    assert_eq!(decode(&bytes), Err(BackupError::Flags(0b1000_0010 | 0b1)));
}

#[test]
fn an_ssid_longer_than_its_field_is_refused_by_its_length_byte() {
    let meta = a_meta();
    let shades = a_shade_record();
    let estate = an_estate_record();
    let mut header = [0u8; HEADER_LEN];
    write_header(&meta, &a_codes(), &mut header);
    header[7] = 33;
    let crc = checksum(&header, &shades, &estate);

    let mut bytes = [0u8; BACKUP_LEN];
    bytes[..HEADER_LEN].copy_from_slice(&header);
    bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN].copy_from_slice(&shades);
    bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN].copy_from_slice(&estate);
    bytes[BACKUP_LEN - 4..].copy_from_slice(&crc.to_le_bytes());

    assert_eq!(
        decode(&bytes),
        Err(BackupError::FieldLength {
            field: MetaField::Ssid,
            len: 33,
        }),
    );
}

#[test]
fn a_broker_field_that_is_not_an_address_is_refused() {
    let meta = a_meta();
    let shades = a_shade_record();
    let estate = an_estate_record();
    let mut header = [0u8; HEADER_LEN];
    write_header(&meta, &a_codes(), &mut header);
    header[8] = 7;
    header[48..55].copy_from_slice(b"not.an.");
    let crc = checksum(&header, &shades, &estate);

    let mut bytes = [0u8; BACKUP_LEN];
    bytes[..HEADER_LEN].copy_from_slice(&header);
    bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN].copy_from_slice(&shades);
    bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN].copy_from_slice(&estate);
    bytes[BACKUP_LEN - 4..].copy_from_slice(&crc.to_le_bytes());

    assert_eq!(decode(&bytes), Err(BackupError::BrokerMalformed));
}

#[test]
fn an_ssid_the_credential_rules_refuse_cannot_arrive_through_a_file() {
    // An SSID containing a NUL is one `WifiCredentials::new` refuses, and the
    // whole point of routing the field back through that constructor is that a
    // file cannot deliver a value no other path could have stored.
    let meta = a_meta();
    let shades = a_shade_record();
    let estate = an_estate_record();
    let mut header = [0u8; HEADER_LEN];
    write_header(&meta, &a_codes(), &mut header);
    header[7] = 5;
    header[16..21].copy_from_slice(b"a\0bcd");
    let crc = checksum(&header, &shades, &estate);

    let mut bytes = [0u8; BACKUP_LEN];
    bytes[..HEADER_LEN].copy_from_slice(&header);
    bytes[OFF_SHADES..OFF_SHADES + SHADE_RECORD_LEN].copy_from_slice(&shades);
    bytes[OFF_ESTATE..OFF_ESTATE + ESTATE_RECORD_LEN].copy_from_slice(&estate);
    bytes[BACKUP_LEN - 4..].copy_from_slice(&crc.to_le_bytes());

    assert!(matches!(decode(&bytes), Err(BackupError::Credentials(_))));
}

#[test]
fn a_code_block_claiming_more_pairs_than_it_holds_is_refused() {
    let (meta, shades, estate) = (a_meta(), a_shade_record(), an_estate_record());
    let mut header = [0u8; HEADER_LEN];
    write_header(&meta, &a_codes(), &mut header);
    header[9] = MAX_CODES as u8 + 1;
    let bytes = reassemble(&header, &shades, &estate);
    assert_eq!(
        decode(&bytes),
        Err(BackupError::CodeCount(MAX_CODES as u8 + 1)),
    );
}

#[test]
fn a_code_against_an_impossible_address_is_refused_rather_than_skipped() {
    // Skipping it would leave that shade with no code and no warning, which is
    // a shade that will not move and nothing saying why. Refusing the file
    // means the operator sees it.
    for address in [0u32, 0x00FF_FFFF, 0x0100_0000] {
        let (meta, shades, estate) = (a_meta(), a_shade_record(), an_estate_record());
        let mut header = [0u8; HEADER_LEN];
        write_header(&meta, &a_codes(), &mut header);
        header[OFF_CODES..OFF_CODES + 4].copy_from_slice(&address.to_le_bytes());
        let bytes = reassemble(&header, &shades, &estate);
        assert_eq!(decode(&bytes), Err(BackupError::CodeAddress(address)));
    }
}

// ---------------------------------------------------------------------------
// The size the firmware allocates against
// ---------------------------------------------------------------------------

#[test]
fn the_container_is_the_size_the_staging_region_is_sized_for() {
    // 4,420 bytes: a 64-byte header, a 256-byte code block, the two 2,048-byte
    // flash records, and four bytes of checksum. The firmware compares an
    // upload's `Content-Length` against this before it writes a byte of flash,
    // so the figure being fixed is load-bearing rather than incidental.
    assert_eq!(HEADER_LEN, 64 + MAX_CODES * 8);
    assert_eq!(BACKUP_LEN, HEADER_LEN + 2_048 + 2_048 + 4);
    assert_eq!(BACKUP_LEN, 4_420);
    assert_eq!(
        encode(
            &a_meta(),
            &a_codes(),
            &a_shade_record(),
            &an_estate_record()
        )
        .len(),
        BACKUP_LEN,
    );
}

#[test]
fn equal_configurations_encode_identically() {
    // The property every record in this project holds and the reason all of
    // them zero-fill: a write is proved to have landed by comparing bytes, so
    // two encodes of the same value must not differ in padding.
    let first = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    let second = encode(
        &a_meta(),
        &a_codes(),
        &a_shade_record(),
        &an_estate_record(),
    );
    assert_eq!(first, second);
}
