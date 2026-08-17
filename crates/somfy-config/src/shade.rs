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
//! so on. The registry's id is the thing `somfy-mqtt` builds `shade_0`,
//! `shade_1` … out of, so a field stored here that the registry did not agree
//! with would be quietly wrong.
//!
//! **So the consequence belongs to whoever writes a record: appending a shade
//! is safe, and reordering or removing one is not.** Removing the first of
//! three shades renumbers the other two, so in Home Assistant they become
//! different entities and the ones they were are left behind as retained
//! orphans. It is stated here and by the provisioning tool because the format
//! cannot fix it alone.
//!
//! ### What has changed, and what has not
//!
//! The half that was missing is no longer missing: `Registry` now has
//! `add_shade_with_id`, which places a shade at an id the caller names and
//! refuses a duplicate or out-of-range one rather than substituting a
//! different slot. An id stored here would now be a field the registry *can*
//! honour.
//!
//! What has not changed is that **the row's position is still the shade's id**.
//! The firmware fills an empty registry in record order, so row 0 is
//! `ShadeId(0)`; nothing stores an id explicitly. That is what
//! [`ShadeRecord::announced`] is indexed by, and it is why a reorder is still
//! unsafe: it would move the announced bits off the shades they describe.
//! Storing the id is a further `VERSION` bump, not a reinterpretation of the
//! remaining padding byte in each entry — that byte is zero in every record
//! ever written, so a reader that took it for an id would read every shade as
//! `ShadeId(0)`. The version field exists precisely so a reader reports a
//! record it does not understand instead of half-reading it.
//!
//! ## Versions, and the board in the field
//!
//! There are three, and all three are read. Version 1 has a 12-byte header and
//! no per-shade radio settings; version 2 adds the announced-shade bitmap to the
//! header — which moves the entries down four bytes — and gives each entry a
//! frame width and a radio protocol in bytes it already had as padding. Version
//! 3 spends the last of those padding bytes on
//! [`somfy_domain::PairingState`], and moves nothing.
//!
//! Version 1 is not a historical curiosity: a provisioned board is carrying one
//! right now, and refusing it would make its shades vanish at the next boot.
//! [`Layout`] is the whole of the difference, and
//! `tests/shade_v1.rs` decodes a byte-for-byte copy of a record the previous
//! build wrote rather than one this build reconstructs.
//!
//! ### What an older record's shades are taken to be
//!
//! **Confirmed.** A version 1 or 2 record has no pairing byte, and the two
//! candidate answers are not symmetric — the same asymmetry that decided the
//! announced-shade bitmap's own migration, one field along.
//!
//! Read as *awaiting confirmation*, every shade on the board that upgrades to
//! this build stops being announced: three working Home Assistant entities
//! vanish, the automations pointing at them break, and nothing on the device
//! says why. Read as *confirmed by the operator*, the only shade that is
//! wrongly described is one that was provisioned and never actually paired —
//! and that shade was already announced and already not moving, so the reading
//! changes nothing about it except that the operator is not invited to finish a
//! setup they may already have finished. One direction breaks what works; the
//! other leaves an existing wrong thing exactly as wrong as it was.
//!
//! ## The seed is not a preference
//!
//! [`StoredShade::initial_code`] is the **next-to-send** rolling code, and it
//! is used only when the store holds none for that address —
//! `somfy_store::seed_if_absent` is where that rule lives and why. A motor
//! rejects any code at or below the last one it accepted, so a value carried
//! anything other than verbatim is a shade that stops responding.

use heapless::Vec;
use somfy_domain::{
    CalibrationSource, DomainError, FrameWidth, PairingState, RadioProtocol, ShadeConfig, ShadeId,
    ShadeKind, TiltMode, DEAD_BAND_RESOLUTION_MS, FACTORY_DOWN_TIME_MS, FACTORY_TILT_TIME_MS,
    FACTORY_UP_TIME_MS, MAX_SHADES, START_LAG_RESOLUTION_MS,
};
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
/// 56 bytes, the 20-byte header and the checksum come to 1816, and the 232
/// bytes left over are [`MAX_LINKS`] linked-remote words — so the record is
/// full to the byte. It is a whole number of 4-byte flash words and it divides
/// a 4 KB erase sector exactly — two records per sector, four across the
/// two-sector region — which are the two relationships the ring needs.
///
/// **Shades do not fit alongside the Wi-Fi and MQTT settings.** That record is
/// 512 bytes with 228 free after its last field, or four shades' worth; a
/// larger slot for it would strand the credentials already on provisioned
/// boards. So this is a second region with its own ring, written by the same
/// host-side tooling and read by the same code path.
pub const SHADE_RECORD_LEN: usize = 2048;

/// `magic`, `version`, `count`, `seq`, `announced`, `link_count`, padding.
const HEADER_LEN: usize = 20;
/// What [`VERSION_ANNOUNCED`] replaced: `magic`, `version`, `count`, `seq`.
const HEADER_LEN_V1: usize = 12;
/// One shade: see the offsets below. Unchanged across both versions — the two
/// new fields went into padding bytes the entry already had.
const ENTRY_LEN: usize = 56;
const CRC_LEN: usize = 4;

/// Marks a slot as this format's. Spells `RTSS` in a hex dump — RTS Shades —
/// and is deliberately distinct from the rolling-code store's `RTSC` and the
/// device config's `RTSW`, so a region mounted at the wrong offset is reported
/// rather than half-read.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSS");

/// The version this build writes.
///
/// Bumped when the layout below changes. A record carrying a version this build
/// has no reader for is reported as such rather than as damage, so a later
/// implementation can migrate instead of erasing shades it does not recognise —
/// and [`VERSION_INITIAL`] is what that promise was written for.
const VERSION: u16 = 4;

/// The first layout, still readable: no `announced` word, entries at
/// [`HEADER_LEN_V1`], and no frame width or protocol in an entry.
///
/// **A board in the field is carrying one of these right now.** Refusing it
/// would make three real shades vanish at the next boot, so it is decoded, not
/// rejected — see [`decode_entry`] for the two values substituted and why they
/// are the only honest choices.
const VERSION_INITIAL: u16 = 1;

