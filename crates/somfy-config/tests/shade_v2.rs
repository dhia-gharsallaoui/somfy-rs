//! Reading a shade record written by the build before this one — the version
//! that is actually on the board.
//!
//! # Why this exists next to `shade_v1.rs`
//!
//! Because version 1 is not what the provisioned board is carrying any more.
//! It was upgraded, and the record on it now is a **version 2** one: an
//! announced-shade bitmap, a linked-remote pool, and a per-entry frame width and
//! radio protocol. Version 3 adds the pairing state, and the question that
//! decides whether three working Home Assistant entities survive the next flash
//! is what a v2 entry's byte 23 — zero in every record ever written — is taken
//! to mean.
//!
//! # The fixture is a capture, not a reconstruction
//!
//! Same doctrine as `shade_v1.rs`, and for the same reason: a v2 record this
//! build reconstructs from its own understanding of v2 would test the build
//! against itself and pass however wrong it was. The bytes below were produced
//! by running the **previous build's** `ShadeRecord::encode` and recording every
//! non-zero byte at its offset. The checksum in the last four bytes came off
//! that build, over those bytes, so a fixture that is wrong anywhere fails as
//! [`ShadeRecordError::Checksum`] rather than passing quietly.
//!
//! The addresses in it are synthetic. This repository holds no real radio
//! address.

use somfy_config::{ShadeRecord, ShadeRecordError, SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY};
use somfy_domain::{FrameWidth, PairingState, RadioProtocol, ShadeId, ShadeKind, TiltMode};
use somfy_rts::RollingCode;

/// Every non-zero byte of one real version-2 record, at its offset.
///
/// The table it encodes:
///
/// | row | name | address | next code | kind | tilt | up / down / tilt ms |
/// |---|---|---|---|---|---|---|
/// | 0 | `Kitchen` | `0x00_1001` | 7 | Blind | Integrated | 11000 / 12000 / 7500 |
/// | 1 | `Salon` | `0x00_1002` | 9 | Roller | None | 10000 / 10000 / 7000 |
/// | 2 | `Bureau` | `0x80_0003` | 4113 | Roller | None | 10000 / 10000 / 7000 |
///
/// All three announced (the `0x07` at offset 12), one linked remote at
/// `0x00_2002` on row 1 (the pool word at 1812), and every entry's byte 23 —
/// what version 3 spends on the pairing state — left at zero, because in
/// version 2 it was padding.
const V2_RECORD: &[(usize, u8)] = &[
    // Header: magic `RTSS`, version 2, count 3, seq 3, announced 0b111,
    // link_count 1.
    (0, 0x52),
    (1, 0x54),
    (2, 0x53),
    (3, 0x53),
    (4, 0x02),
    (6, 0x03),
    (8, 0x03),
    (12, 0x07),
    (16, 0x01),
    // Entry 0, at offset 20.
    (20, 0x01),
    (21, 0x10),
    (24, 0x07),
    (26, 0x01),
    (27, 0x02),
    (28, 0xF8),
    (29, 0x2A),
    (32, 0xE0),
    (33, 0x2E),
    (36, 0x4C),
    (37, 0x1D),
    (40, 0x07),
    (41, 0x38),
    (44, 0x4B),
    (45, 0x69),
    (46, 0x74),
    (47, 0x63),
    (48, 0x68),
    (49, 0x65),
    (50, 0x6E),
    // Entry 1, at offset 76.
    (76, 0x02),
    (77, 0x10),
    (80, 0x09),
    (84, 0x10),
    (85, 0x27),
    (88, 0x10),
    (89, 0x27),
    (92, 0x58),
    (93, 0x1B),
    (96, 0x05),
    (97, 0x38),
    (100, 0x53),
    (101, 0x61),
    (102, 0x6C),
    (103, 0x6F),
    (104, 0x6E),
    // Entry 2, at offset 132.
    (132, 0x03),
    (134, 0x80),
    (136, 0x11),
    (137, 0x10),
    (140, 0x10),
    (141, 0x27),
    (144, 0x10),
    (145, 0x27),
    (148, 0x58),
    (149, 0x1B),
    (152, 0x06),
    (153, 0x38),
    (156, 0x42),
    (157, 0x75),
    (158, 0x72),
    (159, 0x65),
    (160, 0x61),
    (161, 0x75),
    // The linked-remote pool: row 1, address 0x00_2002.
    (1812, 0x02),
    (1813, 0x20),
    (1815, 0x01),
    // CRC-32 of everything above, as the previous build computed it.
    (2044, 0x74),
    (2045, 0x91),
    (2046, 0xE6),
    (2047, 0x9E),
];

