//! The on-flash record: what survives a torn write and what does not.
//!
//! Every assertion here is really a statement about power loss. A record is
//! written to NOR flash a word at a time; losing power part-way leaves some
//! words programmed and the rest erased. The store's whole safety argument is
//! that such a record is *rejected*, so the previous complete record stays
//! newest and the motor's pairing survives.

use somfy_rts::RollingCode;
use somfy_store::{CodeTable, Record, RecordError, TableError, MAX_CODES, RECORD_LEN};

fn table(entries: &[(u32, u16)]) -> CodeTable {
    let mut table = CodeTable::new();
    for (address, code) in entries {
        table
            .set(*address, RollingCode(*code))
            .expect("test table fits");
    }
    table
}

fn record(seq: u32, entries: &[(u32, u16)]) -> Record {
    Record {
        seq,
        table: table(entries),
    }
}

#[test]
fn a_record_round_trips_through_its_encoding() {
    let original = record(7, &[(0x00_C0DE, 42), (0x0F_C155, 9)]);
    assert_eq!(Record::decode(&original.encode()), Ok(original));
}

#[test]
fn an_empty_table_round_trips() {
    let original = record(0, &[]);
    assert_eq!(Record::decode(&original.encode()), Ok(original));
}

#[test]
fn a_full_table_round_trips() {
    let entries: Vec<(u32, u16)> = (0..MAX_CODES).map(|i| (i as u32, i as u16)).collect();
    let original = record(u32::MAX, &entries);
    assert_eq!(original.table.len(), MAX_CODES);
    assert_eq!(Record::decode(&original.encode()), Ok(original));
}

#[test]
fn a_table_rejects_one_address_too_many() {
    let mut table = CodeTable::new();
    for i in 0..MAX_CODES {
        table.set(i as u32, RollingCode(1)).expect("fits");
    }
    assert_eq!(table.set(0xFFFF, RollingCode(1)), Err(TableError::Full));
    // ...but replacing an address already present is not an insertion.
    assert_eq!(table.set(0, RollingCode(99)), Ok(()));
    assert_eq!(table.get(0), Some(RollingCode(99)));
    assert_eq!(table.len(), MAX_CODES);
}

#[test]
fn a_table_rejects_an_address_wider_than_24_bits() {
    let mut table = CodeTable::new();
    assert_eq!(
        table.set(0x0100_0000, RollingCode(1)),
        Err(TableError::Address(0x0100_0000))
    );
    assert!(table.is_empty());
}

#[test]
fn setting_an_address_twice_replaces_rather_than_appends() {
    let mut table = table(&[(0x00_C0DE, 1)]);
    table.set(0x00_C0DE, RollingCode(2)).expect("replace");
    assert_eq!(table.len(), 1);
    assert_eq!(table.get(0x00_C0DE), Some(RollingCode(2)));
}

#[test]
fn an_address_with_no_entry_reads_back_as_none() {
    let table = table(&[(0x00_C0DE, 5)]);
    assert_eq!(table.get(0x00_C0DF), None);
    assert_eq!(CodeTable::new().get(0x00_C0DE), None);
}

#[test]
fn a_table_reports_every_entry_it_holds() {
    let table = table(&[(0x00_C0DE, 5), (0x01_0203, 6)]);
    let entries: Vec<(u32, RollingCode)> = table.iter().collect();
    assert_eq!(
        entries,
        vec![(0x00_C0DE, RollingCode(5)), (0x01_0203, RollingCode(6))]
    );
}

/// An erased NOR flash slot reads as all-ones. That is the ordinary state of
/// every slot the ring has not reached yet, so it must be reported as its own
/// fact rather than as corruption — a store that cannot tell "never written"
/// from "damaged" cannot tell first boot from data loss either, which is
/// exactly what `docs/specs/2026-08-15-config-integrity-requirements.md` R1
/// forbids.
#[test]
fn an_erased_slot_is_blank_not_corrupt() {
    assert_eq!(Record::decode(&[0xFF; RECORD_LEN]), Err(RecordError::Blank));
}

#[test]
fn an_all_zero_slot_is_not_mistaken_for_a_record() {
    assert_eq!(Record::decode(&[0x00; RECORD_LEN]), Err(RecordError::Magic));
}