/// The version that added the announced-shade bitmap and the per-shade frame
/// width and protocol.
const VERSION_ANNOUNCED: u16 = 2;

/// The version that added [`somfy_domain::PairingState`] to each entry.
///
/// It takes the entry's last padding byte, so nothing moved and both older
/// layouts are still read at their own offsets. See this module's docs for what
/// a record without the byte is taken to mean.
const VERSION_PAIRING: u16 = 3;

/// The version that added the per-shade calibration block: where each travel
/// time came from, the start lag, and the two dead bands at the closed limit.
///
/// It is the first change here that could not be made inside an entry, because
/// an entry was already full to the byte — 24 bytes of fields and a 32-byte
/// name is exactly [`ENTRY_LEN`]. So it is a parallel array instead, and it is
/// paid for out of the linked-remote pool: see [`MAX_LINKS`].
const VERSION_CALIBRATION: u16 = 4;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// Header offsets.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_COUNT: usize = 6;
const OFF_SEQ: usize = 8;
/// The announced-shade bitmap. New in [`VERSION_ANNOUNCED`], which is why the
/// entries moved from [`HEADER_LEN_V1`] to [`HEADER_LEN`].
const OFF_ANNOUNCED: usize = 12;
/// How many words of the linked-remote pool are live. New in
/// [`VERSION_ANNOUNCED`]. Bytes 18..20 are padding, so the entries start on a
/// word boundary.
const OFF_LINK_COUNT: usize = 16;
const OFF_ENTRIES: usize = HEADER_LEN;
/// The linked-remote pool: everything between the last entry and the checksum.
const OFF_LINKS: usize = OFF_ENTRIES + SHADE_TABLE_CAPACITY * ENTRY_LEN;
const OFF_CRC: usize = SHADE_RECORD_LEN - CRC_LEN;

/// Bytes one pool word occupies.
const LINK_LEN: usize = 4;

/// Linked remotes one record can carry, **across every shade**.
///
/// # Why the bound is shared rather than per-shade
///
/// Because a per-shade bound of 7 does not fit and shrinking the shade table to
/// make it fit would be the wrong trade. The arithmetic, since it is the whole
/// argument: a slot is 2048 bytes, an entry is 56, and 32 entries with seven
/// 4-byte addresses each would be `20 + 32 * (56 + 28) + 4 = 2712` bytes.
///
/// The three ways out, and why this one:
///
/// - **A bigger slot.** 4096 bytes would hold it, and the ring can be re-carved
///   into two slots of that size. But a slot's bytes are read into a stack
///   buffer, and on the tightest chip this builds for the whole main stack is
///   14,588 bytes with an 8,016-byte state machine already standing in it. A
///   4 KB buffer there is not affordable, and the existing 2 KB one is already
///   the largest single thing on that stack.
/// - **A smaller shade table.** `20 + 24 * 84 + 4 = 2040` fits, at 24 shades
///   instead of 32 — and 32 is not a preference, it is
///   [`somfy_domain::MAX_SHADES`], sized to what a deployed configuration can
///   contain. A record that could no longer hold a full registry would break
///   the property that a table fitting the controller fits the record.
/// - **A shared pool**, which is this. Every shade keeps the domain's own bound
///   of [`MAX_LINKED_REMOTES`]; what is bounded here is the total, and the bound
///   is exactly the space left over — so the record is full to the byte and the
///   figure cannot drift from the layout that produced it.
///
/// **This is a real limit, not a theoretical one:** 32 shades with two wall
/// remotes each is 64 links and does not fit. It is refused at the push rather
/// than dropped at the write, which is this crate's posture everywhere else.
///
/// # It shrank from 58 to 26 at version 4, and that is a real cost
///
/// "Whatever is left over" got smaller: the calibration block took 128 bytes,
/// and it had to, because an entry was already full to the byte and the values
/// it carries are what R7 and R8 of the position-accuracy requirements make
/// mandatory. There was no third option — the two rejected above are rejected
/// for the same reasons they always were, and 2048 is fixed by the slot buffer
/// standing on the boot stack.
///
/// What it costs, precisely: a record written by an older build that holds more
/// than 26 links **cannot be re-written by this one** and is refused with
/// [`ShadeRecordError::LinkCount`] rather than truncated — this crate does not
/// silently drop a wall remote, because a dropped link is an estimate that
/// stops being corrected and nothing says why. No such record has ever been
/// written: the pool arrived one version ago, and the estate it arrived for has
/// three shades.
///
/// **What would give it back:** a 4096-byte slot, which the module docs above
/// price against the stack, or moving the pool into a region of its own.
pub const MAX_LINKS: usize = OFF_CALIBRATION.saturating_sub(OFF_LINKS) / LINK_LEN;

/// What [`MAX_LINKS`] was before [`VERSION_CALIBRATION`] took 128 bytes for the
/// calibration block: the pool ran all the way to the checksum.
///
/// Kept so that a record written by an older build is read against **its own**
/// bound rather than this build's, which is the same rule the entry offsets
/// already follow.
const MAX_LINKS_V3: usize = (OFF_CRC - OFF_LINKS) / LINK_LEN;

/// Bytes one shade's calibration occupies. See [`CAL_SOURCES`] for the layout.
const CAL_LEN: usize = 4;

/// The calibration block: one [`CAL_LEN`]-byte row per registry slot, sitting at
/// the far end of the record so that everything an older layout knows about
/// stays exactly where it was.
///
/// Anchored to [`OFF_CRC`] rather than to the entries for that reason: growing
/// it eats the linked-remote pool from the top, and moves nothing else.
const OFF_CALIBRATION: usize = OFF_CRC - SHADE_TABLE_CAPACITY * CAL_LEN;

// Offsets within one calibration row.
/// Where each of the three travel times came from, two bits each: up in bits
/// 0-1, down in 2-3, tilt in 4-5. Bits 6-7 are spare and written zero.
///
/// Packed rather than a byte each because the row is paid for out of the
/// linked-remote pool at 32 rows a byte, and this is the one field that
/// compresses without losing anything: the values are a three-way choice.
const CAL_SOURCES: usize = 0;
/// Start lag, in units of `somfy_domain::START_LAG_RESOLUTION_MS`.
const CAL_START_LAG: usize = 1;
/// Slat-separation band, in units of `somfy_domain::DEAD_BAND_RESOLUTION_MS`.
const CAL_VENT_BAND: usize = 2;
/// Slat-compression band, in the same units.
const CAL_CLOSE_BAND: usize = 3;

