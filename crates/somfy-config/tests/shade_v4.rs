//! The version-4 calibration block: what it stores, what it costs, and what a
//! record written before it existed is taken to mean.
//!
//! # Why the block exists at all
//!
//! On 2026-08-17 three shades were found carrying 10000/10000/7000 — travel
//! times nobody had ever chosen — and a command for 25% open moved one of them
//! about 1%. Two requirements came out of that afternoon and both need a byte
//! that the record did not have:
//!
//! - **R7:** a factory default MUST be surfaced as *uncalibrated*. The API used
//!   to guess that by comparing the value against the default, which made
//!   `measured` unreachable and could not tell an operator who genuinely chose
//!   10 s from one who had never touched the setting.
//! - **R8:** the model MUST carry a per-direction dead band at the closed limit,
//!   measured and per shade.
//!
//! # Why it is a parallel block rather than four more bytes in the entry
//!
//! Because an entry was already full to the byte: 24 bytes of fields plus a
//! 32-byte name is exactly `ENTRY_LEN`. So the block sits at the far end of the
//! record, anchored to the checksum, and everything an older layout knows about
//! stays at the offset it was written to.
//!
//! # What it costs, and the cost is real
//!
//! 128 bytes out of the linked-remote pool, which was "whatever is left over" —
//! so the pool fell from 58 words to 26. There was no third option: 2048 is
//! fixed by the slot buffer standing on the boot stack, and the two alternatives
//! the pool's own docs weigh (a bigger slot, a smaller shade table) are rejected
//! for reasons that have not changed.

use somfy_config::{
    ShadeRecord, ShadeRecordError, StoredShade, MAX_LINKS, SHADE_RECORD_LEN, SHADE_TABLE_CAPACITY,
};
use somfy_domain::{
    CalibrationSource, DomainError, ShadeConfig, FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS,
    FACTORY_UP_TIME_MS,
};
use somfy_rts::RollingCode;

fn stored(name: &str, address: u32) -> StoredShade {
    StoredShade::new(ShadeConfig::new(name, address).unwrap(), RollingCode(1)).unwrap()
}

fn record(shades: &[StoredShade]) -> ShadeRecord {
    let mut table = heapless::Vec::new();
    for shade in shades {
        table.push(shade.clone()).expect("fits");
    }
    ShadeRecord {
        seq: 1,
        announced: somfy_config::Announced::NONE,
        shades: table,
        links: heapless::Vec::new(),
    }
}

/// The estate's own numbers, and the reason each one is here: 30 s up and 27 s
/// down were measured by hand on 2026-08-17, the 4 s band is the slat separation
/// the owner described, and 110 ms is about the air time of a three-frame 56-bit
/// burst.
fn calibrated() -> StoredShade {
    let mut shade = stored("Bedroom window", 0x00_1001);
    shade.config.up_time_ms = 30_000;
    shade.config.down_time_ms = 27_000;
    shade.config.up_time_source = CalibrationSource::Measured;
    shade.config.down_time_source = CalibrationSource::OperatorSupplied;
    shade.config.tilt_time_source = CalibrationSource::FactoryDefault;
    shade.config.start_lag_ms = 110;
    shade.config.vent_band_ms = 4_000;
    shade.config.close_band_ms = 1_500;
    shade
}

#[test]
fn a_calibration_round_trips_through_the_record() {
    let written = record(&[calibrated()]);
    let read = ShadeRecord::decode(&written.encode()).expect("its own output is readable");
    assert_eq!(read, written);

    let config = &read.shades[0].config;
    assert_eq!(config.up_time_source, CalibrationSource::Measured);
    assert_eq!(config.down_time_source, CalibrationSource::OperatorSupplied);
    assert_eq!(config.tilt_time_source, CalibrationSource::FactoryDefault);
    assert_eq!(config.start_lag_ms, 110);
    assert_eq!(config.vent_band_ms, 4_000);
    assert_eq!(config.close_band_ms, 1_500);
}

/// **The state that was unreachable before this block existed.**
///
/// `Measured` could not be produced by a comparison against the default, and a
/// value that merely differed from the default could not be told apart from one
/// a person typed. Both are distinguishable now, and both survive a write.
#[test]
fn measured_and_operator_supplied_are_now_distinguishable_on_flash() {
    let mut typed = stored("Typed", 0x00_2001);
    typed.config.up_time_ms = 30_000;
    typed.config.up_time_source = CalibrationSource::OperatorSupplied;

    let mut swept = stored("Swept", 0x00_2002);
    swept.config.up_time_ms = 30_000;
    swept.config.up_time_source = CalibrationSource::Measured;

    let read = ShadeRecord::decode(&record(&[typed, swept]).encode()).expect("readable");
    assert_eq!(
        read.shades[0].config.up_time_ms,
        read.shades[1].config.up_time_ms
    );
    assert_ne!(
        read.shades[0].config.up_time_source, read.shades[1].config.up_time_source,
        "the same number, and the record still knows which of them was measured",
    );
}

/// And the false positive R7 rules on: a *measured* value that lands exactly on
/// the factory default is still reported as measured, because the record has the
/// fact rather than a guess at it.
#[test]
fn a_measured_value_equal_to_the_factory_default_is_still_measured() {
    let mut shade = stored("Ten seconds exactly", 0x00_3001);
    shade.config.up_time_ms = FACTORY_UP_TIME_MS;
    shade.config.up_time_source = CalibrationSource::Measured;

    let read = ShadeRecord::decode(&record(&[shade]).encode()).expect("readable");
    assert_eq!(
        read.shades[0].config.up_time_source,
        CalibrationSource::Measured
    );
}

