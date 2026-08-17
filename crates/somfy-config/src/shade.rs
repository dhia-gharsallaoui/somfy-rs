//! The bytes one shade-table slot holds, and what it takes to believe them.
//!
//! Same shape as [`crate::ConfigRecord`] and `somfy_store::Record` — fixed
//! length, magic, version, CRC-32 over the whole thing — because it lives on
//! the same kind of region and fails in the same ways.
//!
//! ## Why a record is the whole table, not one shade
//!
//! For the reason `somfy_store::Record`'s docs give at length: the region is a
//! ring, and a ring wrapping round erases its oldest sector. With one shade per
//! record, that erase could take the only copy of a shade nobody had touched in
//! months, and the shade would simply cease to exist between one boot and the
//! next. A snapshot has exactly one record that matters — the newest valid one
//! — so every older slot is free to be erased whenever the ring reaches it.
//!
//! ## What a shade's id is, and what that costs
//!
//! Nothing here stores an id. The firmware fills an **empty** registry in
//! record order, and `somfy_domain::Registry::add_shade` assigns the lowest
//! free slot, so the first entry is `ShadeId(0)`, the second `ShadeId(1)`, and
//! so on. That is deliberate — the registry's id is the thing `somfy-mqtt`
//! builds `shade_0`, `shade_1` … out of, and an id stored here that the
//! registry did not agree with would be a field that is quietly wrong.
//!
//! **The consequence belongs to whoever writes a record: appending a shade is
//! safe, and reordering or removing one is not.** Removing the first of three
//! shades renumbers the other two, so in Home Assistant they become different
//! entities and the ones they were are left behind as retained orphans. There
//! is no fix for that inside this format — an id the registry cannot honour
//! would not help — so it is stated here and by the provisioning tool, and
//! properly closing it needs `Registry` to be able to take an id, which is a
//! domain change and not a record change.
//!
//! ## The seed is not a preference
//!
//! [`StoredShade::initial_code`] is the **next-to-send** rolling code, and it
//! is used only when the store holds none for that address —
//! `somfy_store::seed_if_absent` is where that rule lives and why. A motor
//! rejects any code at or below the last one it accepted, so a value carried
//! anything other than verbatim is a shade that stops responding.

use heapless::Vec;
use somfy_domain::{DomainError, ShadeConfig, ShadeKind, TiltMode, MAX_SHADES};
use somfy_rts::RollingCode;

/// Shades one record carries: the registry's own capacity, so a table that
/// fits the controller fits the record.
///
/// The record has room to spare at this capacity (see the assertion below), and
/// that is the right way round: the bound a provisioning tool runs into should
/// be the one the controller actually has.
pub const SHADE_TABLE_CAPACITY: usize = MAX_SHADES;

/// Bytes in one shade record, and therefore in one slot of the shade ring.
///
/// 2048 is the smallest power of two that holds a full registry: 32 shades of
/// 56 bytes plus the header and checksum is 1808. It is a whole
/// number of 4-byte flash words and it divides a 4 KB erase sector exactly —
/// two records per sector, four across the two-sector region — which are the
/// two relationships the ring needs.
///
/// **Shades do not fit alongside the Wi-Fi and MQTT settings.** That record is
/// 512 bytes with 228 free after its last field, or four shades' worth; a
/// larger slot for it would strand the credentials already on provisioned
/// boards. So this is a second region with its own ring, written by the same
/// host-side tooling and read by the same code path.
pub const SHADE_RECORD_LEN: usize = 2048;

/// `magic`, `version`, `count`, `seq`.
const HEADER_LEN: usize = 12;
/// One shade: see the offsets below.
const ENTRY_LEN: usize = 56;
const CRC_LEN: usize = 4;

/// Marks a slot as this format's. Spells `RTSS` in a hex dump — RTS Shades —
/// and is deliberately distinct from the rolling-code store's `RTSC` and the
/// device config's `RTSW`, so a region mounted at the wrong offset is reported
/// rather than half-read.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSS");