/// Bits one packed [`CalibrationSource`] occupies in [`CAL_SOURCES`].
const CAL_SOURCE_BITS: u32 = 2;
/// Mask for one packed [`CalibrationSource`].
const CAL_SOURCE_MASK: u8 = (1 << CAL_SOURCE_BITS) - 1;

/// Linked remotes **one shade** may have — the domain's own bound, restated so
/// a record cannot deliver a shade [`Shade::link_remote`](somfy_domain::Shade::link_remote) would then refuse.
pub const MAX_LINKED_REMOTES: usize = somfy_domain::MAX_LINKED_REMOTES;

/// Bits of a pool word the address occupies. An RTS address is 24 bits and
/// [`ShadeConfig::new`] refuses anything at or above `0xFF_FFFF`, so the top
/// byte is free to carry the row the link belongs to — which is what lets one
/// word be one link.
const LINK_ADDRESS_MASK: u32 = 0x00FF_FFFF;

// Offsets within one entry. Spelled out rather than computed so the layout can
// be read off the file and compared against a hex dump. **Every one of them is
// the same in both versions**, which is what makes the migration a change of
// starting offset rather than a second decoder.
const ENTRY_ADDRESS: usize = 0;
const ENTRY_CODE: usize = 4;
const ENTRY_KIND: usize = 6;
const ENTRY_TILT: usize = 7;
const ENTRY_UP_MS: usize = 8;
const ENTRY_DOWN_MS: usize = 12;
const ENTRY_TILT_MS: usize = 16;
const ENTRY_NAME_LEN: usize = 20;
/// New in [`VERSION_ANNOUNCED`], in a byte [`VERSION_INITIAL`] left zero — so a
/// v1 record read with v2 entry rules would claim a zero-bit frame, which is
/// exactly why the version gate decides which rules apply rather than the bytes.
const ENTRY_WIDTH: usize = 21;
/// New in [`VERSION_ANNOUNCED`], and a byte where zero happens to be the right
/// answer ([`RadioProtocol::Rts`]) — which is a coincidence, not a licence to
/// read it out of a v1 record.
const ENTRY_PROTOCOL: usize = 22;
/// New in [`VERSION_PAIRING`], in the entry's last padding byte — so the name
/// still starts on a word boundary and nothing an older record holds moved.
///
/// Zero in every version 1 and 2 record ever written, and zero is
/// [`PairingState::AwaitingConfirmation`], which is the **wrong** answer for
/// such a record: it would hide three working shades. That is precisely why the
/// version gate decides whether to read this byte at all, rather than the byte
/// deciding for itself.
const ENTRY_PAIRING: usize = 23;
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
// The pool is defined as "whatever is left", so this is the statement that
// nothing is left over and nothing overlaps the checksum.
const _: () = assert!(
    OFF_LINKS + MAX_LINKS * LINK_LEN == OFF_CALIBRATION,
    "the linked-remote pool must fill the record exactly, up to the calibration block"
);
// The calibration block is anchored to the checksum and sized by the table, so
// "it reaches the checksum" is true by construction and would be a tautology to
// assert. What is *not* automatic is that it leaves a pool behind it, and that
// the pool is still worth having — the block grows downward into it, so a wider
// row eats links a shade later cannot store.
//
// The floor is one shade's full complement rather than "more than nothing": a
// record that could not hold `MAX_LINKED_REMOTES` for a single shade would
// refuse a link the domain accepts, which is the class of disagreement this
// whole file exists to prevent.
const _: () = assert!(
    OFF_LINKS <= OFF_CALIBRATION,
    "the calibration block must not overlap the linked-remote pool"
);
const _: () = assert!(
    MAX_LINKS >= MAX_LINKED_REMOTES,
    "the calibration block has eaten so much of the linked-remote pool that one \
     shade can no longer hold the remotes the domain lets it link"
);
// Three two-bit fields have to fit the byte that packs them.
const _: () = assert!(
    3 * CAL_SOURCE_BITS <= u8::BITS,
    "a calibration row packs three sources into one byte"
);
// A pool word carries the row in its top byte, so the table cannot be wider
// than a byte — and it is not, but the word says so rather than the reader
// assuming it.
const _: () = assert!(
    SHADE_TABLE_CAPACITY <= u8::MAX as usize,
    "a link word carries its shade's row index in one byte"
);
// The older layout still has to fit, because this build still reads it.
const _: () = assert!(
    HEADER_LEN_V1 + SHADE_TABLE_CAPACITY * ENTRY_LEN <= OFF_CRC,
    "a full registry must fit inside a record of the first layout too"
);
// `Announced` is one bit per registry slot in a `u32`, so a registry wider than
// 32 slots would silently stop recording the announcement of its last shades.
const _: () = assert!(
    SHADE_TABLE_CAPACITY <= u32::BITS as usize,
    "the announced-shade bitmap has one bit per shade and no more"
);

