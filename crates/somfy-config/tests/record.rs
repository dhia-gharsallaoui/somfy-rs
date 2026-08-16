//! The bytes a config slot holds, and what it takes to believe them.
//!
//! The stakes are lower than `somfy-store`'s — losing this record costs a
//! Wi-Fi connection, not a re-pairing procedure at every shade — but the
//! failure modes are the same ones, so the checks are the same ones. A torn
//! write must be rejected rather than half-read, and a blank slot must be
//! distinguishable from a damaged one.

//! Cases needing a re-checksummed record — a version bump, a tampered length —
//! live in `src/record.rs` instead, where the checksum function is reachable.

use somfy_config::{ConfigRecord, RecordError, WifiCredentials, CONFIG_RECORD_LEN};

fn credentials() -> WifiCredentials {
    WifiCredentials::new("example-network", "PLACEHOLDER_PASSPHRASE").expect("valid")
}

fn record() -> ConfigRecord {
    ConfigRecord {
        seq: 7,
        wifi: Some(credentials()),
    }
}

#[test]
fn a_record_round_trips_through_its_bytes() {
    let bytes = record().encode();
    assert_eq!(ConfigRecord::decode(&bytes), Ok(record()));
}

/// "No Wi-Fi configured" is a value, not the absence of a record. Without it
/// there is no way to clear credentials: the newest record would always be the
/// last one that had some.
#[test]
fn a_record_can_say_that_no_network_is_configured() {
    let cleared = ConfigRecord { seq: 8, wifi: None };
    assert_eq!(ConfigRecord::decode(&cleared.encode()), Ok(cleared));
}

/// An erased slot is the ordinary state of every slot the ring has not reached.
/// Reporting it as damage would make a first boot indistinguishable from data
/// loss, which is exactly what the config-integrity spec forbids.
#[test]
fn an_erased_slot_is_blank_not_damaged() {
    let erased = [0xFFu8; CONFIG_RECORD_LEN];
    assert_eq!(ConfigRecord::decode(&erased), Err(RecordError::Blank));
}

/// A record whose first bytes never landed is foreign data, not a checksum
/// failure — and saying which it is, is the difference between "somebody else
/// wrote here" and "our write was interrupted".
#[test]
fn foreign_bytes_are_reported_as_a_magic_mismatch() {
    let mut bytes = record().encode();
    bytes[0] ^= 0xFF;
    assert_eq!(ConfigRecord::decode(&bytes), Err(RecordError::Magic));
}

/// Every truncation point of a write must be rejected. This is the torn write,
/// walked exhaustively rather than sampled: flash programs word by word, so
/// power can be lost at any word boundary and each one has to fail.
#[test]
fn every_truncation_of_a_write_is_rejected() {
    let complete = record().encode();
    for landed in (0..CONFIG_RECORD_LEN).step_by(4) {
        let mut torn = [0xFFu8; CONFIG_RECORD_LEN];
        torn[..landed].copy_from_slice(&complete[..landed]);
        assert!(
            ConfigRecord::decode(&torn).is_err(),
            "a write of {landed} bytes decoded as a record",
        );
    }
    // The whole thing having landed is not a truncation and must still decode.
    assert!(ConfigRecord::decode(&complete).is_ok());
}

/// Bit rot anywhere in the record, including the parts no field reads today,
/// must fail rather than be interpreted. A reserved byte that is not
/// checksummed is a field a later format cannot add safely.
#[test]
fn every_single_bit_flip_is_rejected() {
    let complete = record().encode();
    for byte in 0..CONFIG_RECORD_LEN {
        for bit in 0..8 {
            let mut damaged = complete;
            damaged[byte] ^= 1 << bit;
            if damaged == complete {
                continue;
            }
            assert!(
                ConfigRecord::decode(&damaged).is_err(),
                "flipping bit {bit} of byte {byte} still decoded",
            );
        }
    }
}

/// Equal records encode identically, so a store can prove a write landed by
/// comparing bytes, and a hex dump of flash is readable rather than full of
/// whatever was on the stack.
#[test]
fn equal_records_encode_to_identical_bytes() {
    assert_eq!(record().encode(), record().encode());
    let padding = &record().encode()[..];
    assert!(
        padding.iter().filter(|byte| **byte == 0).count() > CONFIG_RECORD_LEN / 2,
        "unused space should be zero-filled",
    );
}