/// Bumped when the layout below changes. A record carrying a different version
/// is reported as such rather than as damage, so a later implementation can
/// migrate instead of erasing shades it does not recognise.
const VERSION: u16 = 1;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// Header offsets.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_COUNT: usize = 6;
const OFF_SEQ: usize = 8;
const OFF_ENTRIES: usize = HEADER_LEN;
const OFF_CRC: usize = SHADE_RECORD_LEN - CRC_LEN;

// Offsets within one entry. Spelled out rather than computed so the layout can
// be read off the file and compared against a hex dump.
const ENTRY_ADDRESS: usize = 0;
const ENTRY_CODE: usize = 4;
const ENTRY_KIND: usize = 6;
const ENTRY_TILT: usize = 7;
const ENTRY_UP_MS: usize = 8;
const ENTRY_DOWN_MS: usize = 12;
const ENTRY_TILT_MS: usize = 16;
const ENTRY_NAME_LEN: usize = 20;
// 21..24 is padding, so the name starts on a word boundary and a hex dump lines
// up. It is zero-filled and covered by the checksum.
const ENTRY_NAME: usize = 24;

/// Bytes a stored name may occupy — `somfy_domain::ShadeConfig`'s own capacity,
/// which is also `somfy_mqtt::MAX_NAME_LEN`.
const MAX_NAME_LEN: usize = 32;

// The entry has to hold everything it claims to, and a full table has to fit
// between the header and the checksum. Compile-time rather than tests, because
// it is arithmetic over constants.
const _: () = assert!(
    ENTRY_NAME + MAX_NAME_LEN <= ENTRY_LEN,
    "a shade's fields must fit inside one entry"
);
const _: () = assert!(
    OFF_ENTRIES + SHADE_TABLE_CAPACITY * ENTRY_LEN <= OFF_CRC,
    "a full registry must fit inside one record"
);
const _: () = assert!(
    OFF_CRC + CRC_LEN == SHADE_RECORD_LEN,
    "the checksum must occupy the last four bytes of the record"
);

/// Which travel time a [`ShadeError::TravelTimeZero`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TravelField {
    /// Full-travel time toward fully open.
    Up,
    /// Full-travel time toward fully closed.
    Down,
}

impl TravelField {
    /// The field's name, for a message a person reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            TravelField::Up => "up_time_ms",
            TravelField::Down => "down_time_ms",
        }
    }
}

/// Why a shade was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeError {
    /// The domain's own rules: a sentinel address, or a name that does not fit.
    /// Reached by running the value back through
    /// [`ShadeConfig::new`], so this crate never restates them.
    Domain(DomainError),
    /// A full-travel time of zero. The estimator answers that by treating every
    /// move as an instant jump and every Step as a no-op, so the shade would
    /// report positions it never travels to. Refused rather than defaulted: a
    /// substituted travel time is a wrong one, and it would present as a
    /// position estimate that drifts with nothing saying why.
    TravelTimeZero {
        /// Which of the two was zero.
        field: TravelField,
    },
}

/// A sentence an operator can act on, like the other two entered-by-hand
/// errors in this crate. The rules themselves stay where they are enforced —
/// this only says what happened.
impl core::fmt::Display for ShadeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ShadeError::Domain(DomainError::InvalidAddress) => write!(
                f,
                "the radio address is not one a remote can have: 0 and 0xFFFFFF are \
                 reserved, and the field is 24 bits wide"
            ),
            ShadeError::Domain(DomainError::NameTooLong) => {
                write!(f, "the name is longer than the 32 bytes a shade name holds")
            }
            ShadeError::Domain(other) => write!(f, "the shade was refused: {other:?}"),
            ShadeError::TravelTimeZero { field } => write!(
                f,
                "{} may not be zero; it is how long the shade takes to travel end to end, \
                 and the position estimate is computed from it",
                field.as_str(),
            ),
        }
    }
}