/// Which shades this device has published Home Assistant entities for.
///
/// # Why the record has to hold this at all
///
/// A discovery config is published **retained**, so it outlives the device that
/// published it. Remove a shade and the broker keeps its config forever, Home
/// Assistant keeps showing an entity nothing is behind, and the only cure is an
/// MQTT client and a person — the requirements behind this were written after
/// deleting 49 retained topics by hand.
///
/// Removing it properly means publishing a zero-length retained payload to each
/// of its topics, and **that needs the id of a shade that no longer exists**. A
/// device that only ever records what exists cannot name what it has lost. So
/// the announcement is recorded next to the table it was made from, and a boot
/// that finds a bit set for a slot no shade occupies knows exactly which
/// entities to clear.
///
/// # Why a bare `u32` and not a flag-set crate
///
/// [`bitflags`](https://crates.io/crates/bitflags) 2.13.1 is the obvious
/// candidate and is about as well adopted as a Rust crate gets — 1,678,874,973
/// downloads (374,002,223 recent), 1,152 stars, 96 contributors, last commit
/// 2026-07-16, `no_std` without `alloc` by default, MIT OR Apache-2.0 and so
/// compatible with this project's GPL-3.0-only. `enumset` 1.1.14 and
/// `enumflags2` 0.7.12 are the same shape with less adoption.
///
/// **All three model a set of *named* flags, and these bits have no names.**
/// Bit *n* means registry slot *n*: there is no `const READ = 1 << 0;` to write,
/// because the thing being written would be thirty-two identical lines naming
/// `SHADE_0` … `SHADE_31`, and the one operation that matters — "is the bit for
/// *this* [`ShadeId`] set?" — is an index, which a flag-set type deliberately
/// does not offer. What is left after removing the naming is `1 << n`, and the
/// safety that matters is not the bit arithmetic but the **bound**: a
/// [`ShadeId`] outside the registry must not shift past the word. That check is
/// this type's whole content and no flag crate performs it, because a flag
/// crate has no ids to bound.
///
/// Worth reopening if the announced set ever becomes per-shade and per-entity —
/// [`somfy_mqtt::Component`](https://docs.rs/) *is* a named set, and `enumset`
/// with `repr = "u32"` and the default `map = "lsb"` pins a flash-stable layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Announced(u32);

impl Announced {
    /// Nothing has been announced. What a freshly provisioned table holds.
    pub const NONE: Announced = Announced(0);

    /// Reconstruct from the stored word.
    ///
    /// Bits above the registry's capacity are **dropped**, not rejected: they
    /// name slots this build has no shade for and no topic to clear, so
    /// carrying them would only let a later `bits()` write back a claim this
    /// build cannot act on.
    pub const fn from_bits(bits: u32) -> Announced {
        // `SHADE_TABLE_CAPACITY <= 32` is asserted above, and the shift is
        // written so that a capacity of exactly 32 does not overflow.
        Announced(bits & (u32::MAX >> (u32::BITS as usize - SHADE_TABLE_CAPACITY)))
    }

    /// The word as stored.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether `id`'s entities have been published.
    ///
    /// An id past the registry's capacity is always `false`: it names no slot,
    /// so nothing could have announced it.
    pub const fn contains(self, id: ShadeId) -> bool {
        match Announced::mask(id) {
            Some(mask) => self.0 & mask != 0,
            None => false,
        }
    }

    /// The same set with `id` recorded as announced. An out-of-range id is
    /// ignored rather than shifted past the end of the word.
    pub const fn with(self, id: ShadeId) -> Announced {
        match Announced::mask(id) {
            Some(mask) => Announced(self.0 | mask),
            None => self,
        }
    }

    /// The same set with `id` recorded as no longer announced.
    pub const fn without(self, id: ShadeId) -> Announced {
        match Announced::mask(id) {
            Some(mask) => Announced(self.0 & !mask),
            None => self,
        }
    }

    /// Every id in the set, ascending.
    pub fn ids(self) -> impl Iterator<Item = ShadeId> {
        (0..SHADE_TABLE_CAPACITY as u8)
            .map(ShadeId)
            .filter(move |id| self.contains(*id))
    }

    /// The bit `id` occupies, or `None` if it names no registry slot.
    const fn mask(id: ShadeId) -> Option<u32> {
        if (id.0 as usize) < SHADE_TABLE_CAPACITY {
            Some(1u32 << id.0)
        } else {
            None
        }
    }
}

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
    /// The header claims more linked remotes than the pool holds.
    LinkCount(u16),
    /// A pool word names a row the record does not have. Reported rather than
    /// skipped: the link belongs to *some* shade, and guessing which would
    /// attach a wall remote to the wrong motor's position estimate.
    LinkShade {
        /// Which pool word.
        index: usize,
        /// The row it named.
        shade: u8,
    },
    /// A pool word the domain would refuse — a sentinel address, the shade's
    /// own address, a duplicate, or more than [`MAX_LINKED_REMOTES`] for one
    /// shade.
    Link {
        /// Which pool word.
        index: usize,
        /// Why it was refused, in the domain's own terms.
        error: DomainError,
    },
    /// A frame-width byte that is neither of the protocol's two widths.
    /// Reported rather than defaulted to 56 for the same reason a shade kind
    /// is: a motor paired at the other width is deaf to every frame the
    /// substituted value would produce, and nothing would say why.
    Width {
        /// Which entry.
        index: usize,
        /// The byte the record carried.
        raw: u8,
    },
    /// A radio-protocol byte this version does not model.
    Protocol {
        /// Which entry.
        index: usize,
        /// The byte the record carried.
        raw: u8,
    },
    /// A pairing-state byte outside the two
    /// [`somfy_domain::PairingState`] has.
    ///
    /// Reported rather than defaulted, and this is the one place in the record
    /// where *both* defaults are bad: "awaiting" hides a shade that works and
    /// "confirmed" announces one that does not. Refusing the record sends the
    /// operator to re-provision, which is the only action that resolves the
    /// ambiguity rather than picking a side of it.
    Pairing {
        /// Which entry.
        index: usize,
        /// The byte the record carried.
        raw: u8,
    },
    /// A calibration row's packed provenance byte holds a value outside the
    /// three [`CalibrationSource`] has.
    ///
    /// Reported rather than defaulted for the same reason the pairing byte is:
    /// both plausible defaults are wrong in a way somebody pays for. Calling a
    /// measured value "factory default" invites an operator to re-run a
    /// calibration that was already right; calling a factory default "measured"
    /// is the exact misreport that let a 25%-open command move a shade 1% and
    /// took an afternoon to diagnose.
    Calibration {
        /// Which entry.
        index: usize,
        /// The packed byte the record carried.
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
    /// Which shades this device has published Home Assistant entities for.
    ///
    /// Deliberately **not** derived from `shades`: the whole value of the field
    /// is the case where the two disagree, which is a shade that was announced
    /// and has since been removed. See [`Announced`].
    pub announced: Announced,
    /// Every provisioned shade, in the order their registry ids will follow.
    /// An empty table is a value an operator can mean, and is not the same fact
    /// as a blank region.
    pub shades: Vec<StoredShade, SHADE_TABLE_CAPACITY>,
    /// Every wall remote that drives a shade's position estimate.
    ///
    /// A flat pool rather than a field on each shade, bounded by
    /// [`MAX_LINKS`] — see that constant for the arithmetic and for the two
    /// alternatives it rules out. The bound being on the vector rather than on
    /// the encoder is what keeps [`ShadeRecord::encode`] infallible: a table
    /// with more links than the record can hold is refused at the `push`, the
    /// same way a thirty-third shade is.
    pub links: Vec<LinkedRemote, MAX_LINKS>,
}

