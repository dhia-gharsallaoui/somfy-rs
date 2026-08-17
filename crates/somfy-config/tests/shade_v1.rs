//! Reading a shade record written by the build before this one.
//!
//! # Why this test is bytes and not a constructor
//!
//! A provisioned board is carrying a version-1 record right now. Its boot
//! prints three shades, and those three shades are the only thing standing
//! between their owner and a walk to each motor with a screwdriver. A format
//! change that cannot read that record does not fail loudly — it reports
//! `damaged`, loads nothing, and the shades are simply gone.
//!
//! So the fixture below is **not** a v1 record this test reconstructs from this
//! build's understanding of v1. It is a byte-for-byte capture of what the
//! previous build's `ShadeRecord::encode` actually emitted, taken by running
//! that build, recorded as the sparse list of its non-zero bytes (a v1 record
//! is 2048 bytes and all but sixty of them are zero). A reconstruction would
//! test this build against itself and pass however wrong it was; a capture
//! cannot, and the **checksum proves it** — the four bytes at the end were
//! computed by the old code over the old bytes, so a fixture that is wrong
//! anywhere fails as [`ShadeRecordError::Checksum`] rather than passing
//! quietly.
//!
//! The addresses in it are synthetic. This repository holds no real radio
//! address.

use somfy_config::{
    Announced, ShadeRecord, ShadeRecordError, SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY,
};
use somfy_domain::{FrameWidth, PairingState, RadioProtocol, ShadeId, ShadeKind, TiltMode};
use somfy_rts::RollingCode;

/// Every non-zero byte of one real version-1 record, at its offset.
///
/// The table it encodes:
///
/// | row | name | address | next code | kind | tilt | up / down / tilt ms |
/// |---|---|---|---|---|---|---|
/// | 0 | `Kitchen` | `0x00_1001` | 7 | Blind | Integrated | 11000 / 12000 / 7500 |
/// | 1 | `Salon` | `0x00_1002` | 9 | Roller | None | 10000 / 10000 / 7000 |
/// | 2 | `Bureau` | `0x80_0003` | 4113 | Roller | None | 10000 / 10000 / 7000 |
///
/// Row 0 carries a non-default value in every field that has a default, so a
/// migration that silently substituted one would be caught. Row 2 sits in the
/// allocator's own address space and rows 0 and 1 do not, which is the
/// distinction the pairing button is gated on.
const V1_RECORD: &[(usize, u8)] = &[
    // Header: magic `RTSS`, version 1, count 3, seq 3. No announced word — that
    // is what version 2 added, and adding it is what moved the entries.
    (0, 0x52),
    (1, 0x54),
    (2, 0x53),
    (3, 0x53),
    (4, 0x01),
    (6, 0x03),
    (8, 0x03),
    // Entry 0, at offset 12.
    (12, 0x01),
    (13, 0x10),
    (16, 0x07),
    (18, 0x01),
    (19, 0x02),
    (20, 0xF8),
    (21, 0x2A),
    (24, 0xE0),
    (25, 0x2E),
    (28, 0x4C),
    (29, 0x1D),
    (32, 0x07),
    (36, 0x4B),
    (37, 0x69),
    (38, 0x74),
    (39, 0x63),
    (40, 0x68),
    (41, 0x65),
    (42, 0x6E),
    // Entry 1, at offset 68.
    (68, 0x02),
    (69, 0x10),
    (72, 0x09),
    (76, 0x10),
    (77, 0x27),
    (80, 0x10),
    (81, 0x27),
    (84, 0x58),
    (85, 0x1B),
    (88, 0x05),
    (92, 0x53),
    (93, 0x61),
    (94, 0x6C),
    (95, 0x6F),
    (96, 0x6E),
    // Entry 2, at offset 124.
    (124, 0x03),
    (126, 0x80),
    (128, 0x11),
    (129, 0x10),
    (132, 0x10),
    (133, 0x27),
    (136, 0x10),
    (137, 0x27),
    (140, 0x58),
    (141, 0x1B),
    (144, 0x06),
    (148, 0x42),
    (149, 0x75),
    (150, 0x72),
    (151, 0x65),
    (152, 0x61),
    (153, 0x75),
    // CRC-32 of everything above, as the previous build computed it.
    (2044, 0x4B),
    (2045, 0x19),
    (2046, 0x40),
    (2047, 0x8E),
];

fn v1_record() -> [u8; SHADE_RECORD_LEN] {
    let mut bytes = [0u8; SHADE_RECORD_LEN];
    for (at, byte) in V1_RECORD {
        bytes[*at] = *byte;
    }
    bytes
}