impl core::error::Error for ShadeError {}

/// One provisioned shade: everything the registry needs, and the rolling code
/// the store starts from if it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredShade {
    /// Exactly what `somfy_domain::Registry::add_shade` takes.
    pub config: ShadeConfig,
    /// **Next-to-send** rolling code for this address, applied only when the
    /// rolling-code store holds nothing for it. See this module's docs, and
    /// `somfy_store::seed_if_absent`, which is the only thing that should ever
    /// act on it.
    pub initial_code: RollingCode,
}

impl StoredShade {
    /// Check a shade's configuration and pair it with its starting code.
    ///
    /// The address and name rules are the domain's: `config` goes back through
    /// [`ShadeConfig::new`], so a value this accepts is a value
    /// `Registry::add_shade` accepts, and a rule can only be in one place.
    pub fn new(config: ShadeConfig, initial_code: RollingCode) -> Result<StoredShade, ShadeError> {
        // The constructed value is discarded on purpose: what is wanted is the
        // *judgement*, applied to fields that are public and so may have been
        // set to anything since the config was built.
        ShadeConfig::new(&config.name, config.address).map_err(ShadeError::Domain)?;
        for (field, value) in [
            (TravelField::Up, config.up_time_ms),
            (TravelField::Down, config.down_time_ms),
        ] {
            if value == 0 {
                return Err(ShadeError::TravelTimeZero { field });
            }
        }
        Ok(StoredShade {
            config,
            initial_code,
        })
    }
}

/// Why a slot's bytes are not a shade record.
///
/// [`Blank`](ShadeRecordError::Blank) is its own variant for the same reason it
/// is in the other two record formats: an erased slot is the ordinary state of
/// every slot the ring has not reached, and a reader that cannot tell "never
/// written" from "damaged" cannot tell a first boot from data loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeRecordError {
    /// Every byte is erased. The slot has never been written.
    Blank,
    /// Not this format's magic. Foreign data, or a write torn before the
    /// header landed.
    Magic,
    /// The checksum does not match the bytes — a torn write, or bit rot.
    Checksum,
    /// A record of some other version of this format.
    Version(u16),
    /// The header claims more shades than a record can hold.
    Count(u16),
    /// A stored name length does not fit the field it describes. These lengths
    /// come off a device, so they are checked rather than trusted.
    NameLength {
        /// Which entry.
        index: usize,
        /// The length the record claimed.
        len: usize,
    },
    /// A name's bytes are not UTF-8, so they are not a name anything downstream
    /// could show.
    NotUtf8 {
        /// Which entry.
        index: usize,
    },
    /// A shade-kind byte this version does not model. Reported rather than
    /// defaulted to Roller: a drapery silently imported as a roller is a
    /// configuration nobody chose.
    Kind {
        /// Which entry.
        index: usize,
        /// The byte the record carried.
        raw: u8,
    },
    /// A tilt-mode byte this version does not model.
    Tilt {
        /// Which entry.
        index: usize,
        /// The byte the record carried.
        raw: u8,
    },
    /// The entry decoded and the shade it describes would have been refused had
    /// it been entered by hand.
    Shade {
        /// Which entry.
        index: usize,
        /// Why it was refused.
        error: ShadeError,
    },
    /// Two entries name the same radio address, so the record does not say what
    /// that address's travel times or rolling code are — and the registry would
    /// refuse the second one, dropping a shade the operator provisioned.
    DuplicateAddress {
        /// The later of the two entries.
        index: usize,
        /// The address they share.
        address: u32,
    },
}

/// One slot's worth of bytes: a sequence number and the shade table it stamps.
///
/// The sequence number orders records around the ring — the same role, and the
/// same wrapping comparison, as in the other two stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadeRecord {
    /// Monotonic write counter, wrapping at [`u32::MAX`].
    pub seq: u32,
    /// Every provisioned shade, in the order their registry ids will follow.
    /// An empty table is a value an operator can mean, and is not the same fact
    /// as a blank region.
    pub shades: Vec<StoredShade, SHADE_TABLE_CAPACITY>,
}