/// One wall remote, and the shade whose estimate its frames drive.
///
/// Stored as a single word: the address in the low 24 bits, the shade's row in
/// the top 8. That is not packing for its own sake — an RTS address *is* 24
/// bits, and [`ShadeConfig::new`] refuses anything at or above `0xFF_FFFF`, so
/// the top byte was never going to carry address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkedRemote {
    /// The row in [`ShadeRecord::shades`] this remote belongs to — which is
    /// also the [`ShadeId`] that row will take.
    pub shade: ShadeId,
    /// The remote's 24-bit address.
    pub address: u32,
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
        bytes[OFF_ANNOUNCED..OFF_ANNOUNCED + 4]
            .copy_from_slice(&self.announced.bits().to_le_bytes());
        // Bounded by the vector's own capacity, which is `MAX_LINKS`.
        bytes[OFF_LINK_COUNT..OFF_LINK_COUNT + 2]
            .copy_from_slice(&(self.links.len() as u16).to_le_bytes());

        for (index, link) in self.links.iter().enumerate() {
            let at = OFF_LINKS + index * LINK_LEN;
            let word = ((link.shade.0 as u32) << 24) | (link.address & LINK_ADDRESS_MASK);
            bytes[at..at + LINK_LEN].copy_from_slice(&word.to_le_bytes());
        }

        for (index, shade) in self.shades.iter().enumerate() {
            let at = OFF_ENTRIES + index * ENTRY_LEN;
            let entry = &mut bytes[at..at + ENTRY_LEN];
            let config = &shade.config;
            entry[ENTRY_ADDRESS..ENTRY_ADDRESS + 4].copy_from_slice(&config.address.to_le_bytes());
            entry[ENTRY_CODE..ENTRY_CODE + 2].copy_from_slice(&shade.initial_code.0.to_le_bytes());
            entry[ENTRY_KIND] = config.kind as u8;
            entry[ENTRY_TILT] = config.tilt_mode as u8;
            entry[ENTRY_WIDTH] = config.frame_width as u8;
            entry[ENTRY_PROTOCOL] = config.protocol as u8;
            entry[ENTRY_PAIRING] = config.pairing_state as u8;
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

            // The calibration row, in the parallel block at the far end of the
            // record. Same index as the entry; see `OFF_CALIBRATION` for why it
            // is not part of the entry.
            let at = OFF_CALIBRATION + index * CAL_LEN;
            let row = &mut bytes[at..at + CAL_LEN];
            row[CAL_SOURCES] = pack_sources(config);
            // Exact divisions: both values are rounded onto their resolution at
            // every boundary they enter through (`somfy_domain::round_start_lag_ms`
            // and `round_dead_band_ms`), which is what keeps what the device is
            // running equal to what a reboot would load.
            row[CAL_START_LAG] = (config.start_lag_ms as u32 / START_LAG_RESOLUTION_MS) as u8;
            row[CAL_VENT_BAND] = (config.vent_band_ms as u32 / DEAD_BAND_RESOLUTION_MS) as u8;
            row[CAL_CLOSE_BAND] = (config.close_band_ms as u32 / DEAD_BAND_RESOLUTION_MS) as u8;
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
        let layout = Layout::of(version).ok_or(ShadeRecordError::Version(version))?;

        let count = u16::from_le_bytes([bytes[OFF_COUNT], bytes[OFF_COUNT + 1]]);
        if count as usize > SHADE_TABLE_CAPACITY {
            return Err(ShadeRecordError::Count(count));
        }
        let count = count as usize;

        // A record written before the bitmap existed has to answer the question
        // anyway, and the two candidate answers are not symmetric.
        //
        // "Nothing was announced" would let a shade that *had* been announced be
        // removed on this boot and leave its retained config on the broker
        // forever — the exact failure the field was added for, reintroduced by
        // the migration.
        //
        // "Every shade in the table was announced" is wrong only for a board
        // that never reached a broker, and being wrong that way costs a
        // zero-length publish to a topic holding nothing: a no-op. So the older
        // record is read as having announced what it holds.
        let announced = match layout.announced {
            Some(offset) => Announced::from_bits(u32::from_le_bytes(word(bytes, offset))),
            None => (0..count).fold(Announced::NONE, |set, index| set.with(ShadeId(index as u8))),
        };

        // A record written before the pool existed has none. That is a real
        // loss and it is stated where it is felt: such a shade drives its
        // estimate from this controller's own transmissions only, and a wall
        // remote moving it goes unheard until somebody links the remote again.
        // Nothing here can invent an address.
        let links = match layout.links {
            None => 0,
            Some(_) => {
                let claimed =
                    u16::from_le_bytes([bytes[OFF_LINK_COUNT], bytes[OFF_LINK_COUNT + 1]]);
                // Two bounds, and both have to hold. The layout's own is what
                // the record was written against; `MAX_LINKS` is what this build
                // can hold and re-write. They differ only for a record from
                // before the pool was shortened, and a count between them is
                // refused rather than truncated — see [`MAX_LINKS`] for why a
                // silently dropped wall remote is the worse failure.
                if claimed as usize > layout.max_links || claimed as usize > MAX_LINKS {
                    return Err(ShadeRecordError::LinkCount(claimed));
                }
                claimed as usize
            }
        };

        Ok(ShadeHeader {
            seq: u32::from_le_bytes(word(bytes, OFF_SEQ)),
            count,
            announced,
            links,
            layout,
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
            let shade = decode_entry(bytes, header.layout, index)?;
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

        // The links are checked here too, before anything is visited, so that
        // the all-or-nothing rule covers the whole record rather than the
        // entries alone. A caller that placed the shades and then found a bad
        // link would have to unplace them.
        check_links(bytes, &header, &seen)?;

        // Second pass, reached only once every entry above was accepted.
        for index in 0..header.count {
            visit(index, decode_entry(bytes, header.layout, index)?);
        }
        Ok(header)
    }

    /// Hand every linked remote in `bytes` to `visit`, with the shade it
    /// belongs to.
    ///
    /// Separate from [`ShadeRecord::for_each`] rather than folded into it,
    /// because the two are used at different moments: a shade has to exist
    /// before a remote can be linked to it, so the firmware places every shade
    /// first and links afterwards. Both walks validate the whole record, so
    /// either can be called alone and neither can visit half a table.
    pub fn for_each_link(
        bytes: &[u8; SHADE_RECORD_LEN],
        mut visit: impl FnMut(LinkedRemote),
    ) -> Result<ShadeHeader, ShadeRecordError> {
        let header = ShadeRecord::header(bytes)?;
        let mut seen: [u32; SHADE_TABLE_CAPACITY] = [0; SHADE_TABLE_CAPACITY];
        for (index, address) in seen.iter_mut().enumerate().take(header.count) {
            *address = decode_entry(bytes, header.layout, index)?.config.address;
        }
        check_links(bytes, &header, &seen)?;
        for index in 0..header.links {
            visit(decode_link(bytes, index));
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
        let mut links: Vec<LinkedRemote, MAX_LINKS> = Vec::new();
        // Infallible for the same reason: `header.links` is bounded by
        // `MAX_LINKS`, which is this vector's capacity.
        ShadeRecord::for_each_link(bytes, |link| {
            let _ = links.push(link);
        })?;
        Ok(ShadeRecord {
            seq: header.seq,
            announced: header.announced,
            shades,
            links,
        })
    }
}

/// One pool word, as a link. Panic-free by construction: `index` is below a
/// `links` count [`ShadeRecord::header`] has bounded by [`MAX_LINKS`], and the
/// pool is that long (asserted at compile time above).
fn decode_link(bytes: &[u8; SHADE_RECORD_LEN], index: usize) -> LinkedRemote {
    let raw = u32::from_le_bytes(word(bytes, OFF_LINKS + index * LINK_LEN));
    LinkedRemote {
        shade: ShadeId((raw >> 24) as u8),
        address: raw & LINK_ADDRESS_MASK,
    }
}

/// Every rule a linked remote has to satisfy, checked against the shade
/// addresses `addresses[..header.count]` already decoded.
///
/// The rules are the domain's — [`Shade::link_remote`](somfy_domain::Shade::link_remote)'s — restated here for
/// the same reason every other rule in this file is: flash must not be able to
/// deliver a link the registry would then refuse, because the refusal would
/// arrive one shade at a time in a log line nobody reads.
fn check_links(
    bytes: &[u8; SHADE_RECORD_LEN],
    header: &ShadeHeader,
    addresses: &[u32; SHADE_TABLE_CAPACITY],
) -> Result<(), ShadeRecordError> {
    // One byte per row rather than a list per row: what has to be counted is
    // how many links a shade already has, and 32 bytes says that for a full
    // table.
    let mut per_shade: [u8; SHADE_TABLE_CAPACITY] = [0; SHADE_TABLE_CAPACITY];
    for index in 0..header.links {
        let link = decode_link(bytes, index);
        let row = link.shade.0 as usize;
        if row >= header.count {
            return Err(ShadeRecordError::LinkShade {
                index,
                shade: link.shade.0,
            });
        }
        let refuse = |error| Err(ShadeRecordError::Link { index, error });
        if link.address == 0 || link.address >= LINK_ADDRESS_MASK {
            return refuse(DomainError::InvalidAddress);
        }
        // A shade's own address is not a link, and the domain refuses it as a
        // duplicate. Storing it would make the same address arrive twice.
        if link.address == addresses[row] {
            return refuse(DomainError::DuplicateAddress);
        }
        // A duplicate *within* one shade. Checked against the pool rather than
        // against a per-shade list, because the pool is what is in hand.
        for earlier in 0..index {
            let other = decode_link(bytes, earlier);
            if other.shade == link.shade && other.address == link.address {
                return refuse(DomainError::DuplicateAddress);
            }
        }
        per_shade[row] += 1;
        if per_shade[row] as usize > MAX_LINKED_REMOTES {
            return refuse(DomainError::RegistryFull);
        }
    }
    Ok(())
}

/// Where a record of a given version keeps things, and which fields it has.
///
/// Two versions, two rows — small enough to be a table rather than a second
/// decoder, which is the point: the entry offsets are identical across both, so
/// everything that differs is here and nothing that differs is anywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Byte offset of the first entry.
    entries: usize,
    /// Byte offset of the announced-shade bitmap, or `None` in a version that
    /// predates it.
    announced: Option<usize>,
    /// Byte offset of the linked-remote pool, or `None` in a version that
    /// predates it.
    links: Option<usize>,
    /// Whether an entry carries its own frame width and radio protocol.
    per_shade_radio: bool,
    /// Whether an entry carries its own [`PairingState`]. `false` in a version
    /// that predates it, where the byte is zero and zero would be the wrong
    /// answer — see [`ENTRY_PAIRING`].
    per_shade_pairing: bool,
    /// Byte offset of the calibration block, or `None` in a version that
    /// predates it. See [`decode_calibration`] for what a record without one is
    /// taken to mean.
    calibration: Option<usize>,
    /// Linked remotes this version's pool can hold. Not a constant, because
    /// [`VERSION_CALIBRATION`] shortened the pool and an older record's links
    /// still run to where its own pool ended.
    max_links: usize,
}

impl Layout {
    /// The layout `version` describes, or `None` if this build has no reader
    /// for it.
    const fn of(version: u16) -> Option<Layout> {
        match version {
            VERSION_INITIAL => Some(Layout {
                entries: HEADER_LEN_V1,
                announced: None,
                links: None,
                per_shade_radio: false,
                per_shade_pairing: false,
                calibration: None,
                max_links: 0,
            }),
            VERSION_ANNOUNCED => Some(Layout {
                entries: OFF_ENTRIES,
                announced: Some(OFF_ANNOUNCED),
                links: Some(OFF_LINKS),
                per_shade_radio: true,
                per_shade_pairing: false,
                calibration: None,
                max_links: MAX_LINKS_V3,
            }),
            VERSION_PAIRING => Some(Layout {
                entries: OFF_ENTRIES,
                announced: Some(OFF_ANNOUNCED),
                links: Some(OFF_LINKS),
                per_shade_radio: true,
                per_shade_pairing: true,
                calibration: None,
                max_links: MAX_LINKS_V3,
            }),
            VERSION_CALIBRATION => Some(Layout {
                entries: OFF_ENTRIES,
                announced: Some(OFF_ANNOUNCED),
                links: Some(OFF_LINKS),
                per_shade_radio: true,
                per_shade_pairing: true,
                calibration: Some(OFF_CALIBRATION),
                max_links: MAX_LINKS,
            }),
            _ => None,
        }
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
    /// Which shades this device has published entities for — read from the
    /// record, or reconstructed for a record written before the field existed.
    /// See [`ShadeRecord::header`].
    pub announced: Announced,
    /// Linked remotes the record carries, already checked against
    /// [`MAX_LINKS`]. Zero for a record written before the pool existed.
    pub links: usize,
    /// Which version's rules the rest of the record follows.
    layout: Layout,
}

/// One entry's bytes. Panic-free by construction: `index` is always below a
/// `count` [`ShadeRecord::header`] has bounded by [`SHADE_TABLE_CAPACITY`], and
/// a full table fits under either layout (both asserted at compile time above).
fn entry_at(bytes: &[u8; SHADE_RECORD_LEN], layout: Layout, index: usize) -> &[u8] {
    let at = layout.entries + index * ENTRY_LEN;
    &bytes[at..at + ENTRY_LEN]
}

/// One entry's bytes as a shade, or which field was wrong and in which entry.
fn decode_entry(
    bytes: &[u8; SHADE_RECORD_LEN],
    layout: Layout,
    index: usize,
) -> Result<StoredShade, ShadeRecordError> {
    let entry = entry_at(bytes, layout, index);
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

    // A record from before these bytes were fields left them zero, so reading
    // them would give a zero-bit frame and refuse every shade on a board that
    // is working today. `ShadeConfig::new`'s defaults are what such a shade
    // has always been driven as — 56-bit RTS is the only thing this firmware
    // has ever transmitted — so they are what it decodes as.
    if layout.per_shade_radio {
        config.frame_width =
            FrameWidth::from_raw(entry[ENTRY_WIDTH]).ok_or(ShadeRecordError::Width {
                index,
                raw: entry[ENTRY_WIDTH],
            })?;
        config.protocol =
            RadioProtocol::from_raw(entry[ENTRY_PROTOCOL]).ok_or(ShadeRecordError::Protocol {
                index,
                raw: entry[ENTRY_PROTOCOL],
            })?;
    }

    // A record from before this byte was a field left it zero, and zero is
    // `AwaitingConfirmation` — which would un-announce every shade on a board
    // that is working today. So the *layout* decides, and a table that predates
    // the field is read as one whose shades an operator has already reported
    // working: see this module's docs for why that direction of error is the
    // survivable one.
    config.pairing_state = if layout.per_shade_pairing {
        PairingState::from_raw(entry[ENTRY_PAIRING]).ok_or(ShadeRecordError::Pairing {
            index,
            raw: entry[ENTRY_PAIRING],
        })?
    } else {
        PairingState::ConfirmedByOperator
    };

    decode_calibration(bytes, layout, index, &mut config)?;

    let initial_code = RollingCode(u16::from_le_bytes([
        entry[ENTRY_CODE],
        entry[ENTRY_CODE + 1],
    ]));
    StoredShade::new(config, initial_code).map_err(|error| ShadeRecordError::Shade { index, error })
}

/// The three [`CalibrationSource`]s packed into one byte, two bits each.
fn pack_sources(config: &ShadeConfig) -> u8 {
    let mut packed = 0u8;
    for (slot, source) in [
        config.up_time_source,
        config.down_time_source,
        config.tilt_time_source,
    ]
    .into_iter()
    .enumerate()
    {
        packed |= (source as u8) << (slot as u32 * CAL_SOURCE_BITS);
    }
    packed
}

/// Read one shade's calibration row into `config`.
///
/// # What a record from before the block existed is taken to mean
///
/// Two different answers, because the two halves are not the same kind of
/// unknown.
///
/// **The provenance is reconstructed the way it always was**: a travel time
/// equal to what a shade is created with is a value nobody chose, and anything
/// else is a value somebody typed. That is exactly the rule the API applied
/// before this block existed, so a board upgrading into version 4 reports the
/// same three sources it reported yesterday, and the field it gains is the
/// ability to say `Measured` — which nothing could produce before because
/// nothing could store it.
///
/// **The lag and the bands are read as zero**, because zero is not a guess
/// here: it is the un-compensated linear model, which is precisely what those
/// boards have been running. Substituting a plausible dead band would move every
/// estimate on the strength of a number nobody measured, which is the failure
/// this whole area exists to end.
fn decode_calibration(
    bytes: &[u8; SHADE_RECORD_LEN],
    layout: Layout,
    index: usize,
    config: &mut ShadeConfig,
) -> Result<(), ShadeRecordError> {
    let Some(offset) = layout.calibration else {
        config.up_time_source = derived_source(config.up_time_ms, FACTORY_UP_TIME_MS);
        config.down_time_source = derived_source(config.down_time_ms, FACTORY_DOWN_TIME_MS);
        config.tilt_time_source = derived_source(config.tilt_time_ms, FACTORY_TILT_TIME_MS);
        config.start_lag_ms = 0;
        config.vent_band_ms = 0;
        config.close_band_ms = 0;
        return Ok(());
    };
    // Panic-free by construction: `index` is below a `count` the header has
    // bounded by `SHADE_TABLE_CAPACITY`, and the block holds that many rows
    // (asserted at compile time above).
    let row = &bytes[offset + index * CAL_LEN..offset + (index + 1) * CAL_LEN];

    let packed = row[CAL_SOURCES];
    let mut sources = [CalibrationSource::FactoryDefault; 3];
    for (slot, source) in sources.iter_mut().enumerate() {
        let raw = (packed >> (slot as u32 * CAL_SOURCE_BITS)) & CAL_SOURCE_MASK;
        *source = CalibrationSource::from_raw(raw)
            .ok_or(ShadeRecordError::Calibration { index, raw: packed })?;
    }
    config.up_time_source = sources[0];
    config.down_time_source = sources[1];
    config.tilt_time_source = sources[2];

    config.start_lag_ms = (row[CAL_START_LAG] as u32 * START_LAG_RESOLUTION_MS) as u16;
    config.vent_band_ms = (row[CAL_VENT_BAND] as u32 * DEAD_BAND_RESOLUTION_MS) as u16;
    config.close_band_ms = (row[CAL_CLOSE_BAND] as u32 * DEAD_BAND_RESOLUTION_MS) as u16;

    // The same judgement the API and the provisioning tool make, restated here
    // for the same reason every other rule in this file is restated: flash must
    // not be able to deliver a shade whose lag and band leave no travel behind
    // them, because the estimator answers that by reporting every move as
    // instantly arrived.
    config
        .checked_bands()
        .map_err(|error| ShadeRecordError::Shade {
            index,
            error: ShadeError::Domain(error),
        })
}

/// The pre-version-4 rule: a travel time equal to the factory default is one
/// nobody chose.
fn derived_source(value_ms: u32, factory_ms: u32) -> CalibrationSource {
    if value_ms == factory_ms {
        CalibrationSource::FactoryDefault
    } else {
        CalibrationSource::OperatorSupplied
    }
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
        let mut links = Vec::new();
        links
            .push(LinkedRemote {
                shade: ShadeId(1),
                address: 0x00_2002,
            })
            .expect("fits");
        ShadeRecord {
            seq: 3,
            announced: Announced::NONE.with(ShadeId(0)),
            shades,
            links,
        }
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

    /// The unused tail of an entry's name field is checksummed, so a later
    /// format cannot put a field there and have this version accept the record
    /// anyway.
    ///
    /// It used to be byte 23 that was tested here, which is now the pairing
    /// state — version 3 spent the entry's last spare byte. What is left over is
    /// the part of the 32-byte name field past `name_len`, and it is zero-filled
    /// for the same reason: equal records must encode identically.
    #[test]
    fn the_unused_tail_of_a_name_is_covered_by_the_checksum() {
        let mut bytes = record().encode();
        // "Kitchen" is seven bytes, so byte 8 of the field is spare.
        bytes[field(0, ENTRY_NAME) + 8] = 0x01;
        assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
    }

    /// A pairing byte outside the two the domain models is refused rather than
    /// resolved. Both possible defaults are wrong here — one hides a working
    /// shade, the other announces a dead one — so the record is the thing to
    /// fix.
    #[test]
    fn an_unmodelled_pairing_state_is_rejected() {
        let bytes = tampered(|bytes| bytes[field(1, ENTRY_PAIRING)] = 0x07);
        assert_eq!(
            ShadeRecord::decode(&bytes),
            Err(ShadeRecordError::Pairing {
                index: 1,
                raw: 0x07
            }),
        );
    }

    /// The state survives the round trip per shade, rather than per table.
    #[test]
    fn each_shades_pairing_state_round_trips_on_its_own() {
        let mut shades = Vec::new();
        let mut awaiting = shade("Kitchen", 0x00_1001, 7);
        awaiting.config.pairing_state = PairingState::AwaitingConfirmation;
        let mut confirmed = shade("Salon", 0x00_1002, 9);
        confirmed.config.pairing_state = PairingState::ConfirmedByOperator;
        shades.push(awaiting).expect("fits");
        shades.push(confirmed).expect("fits");
        let written = ShadeRecord {
            seq: 1,
            announced: Announced::NONE,
            shades,
            links: Vec::new(),
        };

        let read = ShadeRecord::decode(&written.encode()).expect("its own output is readable");
        assert_eq!(
            read.shades[0].config.pairing_state,
            PairingState::AwaitingConfirmation,
        );
        assert_eq!(
            read.shades[1].config.pairing_state,
            PairingState::ConfirmedByOperator,
        );
    }

    /// The unused tail of the linked-remote pool is checksummed too, so a later
    /// format cannot put a field in it and have this version accept the record.
    #[test]
    fn the_unused_pool_is_covered_by_the_checksum() {
        let mut bytes = record().encode();
        bytes[OFF_LINKS + (MAX_LINKS - 1) * LINK_LEN] = 0x01;
        assert_eq!(ShadeRecord::decode(&bytes), Err(ShadeRecordError::Checksum));
    }

    /// The numbers the docs quote, pinned. That a full table *fits* is asserted
    /// at compile time above; what this adds is that the figures the module and
    /// the partition table are documented with are the figures in force — and
    /// that the record is full to the byte, which is what makes [`MAX_LINKS`]
    /// "whatever is left" rather than a number somebody chose.
    #[test]
    fn a_full_table_is_the_size_the_docs_claim() {
        assert_eq!(ENTRY_LEN, 56);
        assert_eq!(SHADE_TABLE_CAPACITY, 32);
        assert_eq!(HEADER_LEN, 20);
        assert_eq!(CAL_LEN, 4);
        assert_eq!(MAX_LINKS, 26);
        // What version 3 had, and what a record written by that build is still
        // read against. The gap between the two is the calibration block.
        assert_eq!(MAX_LINKS_V3, 58);
        assert_eq!(
            (MAX_LINKS_V3 - MAX_LINKS) * LINK_LEN,
            SHADE_TABLE_CAPACITY * CAL_LEN,
        );
        assert_eq!(
            HEADER_LEN
                + SHADE_TABLE_CAPACITY * ENTRY_LEN
                + MAX_LINKS * LINK_LEN
                + SHADE_TABLE_CAPACITY * CAL_LEN
                + CRC_LEN,
            SHADE_RECORD_LEN,
        );
    }
}