fn v2_record() -> [u8; SHADE_RECORD_LEN] {
    let mut bytes = [0u8; SHADE_RECORD_LEN];
    for (at, byte) in V2_RECORD {
        bytes[*at] = *byte;
    }
    bytes
}

/// The record on the board still decodes, into the same three shades and the
/// same wall remote it has always described.
#[test]
fn a_record_from_the_previous_build_still_decodes() {
    let record = ShadeRecord::decode(&v2_record()).expect("a version-2 record is still readable");

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
    assert_eq!(kitchen.config.frame_width, FrameWidth::Bits56);
    assert_eq!(kitchen.config.protocol, RadioProtocol::Rts);

    assert_eq!(record.shades[1].config.name.as_str(), "Salon");
    assert_eq!(record.shades[2].config.address, 0x80_0003);
    assert_eq!(record.shades[2].initial_code, RollingCode(4113));

    assert_eq!(record.links.len(), 1);
    assert_eq!(record.links[0].shade, ShadeId(1));
    assert_eq!(record.links[0].address, 0x00_2002);

    let ids: heapless::Vec<ShadeId, SHADE_TABLE_CAPACITY> = record.announced.ids().collect();
    assert_eq!(ids.as_slice(), &[ShadeId(0), ShadeId(1), ShadeId(2)]);
}

/// **The assertion this file exists for.**
///
/// Every entry's byte 23 is zero in this capture, and zero is
/// `AwaitingConfirmation`. Read that way, the three shades on the board stop
/// being announced the moment it is reflashed: three working entities vanish
/// from Home Assistant, every automation pointing at them breaks, and nothing
/// on the device says why. The version gate is what stops that — a table
/// written before the field existed is read as one whose shades an operator has
/// already reported working.
#[test]
fn a_version_two_shade_arrives_confirmed_rather_than_awaiting_confirmation() {
    let bytes = v2_record();
    for entry in 0..3 {
        assert_eq!(
            bytes[20 + entry * 56 + 23],
            0,
            "the capture must really carry a zero where the pairing byte now is, \
             or this test proves nothing",
        );
    }

    let record = ShadeRecord::decode(&bytes).expect("readable");
    for shade in &record.shades {
        assert_eq!(
            shade.config.pairing_state,
            PairingState::ConfirmedByOperator,
            "'{}' would stop being announced",
            shade.config.name,
        );
    }
}

/// The fixture is a capture, and this is what says so.
#[test]
fn the_fixture_carries_the_checksum_the_previous_build_computed() {
    let mut bytes = v2_record();
    bytes[24] ^= 0x01;
    assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
}

/// What the board does on its first runtime write after the upgrade: read v2,
/// change something, write back. The shades survive, the announced set survives,
/// and the confirmation reconstructed on the way in must survive too — a
/// re-encode that wrote it back as `AwaitingConfirmation` would retire the
/// entities one debounce later.
#[test]
fn a_migrated_record_re_encodes_as_the_current_version_and_keeps_its_confirmations() {
    let migrated = ShadeRecord::decode(&v2_record()).expect("readable");
    let bytes = migrated.encode();
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 4);

    let rewritten = ShadeRecord::decode(&bytes).expect("its own output is readable");
    assert_eq!(rewritten, migrated);
    for shade in &rewritten.shades {
        assert_eq!(
            shade.config.pairing_state,
            PairingState::ConfirmedByOperator
        );
    }
}
