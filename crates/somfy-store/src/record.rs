//! The bytes a rolling-code store puts in one slot, and what it takes to
//! believe them.
//!
//! ## Why a record is a whole table, not one address
//!
//! A slot holds a **snapshot of every address's next-to-send code**, not a
//! single address's. That is what makes the ring safe to erase.
//!
//! Consider the alternative. With one address per record, a ring wrapping
//! round erases the oldest sector, and the newest record for a rarely-used
//! shade may be the only thing in it. That shade's rolling code is then gone,
//! and the user pays for it with a physical re-pairing procedure — the exact
//! cost the whole persist-before-transmit design exists to avoid. Carrying
//! those records forward means compaction, which means more writes and more
//! ways to be interrupted half-way.
//!
//! A snapshot has none of that. Exactly one record matters — the newest valid
//! one — so every older slot is free to be erased whenever the ring reaches it,
//! and there is nothing to compact.
//!
//! ## Why a checksum, and what it is really for
//!
//! Losing power part-way through a write leaves some words programmed and the
//! rest erased. That record must be **rejected**, so the complete record before
//! it stays newest. A CRC-32 over the record's own bytes is the validity
//! marker: a torn record almost certainly fails it, a blank slot is recognised
//! before the check even runs, and `tests/record.rs` walks every truncation
//! point and every single-bit failure to confirm both.

use somfy_rts::RollingCode;

/// Widest value an RTS remote address can take — the wire field is 24 bits.
///
/// A storage bound, not a domain rule: `somfy-domain` additionally rejects the
/// reserved sentinels 0 and `0xFFFFFF`, and that judgement stays there. All
/// this constant says is that a wider number cannot have come from a remote,
/// so a record carrying one is not a record.
const MAX_ADDRESS: u32 = 0x00FF_FFFF;

/// Bytes in one record, and therefore in one ring slot.
///
/// 256 divides a 4 KB flash sector exactly and is a whole number of 4-byte
/// flash words, which is what lets a slot be written without disturbing its
/// neighbours and erased without taking half of one. `SectorRing::new` refuses
/// any geometry where that is not true, and the firmware asserts the same
/// relationship against `esp-storage`'s own constants at compile time.
///
/// It is also exactly one SPI NOR **page**. Laid on a sector-aligned region,
/// every record is therefore a single whole-page program, and every page is
/// programmed once between erases — no partial-page programming anywhere, which
/// is the pattern flash endurance figures are quoted for.
pub const RECORD_LEN: usize = 256;

/// Addresses one record can carry.
///
/// Whatever is left of [`RECORD_LEN`] once the header and checksum are paid
/// for. A commit for a new address beyond this fails loudly rather than
/// evicting one — silently dropping a rolling code is the failure this store
/// exists to prevent, and a controller with more than 30 paired remotes is a
/// problem worth being told about.
pub const MAX_CODES: usize = (RECORD_LEN - HEADER_LEN - CRC_LEN) / ENTRY_LEN;

/// `magic`, `version`, `count`, `seq`.
const HEADER_LEN: usize = 12;
/// `address`, `code`, padding.
const ENTRY_LEN: usize = 8;
const CRC_LEN: usize = 4;

/// Marks a slot as this format's. Spells `RTSC` in a hex dump, which is the
/// point: someone staring at raw flash should be able to tell what wrote it.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSC");

/// Bumped when the layout below changes. A record carrying a different version
/// is reported as such rather than as damage, so a later implementation can
/// migrate instead of treating every old record as corruption.
const VERSION: u16 = 1;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

/// Why [`CodeTable::set`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableError {
    /// The table already holds [`MAX_CODES`] addresses and this is a new one.
    Full,
    /// Not a 24-bit RTS remote address.
    Address(u32),
}

/// Every address's next-to-send rolling code, as one record carries them.
///
/// Entries past `len` are unreachable and are not part of the value; see the
/// hand-written [`PartialEq`].
#[derive(Debug, Clone, Copy, Eq)]
pub struct CodeTable {
    entries: [(u32, u16); MAX_CODES],
    len: usize,
}

/// Compares the live entries only.
///
/// Not derived, because the derived version would compare all [`MAX_CODES`]
/// array slots including the ones behind `len`. Those are unreachable through
/// every method here, so two tables that differ only there are the same table —
/// but the flash store's write-verify compares a decoded record against the one
/// it encoded, and an equality that could depend on unreachable bytes would
/// quietly make **durability** depend on them too. The invariant that keeps the
/// derived version correct (`new` zero-fills, entries are append-only) is real,
/// but it lives elsewhere; this makes the property local.
impl PartialEq for CodeTable {
    fn eq(&self, other: &Self) -> bool {
        self.entries[..self.len] == other.entries[..other.len]
    }
}

impl Default for CodeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeTable {
    /// An empty table — no addresses known, which is a different fact from
    /// every address reading zero.
    pub const fn new() -> Self {
        Self {
            entries: [(0, 0); MAX_CODES],
            len: 0,
        }
    }