impl ShadeRecord {
    /// Serialise into the exact bytes a slot holds.
    ///
    /// Everything unused is zero-filled, so equal records produce identical
    /// bytes — which is what lets a writer prove a write landed by reading it
    /// back and comparing — and so a hex dump of flash is readable.
    pub fn encode(&self) -> [u8; SHADE_RECORD_LEN] {
        let mut bytes = [0u8; SHADE_RECORD_LEN];
        bytes[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());
        // Bounded by the vector's own capacity, which is `SHADE_TABLE_CAPACITY`.
        bytes[OFF_COUNT..OFF_COUNT + 2].copy_from_slice(&(self.shades.len() as u16).to_le_bytes());
        bytes[OFF_SEQ..OFF_SEQ + 4].copy_from_slice(&self.seq.to_le_bytes());

        for (index, shade) in self.shades.iter().enumerate() {
            let at = OFF_ENTRIES + index * ENTRY_LEN;
            let entry = &mut bytes[at..at + ENTRY_LEN];
            let config = &shade.config;
            entry[ENTRY_ADDRESS..ENTRY_ADDRESS + 4].copy_from_slice(&config.address.to_le_bytes());
            entry[ENTRY_CODE..ENTRY_CODE + 2].copy_from_slice(&shade.initial_code.0.to_le_bytes());
            entry[ENTRY_KIND] = config.kind as u8;
            entry[ENTRY_TILT] = config.tilt_mode as u8;
            for (offset, value) in [
                (ENTRY_UP_MS, config.up_time_ms),
                (ENTRY_DOWN_MS, config.down_time_ms),
                (ENTRY_TILT_MS, config.tilt_time_ms),
            ] {
                entry[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            // Bounded by `ShadeConfig::name`'s own capacity, which is the field
            // width here.
            let name = config.name.as_bytes();
            entry[ENTRY_NAME_LEN] = name.len() as u8;
            entry[ENTRY_NAME..ENTRY_NAME + name.len()].copy_from_slice(name);
        }

        let checksum = CRC.checksum(&bytes[..OFF_CRC]);
        bytes[OFF_CRC..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Read the header, and nothing else.
    ///
    /// The checksum is verified here, so a header this returns describes bytes
    /// that are whole. What it does **not** do is decode a single shade, which
    /// is the point: a scan of the ring needs each slot's sequence number to
    /// find the newest record, and decoding four tables to compare four `u32`s
    /// costs 2,320 bytes of stack per table on a device where that is a
    /// sixth of the whole stack.
    pub fn header(bytes: &[u8; SHADE_RECORD_LEN]) -> Result<ShadeHeader, ShadeRecordError> {
        if bytes.iter().all(|byte| *byte == 0xFF) {
            return Err(ShadeRecordError::Blank);
        }
        if u32::from_le_bytes(word(bytes, OFF_MAGIC)) != MAGIC {
            return Err(ShadeRecordError::Magic);
        }
        if u32::from_le_bytes(word(bytes, OFF_CRC)) != CRC.checksum(&bytes[..OFF_CRC]) {
            return Err(ShadeRecordError::Checksum);
        }

        let version = u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]);
        if version != VERSION {
            return Err(ShadeRecordError::Version(version));
        }

        let count = u16::from_le_bytes([bytes[OFF_COUNT], bytes[OFF_COUNT + 1]]);
        if count as usize > SHADE_TABLE_CAPACITY {
            return Err(ShadeRecordError::Count(count));
        }

        Ok(ShadeHeader {
            seq: u32::from_le_bytes(word(bytes, OFF_SEQ)),
            count: count as usize,
        })
    }

    /// Hand every shade in `bytes` to `visit`, one at a time.
    ///
    /// **All or nothing: if any entry is refused, nothing is visited at all**,
    /// and the error names the entry. That is not tidiness — a table missing
    /// its third shade is a table whose fourth and fifth shades *change id*,
    /// because the registry assigns the lowest free slot and ids are what Home
    /// Assistant's entities are named after. Loading the survivors would
    /// silently rename half an installation to route around one bad field.
    /// The cost of the rule is a second pass over the same bytes, which are
    /// already in hand.
    ///
    /// Exists next to [`ShadeRecord::decode`] because a caller that only wants
    /// to *place* the shades never has to hold them: one entry is 72 bytes and
    /// a decoded table is 2,320.
    pub fn for_each(
        bytes: &[u8; SHADE_RECORD_LEN],
        mut visit: impl FnMut(usize, StoredShade),
    ) -> Result<ShadeHeader, ShadeRecordError> {
        let header = ShadeRecord::header(bytes)?;

        // First pass: every entry must decode, and no address may repeat.
        // Addresses only — 128 bytes for a full table, against 2,320 to hold
        // the shades themselves.
        let mut seen: [u32; SHADE_TABLE_CAPACITY] = [0; SHADE_TABLE_CAPACITY];
        for index in 0..header.count {
            let shade = decode_entry(entry_at(bytes, index), index)?;
            // A repeated address makes the table ambiguous, and the registry
            // would answer it by refusing the second shade — which is a
            // provisioned shade vanishing with only a log line to say so.
            if seen[..index].contains(&shade.config.address) {
                return Err(ShadeRecordError::DuplicateAddress {
                    index,
                    address: shade.config.address,
                });
            }
            seen[index] = shade.config.address;
        }

        // Second pass, reached only once every entry above was accepted.
        for index in 0..header.count {
            visit(index, decode_entry(entry_at(bytes, index), index)?);
        }
        Ok(header)
    }

    /// Read a slot's bytes back as a whole table, or say precisely why they are
    /// not one.
    ///
    /// The checksum is verified **before** any field is interpreted, so a torn
    /// write is reported as [`ShadeRecordError::Checksum`] rather than as
    /// whatever its half-written header happens to spell.
    pub fn decode(bytes: &[u8; SHADE_RECORD_LEN]) -> Result<ShadeRecord, ShadeRecordError> {
        let mut shades: Vec<StoredShade, SHADE_TABLE_CAPACITY> = Vec::new();
        let header = ShadeRecord::for_each(bytes, |_, shade| {
            // Infallible: `for_each` yields at most `header.count` shades and
            // that is bounded by the capacity.
            let _ = shades.push(shade);
        })?;
        Ok(ShadeRecord {
            seq: header.seq,
            shades,
        })
    }
}

/// What a record says about itself before any shade is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadeHeader {
    /// Monotonic write counter, wrapping at [`u32::MAX`]. What orders records
    /// around the ring.
    pub seq: u32,
    /// Shades the record carries, already checked against the capacity.
    pub count: usize,
}

/// One entry's bytes. Panic-free by construction: `index` is always below a
/// `count` [`ShadeRecord::header`] has bounded by [`SHADE_TABLE_CAPACITY`], and
/// a full table fits (asserted at compile time above).
fn entry_at(bytes: &[u8; SHADE_RECORD_LEN], index: usize) -> &[u8] {
    let at = OFF_ENTRIES + index * ENTRY_LEN;
    &bytes[at..at + ENTRY_LEN]
}

/// One entry's bytes as a shade, or which field was wrong and in which entry.
fn decode_entry(entry: &[u8], index: usize) -> Result<StoredShade, ShadeRecordError> {
    let name_len = entry[ENTRY_NAME_LEN] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ShadeRecordError::NameLength {
            index,
            len: name_len,
        });
    }
    let name = core::str::from_utf8(&entry[ENTRY_NAME..ENTRY_NAME + name_len])
        .map_err(|_| ShadeRecordError::NotUtf8 { index })?;

    let address = u32::from_le_bytes([
        entry[ENTRY_ADDRESS],
        entry[ENTRY_ADDRESS + 1],
        entry[ENTRY_ADDRESS + 2],
        entry[ENTRY_ADDRESS + 3],
    ]);
    let kind = ShadeKind::from_raw(entry[ENTRY_KIND]).ok_or(ShadeRecordError::Kind {
        index,
        raw: entry[ENTRY_KIND],
    })?;
    let tilt_mode = TiltMode::from_raw(entry[ENTRY_TILT]).ok_or(ShadeRecordError::Tilt {
        index,
        raw: entry[ENTRY_TILT],
    })?;

    // Straight back through the domain's constructor, exactly as a
    // hand-provisioned shade goes, so flash cannot deliver a shade the
    // validator would have refused — a sentinel address, or a name too long
    // for the registry to hold.
    let mut config = ShadeConfig::new(name, address).map_err(|error| ShadeRecordError::Shade {
        index,
        error: ShadeError::Domain(error),
    })?;
    config.kind = kind;
    config.tilt_mode = tilt_mode;
    config.up_time_ms = u32::from_le_bytes(entry_word(entry, ENTRY_UP_MS));
    config.down_time_ms = u32::from_le_bytes(entry_word(entry, ENTRY_DOWN_MS));
    config.tilt_time_ms = u32::from_le_bytes(entry_word(entry, ENTRY_TILT_MS));

    let initial_code = RollingCode(u16::from_le_bytes([
        entry[ENTRY_CODE],
        entry[ENTRY_CODE + 1],
    ]));
    StoredShade::new(config, initial_code).map_err(|error| ShadeRecordError::Shade { index, error })
}