/// The whole point: the record on the board still decodes, into the same three
/// shades it has always described.
#[test]
fn a_record_from_the_previous_build_still_decodes() {
    let record = ShadeRecord::decode(&v1_record()).expect("a version-1 record is still readable");

    assert_eq!(record.seq, 3);
    assert_eq!(record.shades.len(), 3);

    let kitchen = &record.shades[0];
    assert_eq!(kitchen.config.name.as_str(), "Kitchen");
    assert_eq!(kitchen.config.address, 0x00_1001);
    assert_eq!(kitchen.initial_code, RollingCode(7));
    assert_eq!(kitchen.config.kind, ShadeKind::Blind);
    assert_eq!(kitchen.config.tilt_mode, TiltMode::Integrated);
    assert_eq!(kitchen.config.up_time_ms, 11_000);
    assert_eq!(kitchen.config.down_time_ms, 12_000);
    assert_eq!(kitchen.config.tilt_time_ms, 7_500);

    assert_eq!(record.shades[1].config.name.as_str(), "Salon");
    assert_eq!(record.shades[1].config.address, 0x00_1002);
    assert_eq!(record.shades[1].initial_code, RollingCode(9));

    assert_eq!(record.shades[2].config.name.as_str(), "Bureau");
    assert_eq!(record.shades[2].config.address, 0x80_0003);
    assert_eq!(record.shades[2].initial_code, RollingCode(4113));
}

/// The three fields version 1 had no bytes for.
///
/// The radio settings decode to what such a shade has always been driven as,
/// because 56-bit RTS is the only thing this firmware has ever transmitted —
/// **not** to whatever the padding bytes happened to hold, which for a v1 record
/// is zero and would mean a zero-bit frame.
///
/// The pairing state is the same shape of judgement and the sharper one: zero
/// would be `AwaitingConfirmation`, and reading it that way un-announces every
/// shade on a board that is working today.
#[test]
fn a_version_one_shade_arrives_as_the_settings_it_was_always_driven_with() {
    let record = ShadeRecord::decode(&v1_record()).expect("readable");
    for shade in &record.shades {
        assert_eq!(shade.config.frame_width, FrameWidth::Bits56);
        assert_eq!(shade.config.protocol, RadioProtocol::Rts);
        assert_eq!(
            shade.config.pairing_state,
            PairingState::ConfirmedByOperator
        );
    }
}

/// A version-1 record is read as having announced every shade it holds.
///
/// The alternative — "nothing was announced" — reintroduces the exact failure
/// the bitmap exists to prevent: a shade removed on the first boot after the
/// upgrade would leave its retained discovery config on the broker with nothing
/// able to name it again. This direction is wrong only for a board that never
/// reached a broker, and being wrong that way costs one zero-length publish to
/// a topic holding nothing.
#[test]
fn a_version_one_record_is_read_as_having_announced_what_it_holds() {
    let record = ShadeRecord::decode(&v1_record()).expect("readable");
    let ids: heapless::Vec<ShadeId, SHADE_TABLE_CAPACITY> = record.announced.ids().collect();
    assert_eq!(ids.as_slice(), &[ShadeId(0), ShadeId(1), ShadeId(2)]);
}

/// The fixture is a capture, and this is what says so: the checksum came off
/// the previous build, so any byte of the sparse list being wrong shows up
/// here rather than as a plausible wrong shade.
#[test]
fn the_fixture_carries_the_checksum_the_previous_build_computed() {
    let mut bytes = v1_record();
    // Move one byte of the *table* and the record must stop verifying, which
    // is only true if the stored checksum was computed over these exact bytes
    // by something other than this test.
    bytes[16] ^= 0x01;
    assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
}

/// Re-encoding a migrated record writes the current version, and that version
/// round-trips.
///
/// This is what a board does on its first runtime write after the upgrade: it
/// reads v1, changes something, and writes back. The shades must survive that
/// unchanged, and the announced set must survive it too — it was reconstructed
/// once, on the way in, and must not be reconstructed again from a table that
/// has since lost a row.
#[test]
fn a_migrated_record_re_encodes_as_the_current_version_and_round_trips() {
    let migrated = ShadeRecord::decode(&v1_record()).expect("readable");
    let rewritten = ShadeRecord::decode(&migrated.encode()).expect("its own output is readable");
    assert_eq!(rewritten, migrated);

    // And the header now really is the current version: the entries have moved
    // since v1, so a record that still claimed version 1 would decode its own
    // bytes as garbage rather than as itself.
    let bytes = migrated.encode();
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 3);
    assert_eq!(
        u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        migrated.announced.bits(),
    );
}

/// A version this build has no reader for is still reported as a version, not
/// as damage — the promise the version field was added to keep.
#[test]
fn a_version_from_the_future_is_named_rather_than_treated_as_damage() {
    let mut bytes = v1_record();
    bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::Version(99)),
    );
}

/// Version 0 is not a version anything wrote, and it is refused rather than
/// being taken for the first layout by an off-by-one.
#[test]
fn version_zero_is_not_the_first_layout() {
    let mut bytes = v1_record();
    bytes[4..6].copy_from_slice(&0u16.to_le_bytes());
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::Version(0))
    );
}

/// An empty version-1 record — a table an operator deliberately provisioned
/// with nothing in it — announces nothing, rather than announcing shade 0.
#[test]
fn an_empty_version_one_record_announces_nothing() {
    let mut bytes = [0u8; SHADE_RECORD_LEN];
    bytes[0..4].copy_from_slice(b"RTSS");
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());

    let record = ShadeRecord::decode(&bytes).expect("an empty table is a table");
    assert!(record.shades.is_empty());
    assert_eq!(record.announced, Announced::NONE);
}