/// A packed provenance byte outside the three states is reported rather than
/// defaulted, for the reason every other field in this record is: both plausible
/// defaults are wrong in a way somebody pays for.
#[test]
fn an_unknown_provenance_byte_is_refused_rather_than_defaulted() {
    let mut bytes = record(&[calibrated()]).encode();
    // The calibration block is anchored to the checksum: 32 rows of 4 bytes
    // ending at `OFF_CRC`. Row zero's first byte is the packed provenance.
    let sources = SHADE_RECORD_LEN - 4 - SHADE_TABLE_CAPACITY * 4;
    bytes[sources] = 0x03;
    // Re-checksum, so the failure is the field rather than the CRC.
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());

    assert_eq!(
        ShadeRecord::decode(&bytes),
        Err(ShadeRecordError::Calibration {
            index: 0,
            raw: 0x03,
        }),
    );
}

/// Flash may not deliver a shade whose lag and band leave no travel behind them:
/// the estimator answers that by reporting every move as instantly arrived,
/// which is a shade that claims to be wherever it was last sent.
#[test]
fn a_record_whose_band_eats_its_whole_traverse_is_refused() {
    let mut shade = stored("Impossible", 0x00_4001);
    shade.config.up_time_ms = 3_000;
    shade.config.vent_band_ms = 3_000;

    assert_eq!(
        ShadeRecord::decode(&record(&[shade]).encode()),
        Err(ShadeRecordError::Shade {
            index: 0,
            error: somfy_config::ShadeError::Domain(DomainError::DeadBandTooLong),
        }),
    );
}

/// **A record written before the block existed keeps reporting exactly what it
/// reported yesterday.**
///
/// The provenance is reconstructed by the same comparison the API used to make,
/// so a board upgrading into version 4 sees no change in what it is told; what
/// it gains is the *ability* to say `measured`, which nothing could produce
/// before because nothing could store it.
///
/// The lag and the bands read as zero, and zero is not a guess here: it is the
/// un-compensated linear model, which is precisely what those boards have been
/// running.
#[test]
fn a_record_from_before_the_block_reports_what_it_always_did() {
    // Build a version-4 record, then rewrite its header as version 3 and
    // re-checksum: what that produces is a record with no calibration block, at
    // exactly the offsets version 3 wrote.
    let mut hand_set = stored("Measured by hand", 0x00_5001);
    hand_set.config.up_time_ms = 30_000;
    hand_set.config.down_time_ms = 27_000;
    let untouched = stored("Never touched", 0x00_5002);

    let mut bytes = record(&[hand_set, untouched]).encode();
    bytes[4] = 0x03;
    bytes[5] = 0x00;
    // Blank the block, so nothing is read out of bytes version 3 never wrote.
    let block = SHADE_RECORD_LEN - 4 - SHADE_TABLE_CAPACITY * 4;
    bytes[block..SHADE_RECORD_LEN - 4].fill(0);
    let checksum =
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&bytes[..SHADE_RECORD_LEN - 4]);
    bytes[SHADE_RECORD_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());

    let read = ShadeRecord::decode(&bytes).expect("a version-3 record is still readable");

    let hand = &read.shades[0].config;
    assert_eq!(hand.up_time_ms, 30_000);
    assert_eq!(hand.up_time_source, CalibrationSource::OperatorSupplied);
    assert_eq!(hand.down_time_source, CalibrationSource::OperatorSupplied);
    assert_eq!(
        hand.tilt_time_source,
        CalibrationSource::FactoryDefault,
        "the tilt time was left alone, and is reported per field",
    );

    let fresh = &read.shades[1].config;
    assert_eq!(fresh.up_time_ms, FACTORY_UP_TIME_MS);
    assert_eq!(fresh.down_time_ms, FACTORY_DOWN_TIME_MS);
    assert_eq!(fresh.tilt_time_ms, FACTORY_TILT_TIME_MS);
    assert_eq!(fresh.up_time_source, CalibrationSource::FactoryDefault);
    assert_eq!(fresh.down_time_source, CalibrationSource::FactoryDefault);
    assert_eq!(fresh.tilt_time_source, CalibrationSource::FactoryDefault);

    for shade in &read.shades {
        assert_eq!(shade.config.start_lag_ms, 0);
        assert_eq!(shade.config.vent_band_ms, 0);
        assert_eq!(shade.config.close_band_ms, 0);
    }
}

/// The block is checksummed like everything else, so a later format cannot put
/// a field in its spare bits and have this version accept the record.
#[test]
fn the_calibration_block_is_covered_by_the_checksum() {
    let mut bytes = record(&[calibrated()]).encode();
    // The last row of the block — a shade the table does not have.
    bytes[SHADE_RECORD_LEN - 5] = 0x01;
    assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
}

/// The pool shrank, and this is the figure it shrank to. Asserted here as well
/// as in the crate's own tests because it is the visible cost of the block and
/// the thing a reader will want to check.
#[test]
fn the_block_is_paid_for_out_of_the_linked_remote_pool() {
    assert_eq!(MAX_LINKS, 26);
    // 128 bytes of block is 32 pool words, and 58 - 32 = 26.
    assert_eq!(SHADE_TABLE_CAPACITY * 4, 128);
    assert_eq!(MAX_LINKS + SHADE_TABLE_CAPACITY * 4 / 4, 58);
}