/// Four bytes at `at` within one entry. Panic-free by construction: every call
/// site passes a fixed offset within [`ENTRY_LEN`], and `entry` is that long.
fn entry_word(entry: &[u8], at: usize) -> [u8; 4] {
    [entry[at], entry[at + 1], entry[at + 2], entry[at + 3]]
}

/// Four bytes at `at`, as an array. Panic-free by construction: every call site
/// passes a fixed offset within [`SHADE_RECORD_LEN`].
fn word(bytes: &[u8; SHADE_RECORD_LEN], at: usize) -> [u8; 4] {
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shade(name: &str, address: u32, code: u16) -> StoredShade {
        let config = ShadeConfig::new(name, address).expect("valid");
        StoredShade::new(config, RollingCode(code)).expect("valid")
    }

    fn record() -> ShadeRecord {
        let mut shades = Vec::new();
        shades.push(shade("Kitchen", 0x00_1001, 7)).expect("fits");
        shades.push(shade("Salon", 0x00_1002, 9)).expect("fits");
        ShadeRecord { seq: 3, shades }
    }

    /// Re-stamp a record's bytes and re-checksum, as a writer of some other
    /// version of this region would. Every test below needs this, which is why
    /// they are here rather than in `tests/shade.rs`.
    fn tampered(edit: impl FnOnce(&mut [u8; SHADE_RECORD_LEN])) -> [u8; SHADE_RECORD_LEN] {
        let mut bytes = record().encode();
        edit(&mut bytes);
        let checksum = CRC.checksum(&bytes[..OFF_CRC]);
        bytes[OFF_CRC..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Byte `offset` of entry `index`.
    fn field(index: usize, offset: usize) -> usize {
        OFF_ENTRIES + index * ENTRY_LEN + offset
    }

    /// The one thing here that could be silently wrong: whether the chosen CRC
    /// parameters are the ones everyone means by "CRC-32". This is the standard
    /// check value — the checksum of the ASCII digits 1 to 9.
    #[test]
    fn the_checksum_is_the_standard_crc32() {
        assert_eq!(CRC.checksum(b"123456789"), 0xCBF4_3926);
    }

    /// The three magics in this workspace must stay distinct, or a region
    /// mounted at the wrong offset is half-read instead of reported.
    #[test]
    fn the_magic_is_not_another_records_magic() {
        assert_ne!(MAGIC, u32::from_le_bytes(*b"RTSC"));
        assert_ne!(MAGIC, u32::from_le_bytes(*b"RTSW"));
    }

    #[test]
    fn a_record_of_another_version_names_the_version() {
        let bytes = tampered(|bytes| {
            bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&9u16.to_le_bytes())
        });
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Version(9))
        );
    }

    #[test]
    fn a_record_claiming_more_shades_than_fit_is_rejected() {
        let count = SHADE_TABLE_CAPACITY as u16 + 1;
        let bytes =
            tampered(|bytes| bytes[OFF_COUNT..OFF_COUNT + 2].copy_from_slice(&count.to_le_bytes()));
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Count(count))
        );
    }

    #[test]
    fn a_name_longer_than_the_field_is_rejected() {
        let bytes = tampered(|bytes| bytes[field(1, ENTRY_NAME_LEN)] = 200);
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::NameLength { index: 1, len: 200 }),
        );
    }

    #[test]
    fn a_name_that_is_not_utf8_names_the_entry() {
        let bytes = tampered(|bytes| bytes[field(1, ENTRY_NAME)] = 0xFF);
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::NotUtf8 { index: 1 }),
        );
    }

    /// A shade kind this version does not model is reported, not defaulted.
    /// `0x05` is a garage door in the migration format: importing one as a
    /// roller would move a garage door with a shade's travel times.
    #[test]
    fn an_unmodelled_shade_kind_is_rejected() {
        let bytes = tampered(|bytes| bytes[field(0, ENTRY_KIND)] = 0x05);
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Kind {
                index: 0,
                raw: 0x05
            }),
        );
    }

    #[test]
    fn an_unmodelled_tilt_mode_is_rejected() {
        let bytes = tampered(|bytes| bytes[field(0, ENTRY_TILT)] = 0x09);
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Tilt {
                index: 0,
                raw: 0x09
            }),
        );
    }

    /// Decoded values go through exactly the validation hand-entered ones do,
    /// so flash cannot deliver a shade at the sentinel address that
    /// `Registry::add_shade` would refuse.
    #[test]
    fn a_record_carrying_a_sentinel_address_is_refused() {
        let bytes = tampered(|bytes| {
            bytes[field(0, ENTRY_ADDRESS)..field(0, ENTRY_ADDRESS) + 4]
                .copy_from_slice(&0u32.to_le_bytes())
        });
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Shade {
                index: 0,
                error: ShadeError::Domain(DomainError::InvalidAddress),
            }),
        );
    }

    /// And the same for a travel time the estimator cannot use.
    #[test]
    fn a_record_carrying_a_zero_travel_time_is_refused() {
        let bytes = tampered(|bytes| {
            bytes[field(1, ENTRY_DOWN_MS)..field(1, ENTRY_DOWN_MS) + 4]
                .copy_from_slice(&0u32.to_le_bytes())
        });
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Shade {
                index: 1,
                error: ShadeError::TravelTimeZero {
                    field: TravelField::Down,
                },
            }),
        );
    }

    /// The padding inside an entry is checksummed, so a later format cannot put
    /// a field there and have this version accept the record anyway.
    #[test]
    fn entry_padding_is_covered_by_the_checksum() {
        let mut bytes = record().encode();
        bytes[field(0, ENTRY_NAME_LEN) + 1] = 0x01;
        assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
    }

    /// The numbers the docs quote, pinned. That a full table *fits* is asserted
    /// at compile time above; what this adds is that the figures the module and
    /// the partition table are documented with are the figures in force.
    #[test]
    fn a_full_table_is_the_size_the_docs_claim() {
        assert_eq!(ENTRY_LEN, 56);
        assert_eq!(SHADE_TABLE_CAPACITY, 32);
        assert_eq!(
            HEADER_LEN + SHADE_TABLE_CAPACITY * ENTRY_LEN + CRC_LEN,
            1808
        );
    }
}