/// The decisive test. A torn write is a prefix of the intended bytes with the
/// tail still erased. Every truncation point must be rejected: if any of them
/// decoded, a half-written record could outrank the complete one before it and
/// the controller would resume from a rolling code the motor has already seen.
#[test]
fn every_torn_write_prefix_is_rejected() {
    let bytes = record(3, &[(0x00_C0DE, 1000)]).encode();

    for programmed in 0..RECORD_LEN {
        // A tail that was going to be 0xFF anyway is not actually torn — the
        // erased state and the intended state are the same bytes, so the slot
        // holds the complete record and accepting it is right.
        if bytes[programmed..].iter().all(|byte| *byte == 0xFF) {
            continue;
        }
        let mut torn = [0xFFu8; RECORD_LEN];
        torn[..programmed].copy_from_slice(&bytes[..programmed]);
        assert!(
            Record::decode(&torn).is_err(),
            "a record torn after {programmed} bytes decoded as valid"
        );
    }

    // The un-torn record — every byte programmed — is of course fine.
    assert!(Record::decode(&bytes).is_ok());
}

/// Flash programming only ever clears bits, so a write into a slot that was
/// not erased first ANDs the new bytes into the old ones, and a weak cell can
/// leave a single bit unprogrammed. Those records must be rejected too, or a
/// botched geometry change would quietly resurrect a mangled counter.
#[test]
fn every_single_bit_that_fails_to_program_is_caught() {
    let bytes = record(3, &[(0x00_C0DE, 1000)]).encode();

    for byte in 0..RECORD_LEN {
        for bit in 0..8 {
            let mut damaged = bytes;
            damaged[byte] ^= 1 << bit;
            assert!(
                Record::decode(&damaged).is_err(),
                "flipping bit {bit} of byte {byte} still decoded as valid"
            );
        }
    }
}

/// Two tables holding the same addresses are the same table, whatever is in
/// the unused entries behind them. The flash store's durability check compares
/// a decoded record against the one it wrote, so an equality that could depend
/// on unreachable bytes would make that check depend on them too.
#[test]
fn table_equality_ignores_everything_past_the_live_entries() {
    let mut first = CodeTable::new();
    first.set(0x00_C0DE, RollingCode(1)).expect("fits");
    first.set(0x00_BEEF, RollingCode(2)).expect("fits");

    // The same two entries, but reached by writing and overwriting a third
    // address first, so the arrays behind `len` took a different route.
    let mut second = CodeTable::new();
    second.set(0x00_C0DE, RollingCode(9)).expect("fits");
    second.set(0x00_BEEF, RollingCode(9)).expect("fits");
    second.set(0x00_C0DE, RollingCode(1)).expect("replace");
    second.set(0x00_BEEF, RollingCode(2)).expect("replace");

    assert_eq!(first, second);
}

#[test]
fn tables_differing_in_a_live_entry_are_not_equal() {
    assert_ne!(table(&[(1, 1)]), table(&[(1, 2)]));
    assert_ne!(table(&[(1, 1)]), table(&[(2, 1)]));
    assert_ne!(table(&[(1, 1)]), table(&[(1, 1), (2, 2)]));
    // Order is part of the value, not incidental: it is the order the entries
    // are written to flash in, and the store's durability check compares a
    // decoded record against the one it encoded.
    assert_ne!(table(&[(1, 1), (2, 2)]), table(&[(2, 2), (1, 1)]));
}

#[test]
fn a_record_from_another_format_is_rejected_by_its_magic() {
    let mut bytes = record(1, &[]).encode();
    bytes[0] ^= 0xFF;
    assert_eq!(Record::decode(&bytes), Err(RecordError::Magic));
}

/// The record has to sit whole inside a flash write unit and tile an erase
/// unit exactly, or a slot would straddle a sector boundary and one erase
/// would take half of two records. The firmware asserts the same relationship
/// against `esp-storage`'s own constants; this pins the value that assertion
/// compares against.
#[test]
fn the_record_length_tiles_flash_write_and_erase_units() {
    assert_eq!(
        RECORD_LEN % 4,
        0,
        "not a whole number of 4-byte flash words"
    );
    assert_eq!(4096 % RECORD_LEN, 0, "does not tile a 4 KB erase sector");
}