    /// The next-to-send code for `address`, or `None` if the table has no
    /// entry for it.
    pub fn get(&self, address: u32) -> Option<RollingCode> {
        self.entries[..self.len]
            .iter()
            .find(|(stored, _)| *stored == address)
            .map(|(_, code)| RollingCode(*code))
    }

    /// Record `code` as next-to-send for `address`, replacing any existing
    /// entry.
    pub fn set(&mut self, address: u32, code: RollingCode) -> Result<(), TableError> {
        if address > MAX_ADDRESS {
            return Err(TableError::Address(address));
        }
        if let Some(entry) = self.entries[..self.len]
            .iter_mut()
            .find(|(stored, _)| *stored == address)
        {
            entry.1 = code.0;
            return Ok(());
        }
        if self.len == MAX_CODES {
            return Err(TableError::Full);
        }
        self.entries[self.len] = (address, code.0);
        self.len += 1;
        Ok(())
    }

    /// Addresses the table holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the table holds no addresses at all.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Every `(address, next-to-send code)` pair, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, RollingCode)> + '_ {
        self.entries[..self.len]
            .iter()
            .map(|(address, code)| (*address, RollingCode(*code)))
    }
}

/// One slot's worth of bytes: a sequence number and the table it stamps.
///
/// The sequence number is what orders records around the ring; see
/// [`crate::newest_slot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// Monotonic write counter, wrapping at [`u32::MAX`].
    pub seq: u32,
    /// Every address's next-to-send code at the moment this record was written.
    pub table: CodeTable,
}

/// Why a slot's bytes are not a record.
///
/// [`Blank`](RecordError::Blank) is deliberately its own variant and not folded
/// into the rest: an erased slot is the ordinary state of every slot the ring
/// has not reached yet, and a store that cannot tell "never written" from
/// "damaged" cannot tell a first boot from data loss either — which is what
/// `docs/specs/2026-08-15-config-integrity-requirements.md` R1 forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Every byte is erased. The slot has never been written.
    Blank,
    /// Not this format's magic. Foreign data, or a write torn before the
    /// header landed.
    Magic,
    /// The checksum does not match the bytes — a torn write, or bit rot.
    Checksum,
    /// A record of some other version of this format.
    Version(u16),
    /// The header claims more entries than a record can hold.
    Count(u16),
    /// An entry carries something that is not a 24-bit RTS address.
    Address(u32),
    /// Two entries name the same address, so the record does not say what that
    /// address's code is. Unreachable through [`CodeTable::set`], which
    /// replaces rather than appends; these bytes came from somewhere else.
    DuplicateAddress(u32),
}

impl Record {
    /// Serialise into the exact bytes a slot holds.
    ///
    /// Unused entry slots are zero-filled rather than left arbitrary, so two
    /// equal records always produce identical bytes and a hex dump of flash is
    /// readable.
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut bytes = [0u8; RECORD_LEN];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&(self.table.len as u16).to_le_bytes());
        bytes[8..12].copy_from_slice(&self.seq.to_le_bytes());

        for (index, (address, code)) in self.table.iter().enumerate() {
            let at = HEADER_LEN + index * ENTRY_LEN;
            bytes[at..at + 4].copy_from_slice(&address.to_le_bytes());
            bytes[at + 4..at + 6].copy_from_slice(&code.0.to_le_bytes());
            // Two padding bytes, left zero. They are covered by the checksum,
            // so a future field here cannot be mistaken for an old record.
        }

        let checksum = CRC.checksum(&bytes[..RECORD_LEN - CRC_LEN]);
        bytes[RECORD_LEN - CRC_LEN..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Read a slot's bytes back, or say precisely why they are not a record.
    ///
    /// The checksum is verified **before** any field is interpreted, so a torn
    /// write is reported as [`RecordError::Checksum`] rather than as whatever
    /// nonsense its half-written header happens to spell.
    pub fn decode(bytes: &[u8; RECORD_LEN]) -> Result<Record, RecordError> {
        if bytes.iter().all(|byte| *byte == 0xFF) {
            return Err(RecordError::Blank);
        }
        if u32::from_le_bytes(word(bytes, 0)) != MAGIC {
            return Err(RecordError::Magic);
        }

        let stored = u32::from_le_bytes(word(bytes, RECORD_LEN - CRC_LEN));
        if stored != CRC.checksum(&bytes[..RECORD_LEN - CRC_LEN]) {
            return Err(RecordError::Checksum);
        }

        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != VERSION {
            return Err(RecordError::Version(version));
        }

        let count = u16::from_le_bytes([bytes[6], bytes[7]]);
        if count as usize > MAX_CODES {
            return Err(RecordError::Count(count));
        }

        let mut table = CodeTable::new();
        for index in 0..count as usize {
            let at = HEADER_LEN + index * ENTRY_LEN;
            let address = u32::from_le_bytes(word(bytes, at));
            let code = RollingCode(u16::from_le_bytes([bytes[at + 4], bytes[at + 5]]));
            // A repeated address would make `set` replace rather than append,
            // so the record would decode to fewer entries than it claims and
            // re-encode to different bytes — which the flash store's
            // write-verify would then reject with nothing to explain it. The
            // record simply does not say what that address's code is; refuse it.
            if table.get(address).is_some() {
                return Err(RecordError::DuplicateAddress(address));
            }
            // `set` is the only way in, so a decoded table obeys exactly the
            // same rules as one built by hand — including rejecting an address
            // too wide to be an RTS remote. `Full` is unreachable: `count` was
            // bounded above.
            table.set(address, code).map_err(|error| match error {
                TableError::Address(address) => RecordError::Address(address),
                TableError::Full => RecordError::Count(count),
            })?;
        }

        Ok(Record {
            seq: u32::from_le_bytes(word(bytes, 8)),
            table,
        })
    }
}

/// Four bytes at `at`, as an array. Panic-free by construction: every call site
/// passes a fixed offset within [`RECORD_LEN`].
fn word(bytes: &[u8; RECORD_LEN], at: usize) -> [u8; 4] {
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-stamp a record's bytes with a different version and re-checksum, as
    /// a future writer of this region would.
    fn with_version(record: &Record, version: u16) -> [u8; RECORD_LEN] {
        let mut bytes = record.encode();
        bytes[4..6].copy_from_slice(&version.to_le_bytes());
        re_checksum(&mut bytes);
        bytes
    }

    fn re_checksum(bytes: &mut [u8; RECORD_LEN]) {
        let checksum = CRC.checksum(&bytes[..RECORD_LEN - CRC_LEN]);
        bytes[RECORD_LEN - CRC_LEN..].copy_from_slice(&checksum.to_le_bytes());
    }

    fn record() -> Record {
        let mut table = CodeTable::new();
        table.set(0x00_C0DE, RollingCode(42)).expect("fits");
        Record { seq: 3, table }
    }

    /// The one thing in this file that could be silently wrong: whether the
    /// chosen CRC parameters are the ones everyone means by "CRC-32". This is
    /// the standard check value — the checksum of the ASCII digits 1 to 9.
    #[test]
    fn the_checksum_is_the_standard_crc32() {
        assert_eq!(CRC.checksum(b"123456789"), 0xCBF4_3926);
    }

    /// A future format bump must be recognisable as one. Reporting `Version`
    /// and not `Checksum` is what lets a later implementation migrate rather
    /// than silently treat every old record as damage.
    #[test]
    fn a_record_of_another_version_names_the_version() {
        assert_eq!(
            Record::decode(&with_version(&record(), u16::MAX)),
            Err(RecordError::Version(u16::MAX))
        );
        assert_eq!(
            Record::decode(&with_version(&record(), 0)),
            Err(RecordError::Version(0))
        );
    }

    #[test]
    fn a_record_claiming_more_entries_than_fit_is_rejected() {
        let mut bytes = record().encode();
        let count = MAX_CODES as u16 + 1;
        bytes[6..8].copy_from_slice(&count.to_le_bytes());
        re_checksum(&mut bytes);
        assert_eq!(Record::decode(&bytes), Err(RecordError::Count(count)));
    }

    #[test]
    fn a_record_carrying_an_impossible_address_is_rejected() {
        let mut bytes = record().encode();
        bytes[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        re_checksum(&mut bytes);
        assert_eq!(
            Record::decode(&bytes),
            Err(RecordError::Address(0xDEAD_BEEF))
        );
    }

    #[test]
    fn a_record_naming_the_same_address_twice_is_rejected() {
        let mut table = CodeTable::new();
        table.set(0x00_C0DE, RollingCode(1)).expect("fits");
        table.set(0x00_BEEF, RollingCode(2)).expect("fits");
        let mut bytes = Record { seq: 1, table }.encode();

        // Point the second entry at the first entry's address.
        let (first, second) = (HEADER_LEN, HEADER_LEN + ENTRY_LEN);
        let address = word(&bytes, first);
        bytes[second..second + 4].copy_from_slice(&address);
        re_checksum(&mut bytes);

        assert_eq!(
            Record::decode(&bytes),
            Err(RecordError::DuplicateAddress(0x00_C0DE))
        );
    }

    /// The padding inside an entry is checksummed, so a later format cannot
    /// put a field there and have this version accept the record anyway.
    #[test]
    fn entry_padding_is_covered_by_the_checksum() {
        let mut bytes = record().encode();
        bytes[HEADER_LEN + 6] = 0x01;
        assert_eq!(Record::decode(&bytes), Err(RecordError::Checksum));
    }

    /// The header arithmetic has to leave the entries exactly filling the
    /// record; a mismatch would silently waste or overrun bytes.
    #[test]
    fn the_entries_fill_the_record_exactly() {
        assert_eq!(HEADER_LEN + MAX_CODES * ENTRY_LEN + CRC_LEN, RECORD_LEN);
        assert_eq!(MAX_CODES, 30);
    }
}
