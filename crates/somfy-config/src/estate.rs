//! The bytes one estate slot holds: the rooms a shade lives in and the groups
//! it answers to.
//!
//! Same shape as [`crate::ShadeRecord`] and [`crate::ConfigRecord`] — fixed
//! length, magic, version, CRC-32 over the whole thing — because it lives on
//! the same kind of region and fails in the same ways. What differs is the
//! consequence: losing this record costs the *arrangement* of an installation,
//! not the installation. Every shade still exists, is still commandable and
//! still keeps its rolling code; what is gone is which room it is in and which
//! group it moves with.
//!
//! ## Why it is a third region and not more of the second
//!
//! Because the shade record has no room left, to the byte. Its own docs work
//! the arithmetic: a slot is 2048, a full table of 32 entries plus the header,
//! the calibration block and the checksum leave exactly [`crate::MAX_LINKS`]
//! four-byte words, and a compile-time assertion states that the pool "must
//! fill the record exactly". There is nowhere in it to put a room.
//!
//! The three alternatives were the same three that constant already weighs. A
//! bigger shade slot is priced against the boot stack and refused there. A
//! smaller shade table would break the property that a registry which fits the
//! controller fits the record. A third region costs one partition-table entry
//! in space the table already holds in reserve — 0x208000 to 0x210000 is
//! unallocated and `crates/firmware/partitions.csv` names Plan 6's
//! configuration as its likely claimant — and it costs nothing that already
//! exists: no offset moves, so a provisioned board reflashed with the new table
//! keeps its rolling codes, its credentials and its shades exactly as it did.
//!
//! ## What a row's position means
//!
//! The same thing it means in the shade record: **the row is the id.** The
//! firmware fills an empty registry in record order and
//! `somfy_domain::Registry::add_room` / `add_group` assign the lowest free
//! slot, so the first room is `RoomId(0)` and the first group `GroupId(0)`.
//!
//! And it means that here in a second way, because this record *refers* to the
//! shade record by row: [`StoredGroup::members`] is a bitmap over shade rows
//! and [`EstateRecord::room_of`] is indexed by one. So the two records are read
//! together or not at all — **reordering the shade table silently repoints
//! every group and every room assignment**, which is one more reason the shade
//! record's own docs say a reorder is unsafe.
//!
//! The two are written together for that reason: `provision_shades` emits both
//! files from one import, and neither is meaningful beside the other's
//! predecessor.
//!
//! ## What the device does with it, and what it does not
//!
//! The firmware **reads** this region and never writes it, exactly as it did
//! with the shade region before there was an edit path. Rooms and groups have
//! no runtime edit vocabulary yet — `crates/firmware/src/edits.rs` carries
//! shades only — so the host-side provisioning tool is the one writer, and
//! saying that plainly is better than a write path with no producer.
//!
//! A group's [`address`](StoredGroup::address) and
//! [`next_code`](StoredGroup::next_code) are **stored and not yet read.** A
//! group on the controller this replaces is a virtual remote in its own right,
//! with an address and a rolling code of its own, and v1.0 executes a group
//! command by fanning it out to each member shade rather than transmitting a
//! group frame, so nothing here needs the identity today. It is carried because
//! it cannot be recovered later: the controller being replaced is the only
//! thing that knows it, and it knows it once, at the moment of the export. See
//! [`StoredGroup::code_recovered`] for the half of it that is a warning rather
//! than a value.

use heapless::{String, Vec};
use somfy_domain::{GroupId, RoomId, ShadeId, MAX_GROUPS, MAX_ROOMS, MAX_SHADES};
use somfy_rts::RollingCode;

/// Rooms one record carries — the registry's own capacity, so an estate that
/// fits the controller fits the record.
pub const ESTATE_ROOM_CAPACITY: usize = MAX_ROOMS;

/// Groups one record carries — likewise.
pub const ESTATE_GROUP_CAPACITY: usize = MAX_GROUPS;

/// Shade rows a room assignment or a group membership may refer to. The same
/// bound the shade record's table has, because it is that table being indexed.
pub const ESTATE_SHADE_CAPACITY: usize = MAX_SHADES;

/// Bytes a stored name may occupy — `somfy_domain::Registry`'s own capacity for
/// a room or group name, which is also a shade's.
const MAX_NAME_LEN: usize = 32;

/// Bytes in one estate record, and therefore in one slot of the estate ring.
///
/// 2048 for the two relationships the ring needs and for no other reason: it is
/// a whole number of 4-byte flash words, and it divides a 4 KB erase sector
/// exactly, so two records tile a sector and four tile the two-sector region.
/// 1024 would do neither — see the assertion below, which is what says so: the
/// fields come to 1,328 bytes and would not fit.
///
/// **It is not full, and that is stated rather than dressed up.** 716 bytes sit
/// between the last group and the checksum. The shade record next door is full
/// to the byte and makes a virtue of it; this one is sized by the sector rather
/// than by its contents, so the spare is real. What could grow into it, in
/// rough order of likelihood: a group's frame width, radio protocol and repeat
/// count, which a group-transmit path would need; a sort order per room and per
/// group, which a migrated installation carries and this does not; and a
/// per-group pairing
/// state, if a group is ever paired the way a shade is.
pub const ESTATE_RECORD_LEN: usize = 2048;

/// Marks a slot as this format's. Spells `RTSE` in a hex dump — RTS Estate —
/// and is deliberately distinct from `RTSC` (rolling codes), `RTSW` (device
/// config) and `RTSS` (shades), so a region mounted at the wrong offset is
/// reported rather than half-read.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSE");

/// The version this build writes.
///
/// Bumped when the layout below changes. A record carrying a version this build
/// has no reader for is reported as such rather than as damage, so a later
/// implementation can migrate instead of erasing an estate it does not
/// recognise. There is only one so far, and unlike the shade record there is no
/// board in the field carrying an older one — this region has never been
/// written before.
const VERSION: u16 = 1;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// Header offsets. Spelled out rather than computed so the layout can be read
// off the file and compared against a hex dump.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_ROOM_COUNT: usize = 6;
const OFF_GROUP_COUNT: usize = 7;
const OFF_SEQ: usize = 8;
// 12..16 is padding, written zero, so the first room starts on a 16-byte
// boundary and the blocks below line up in a dump.
const HEADER_LEN: usize = 16;

/// One room: a length byte, three bytes of padding so the name starts on a word
/// boundary, and the name.
const ROOM_LEN: usize = 36;
const ROOM_NAME_LEN: usize = 0;
const ROOM_NAME: usize = 4;

const OFF_ROOMS: usize = HEADER_LEN;

/// Which room each shade row belongs to, one byte per row.
///
/// A **per-shade array rather than a member list per room**, and the difference
/// is a rule made unrepresentable: a shade lives in at most one room —
/// `somfy_domain::Registry::room_assign` enforces it by removing the shade from
/// every room before adding it to the target — and an array of one byte per
/// shade cannot say otherwise. Member lists could, and the format would then
/// need a check for something the domain has already decided.
const OFF_ROOM_OF: usize = OFF_ROOMS + ESTATE_ROOM_CAPACITY * ROOM_LEN;

/// What a [`OFF_ROOM_OF`] byte holds for a shade in no room. `0xFF` rather than
/// zero because zero is `RoomId(0)`, a perfectly ordinary room.
const ROOM_NONE: u8 = 0xFF;

/// One group: its virtual-remote identity, its membership bitmap, and its name.
const GROUP_LEN: usize = 44;
const GROUP_ADDRESS: usize = 0;
const GROUP_CODE: usize = 4;
const GROUP_NAME_LEN: usize = 6;
const GROUP_FLAGS: usize = 7;
const GROUP_MEMBERS: usize = 8;
const GROUP_NAME: usize = 12;

/// Bit 0 of [`GROUP_FLAGS`]: the rolling code in this row came off the file
/// rather than being invented. See [`StoredGroup::code_recovered`].
const GROUP_FLAG_CODE_RECOVERED: u8 = 1 << 0;

/// Every flag bit this version defines. A record with any other bit set is a
/// record this build does not understand well enough to re-write.
const GROUP_FLAGS_KNOWN: u8 = GROUP_FLAG_CODE_RECOVERED;

const OFF_GROUPS: usize = OFF_ROOM_OF + ESTATE_SHADE_CAPACITY;

const CRC_LEN: usize = 4;
const OFF_CRC: usize = ESTATE_RECORD_LEN - CRC_LEN;

/// The sentinel addresses a remote may not have, matching
/// `somfy_domain::ShadeConfig::new`: `0` is "unset" and `0xFF_FFFF` is the
/// broadcast-ish top of the 24-bit space.
const ADDRESS_SENTINEL: u32 = 0xFF_FFFF;

// Everything the record claims to hold has to fit between the header and the
// checksum. Compile-time rather than tests, because it is arithmetic over
// constants and a test would only assert what the compiler already knows.
const _: () = assert!(
    ROOM_NAME + MAX_NAME_LEN <= ROOM_LEN,
    "a room's name must fit inside one room row"
);
const _: () = assert!(
    GROUP_NAME + MAX_NAME_LEN <= GROUP_LEN,
    "a group's fields must fit inside one group row"
);
const _: () = assert!(
    OFF_GROUPS + ESTATE_GROUP_CAPACITY * GROUP_LEN <= OFF_CRC,
    "a full estate must fit inside one record"
);
const _: () = assert!(
    OFF_CRC + CRC_LEN == ESTATE_RECORD_LEN,
    "the checksum must occupy the last four bytes of the record"
);
// A membership bitmap is one bit per shade row in a `u32`, so a shade table
// wider than 32 rows would silently stop recording the last shades' membership.
const _: () = assert!(
    ESTATE_SHADE_CAPACITY <= u32::BITS as usize,
    "a group's membership bitmap has one bit per shade row and no more"
);
// A room index and a shade row are each stored in one byte, and `ROOM_NONE`
// must not be a room a record could legitimately name.
const _: () = assert!(
    ESTATE_ROOM_CAPACITY < ROOM_NONE as usize,
    "the unassigned sentinel must not collide with a real room index"
);

/// One room: everything the registry needs to create it.
///
/// A name and nothing else, because that is all
/// `somfy_domain::Registry::add_room` takes. Where its shades are is
/// [`EstateRecord::room_of`], on the other side of the relationship, for the
/// reason the room-assignment array gives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoredRoom {
    /// What it is called. May be empty: `Registry::add_room` accepts an empty
    /// name and a room is not a Home Assistant entity, so refusing one here
    /// would be a rule this crate invented rather than one it enforces.
    pub name: String<MAX_NAME_LEN>,
}

/// One group: what it is called, which shades move with it, and the virtual
/// remote it *would* transmit as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGroup {
    /// What it is called.
    pub name: String<MAX_NAME_LEN>,
    /// The group's own 24-bit radio address.
    ///
    /// A group in the controller being replaced is a remote in its own right,
    /// with an address and a rolling code, and it was allocated out of the same
    /// space the shades were. Nothing transmits it here — v1.0 fans a group
    /// command out to its members — but it is the half of the identity that
    /// cannot be recreated, so it is carried.
    pub address: u32,
    /// **Next-to-send** rolling code for [`address`](StoredGroup::address),
    /// under the same convention as a shade's: the file stores the last code
    /// sent and the import adds one.
    ///
    /// Read [`code_recovered`](StoredGroup::code_recovered) before believing
    /// it.
    pub next_code: RollingCode,
    /// Whether [`next_code`](StoredGroup::next_code) came off the backup rather
    /// than being invented.
    ///
    /// # Why a record carries a warning
    ///
    /// Because the value it warns about outlives the warning. A backup at
    /// format version 19 to 22 does **not** contain a group's rolling code —
    /// the controller being replaced keeps it outside the file it exports — so
    /// `somfy_migrate::parse_group_record` fabricates
    /// `RollingCode(1)`, and nothing about the resulting number says it was
    /// fabricated: `1` is a value a real group could legitimately be at.
    ///
    /// A motor rejects any code at or below the last it accepted, so a
    /// fabricated code is a group that will not actuate until somebody re-pairs
    /// it or enters the real number. `provision_shades` says so at import, to a
    /// person, once. This bit is what is left when that terminal has been
    /// closed — and **the first thing a group-transmit path must read**, since
    /// it is the only thing standing between a stored `1` and a burst nothing
    /// obeys.
    pub code_recovered: bool,
    /// Which shade rows move with this group, as rows of the shade record —
    /// which are also their [`ShadeId`]s.
    pub members: Members,
}

/// The shade rows one group holds, as a bitmap.
///
/// # Why a bitmap and not a list
///
/// Because membership is a *set of rows* and the question asked of it is "is
/// this row in it?". A list would be 32 bytes per group where this is four, and
/// it could hold the same row twice — which `Registry::group_add_shade` treats
/// as a no-op, so the format would be able to say something the domain cannot
/// hear.
///
/// It is deliberately **not** [`crate::Announced`], which is the same shape one
/// module over: that one is a set of shades that have been announced, a fact
/// about the device, and this is a set of shades in a group, a fact about the
/// estate. Sharing the type would couple a change in one to the other. The
/// forty lines are cheap; the coupling is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Members(u32);

impl Members {
    /// The empty set. A group with no members is a group somebody made and has
    /// not filled in, which is a thing an operator can mean.
    pub const NONE: Members = Members(0);

    /// Reconstruct from the stored word.
    ///
    /// Bits above the shade capacity are **dropped**, not rejected: they name
    /// rows this build has no shade for and nothing to command, so carrying
    /// them would only let a later [`Members::bits`] write back a claim this
    /// build cannot act on.
    pub const fn from_bits(bits: u32) -> Members {
        // `ESTATE_SHADE_CAPACITY <= 32` is asserted above, and the shift is
        // written so that a capacity of exactly 32 does not overflow.
        Members(bits & (u32::MAX >> (u32::BITS as usize - ESTATE_SHADE_CAPACITY)))
    }

    /// The word as stored.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether `id` moves with this group.
    ///
    /// A row past the shade capacity is always `false`: it names no slot, so
    /// nothing could have joined it.
    pub const fn contains(self, id: ShadeId) -> bool {
        match Members::mask(id) {
            Some(mask) => self.0 & mask != 0,
            None => false,
        }
    }

    /// The same set with `id` in it. An out-of-range row is ignored rather than
    /// shifted past the end of the word.
    pub const fn with(self, id: ShadeId) -> Members {
        match Members::mask(id) {
            Some(mask) => Members(self.0 | mask),
            None => self,
        }
    }

    /// Every row in the set, ascending.
    pub fn ids(self) -> impl Iterator<Item = ShadeId> {
        (0..ESTATE_SHADE_CAPACITY as u8)
            .map(ShadeId)
            .filter(move |id| self.contains(*id))
    }

    /// How many rows are in the set.
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The bit `id` occupies, or `None` if it names no shade row.
    const fn mask(id: ShadeId) -> Option<u32> {
        if (id.0 as usize) < ESTATE_SHADE_CAPACITY {
            Some(1u32 << id.0)
        } else {
            None
        }
    }
}

/// Why a slot's bytes are not an estate record.
///
/// [`Blank`](EstateRecordError::Blank) is its own variant for the same reason
/// it is in the other three record formats: an erased slot is the ordinary
/// state of every slot the ring has not reached, and a reader that cannot tell
/// "never written" from "damaged" cannot tell a first boot from data loss. Here
/// it is the *expected* state — no board has ever had this region written — so
/// it must not read as an estate that was lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstateRecordError {
    /// Every byte is erased. The slot has never been written.
    Blank,
    /// Not this format's magic. Foreign data, or a write torn before the header
    /// landed.
    Magic,
    /// The checksum does not match the bytes — a torn write, or bit rot.
    Checksum,
    /// A record of some other version of this format.
    Version(u16),
    /// The header claims more rooms than a record can hold.
    RoomCount(u8),
    /// The header claims more groups than a record can hold.
    GroupCount(u8),
    /// A stored name length does not fit the field it describes. These lengths
    /// come off a device, so they are checked rather than trusted.
    NameLength {
        /// Which row, and of which kind.
        at: Row,
        /// The length the record claimed.
        len: usize,
    },
    /// A name's bytes are not UTF-8, so they are not a name anything downstream
    /// could show.
    NotUtf8 {
        /// Which row, and of which kind.
        at: Row,
    },
    /// A shade row is assigned to a room the record does not have.
    ///
    /// Reported rather than dropped: the shade belongs to *some* room, and
    /// silently leaving it in none is a rearrangement of an installation
    /// nothing announced.
    RoomIndex {
        /// The shade row carrying the assignment.
        shade: ShadeId,
        /// The room index it named.
        room: u8,
    },
    /// A group's address is one no remote can have — `0` or `0xFF_FFFF`, the
    /// two sentinels `somfy_domain::ShadeConfig::new` refuses. A group is a
    /// virtual remote, so it is held to a remote's rule.
    GroupAddress {
        /// Which group row.
        group: GroupId,
        /// The address the record carried.
        address: u32,
    },
    /// Two groups at one radio address, which means the record does not say
    /// whose rolling code is whose.
    DuplicateAddress {
        /// The later of the two rows.
        group: GroupId,
        /// The address they share.
        address: u32,
    },
    /// A group row has a flag bit this version does not define.
    GroupFlags {
        /// Which group row.
        group: GroupId,
        /// The byte the record carried.
        raw: u8,
    },
}

/// Which row an [`EstateRecordError`] is about, so a message names something a
/// person can find.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// A room row, which is also its [`RoomId`].
    Room(usize),
    /// A group row, which is also its [`GroupId`].
    Group(usize),
}

impl core::fmt::Display for Row {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Row::Room(index) => write!(f, "room {index}"),
            Row::Group(index) => write!(f, "group {index}"),
        }
    }
}

impl core::fmt::Display for EstateRecordError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EstateRecordError::Blank => write!(f, "the region has never been written"),
            EstateRecordError::Magic => write!(f, "these bytes are not an estate record"),
            EstateRecordError::Checksum => {
                write!(f, "the checksum does not match the bytes: a torn write")
            }
            EstateRecordError::Version(version) => {
                write!(
                    f,
                    "the record is format version {version}, which this build cannot read"
                )
            }
            EstateRecordError::RoomCount(count) => write!(
                f,
                "the record claims {count} rooms and holds at most {ESTATE_ROOM_CAPACITY}"
            ),
            EstateRecordError::GroupCount(count) => write!(
                f,
                "the record claims {count} groups and holds at most {ESTATE_GROUP_CAPACITY}"
            ),
            EstateRecordError::NameLength { at, len } => write!(
                f,
                "{at} claims a {len}-byte name and the field holds {MAX_NAME_LEN}"
            ),
            EstateRecordError::NotUtf8 { at } => write!(f, "{at}'s name is not UTF-8"),
            EstateRecordError::RoomIndex { shade, room } => write!(
                f,
                "shade row {} is assigned to room {room}, which the record does not have",
                shade.0
            ),
            EstateRecordError::GroupAddress { group, address } => write!(
                f,
                "group {} is at address {address} ({address:#08X}), which is not one a remote \
                 can have: 0 and 0xFFFFFF are reserved",
                group.0
            ),
            EstateRecordError::DuplicateAddress { group, address } => write!(
                f,
                "group {} is at address {address} ({address:#08X}), which an earlier group \
                 already holds",
                group.0
            ),
            EstateRecordError::GroupFlags { group, raw } => write!(
                f,
                "group {}'s flags byte is {raw:#04X}, which sets a bit this format does not \
                 define",
                group.0
            ),
        }
    }
}

impl core::error::Error for EstateRecordError {}

/// One slot's worth of bytes: a sequence number and the estate it stamps.
///
/// The sequence number orders records around the ring — the same role, and the
/// same wrapping comparison, as in the other three stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstateRecord {
    /// Monotonic write counter, wrapping at [`u32::MAX`].
    pub seq: u32,
    /// Every room, in the order their ids will follow.
    pub rooms: Vec<StoredRoom, ESTATE_ROOM_CAPACITY>,
    /// Which room each shade row is in, indexed by row. `None` is a shade in no
    /// room, which is what a backup holds for an unassigned shade and what a
    /// freshly provisioned table holds throughout.
    ///
    /// Always [`ESTATE_SHADE_CAPACITY`] long, so it needs no agreement with the
    /// shade record about how many shades there are. A row past the end of the
    /// shade table is simply never consulted, which is also what makes a stale
    /// entry for a removed shade harmless rather than an error.
    pub room_of: [Option<RoomId>; ESTATE_SHADE_CAPACITY],
    /// Every group, in the order their ids will follow.
    pub groups: Vec<StoredGroup, ESTATE_GROUP_CAPACITY>,
}

impl Default for EstateRecord {
    fn default() -> EstateRecord {
        EstateRecord::empty(0)
    }
}

impl EstateRecord {
    /// An estate with no rooms and no groups, which is what a board that was
    /// never imported into has and is a value an operator can mean.
    pub fn empty(seq: u32) -> EstateRecord {
        EstateRecord {
            seq,
            rooms: Vec::new(),
            room_of: [None; ESTATE_SHADE_CAPACITY],
            groups: Vec::new(),
        }
    }

    /// Whether this estate says nothing at all.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty() && self.groups.is_empty()
    }

    /// Serialise into the exact bytes a slot holds.
    ///
    /// Everything unused is zero-filled, so equal records produce identical
    /// bytes — which is what lets a writer prove a write landed by reading it
    /// back and comparing — and so a hex dump of flash is readable. The one
    /// exception is [`EstateRecord::room_of`], whose unused entries are
    /// the unassigned sentinel rather than zero, because zero is a room.
    pub fn encode(&self) -> [u8; ESTATE_RECORD_LEN] {
        let mut bytes = [0u8; ESTATE_RECORD_LEN];
        bytes[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());
        // Both bounded by their vectors' own capacities, which are the two
        // capacity constants.
        bytes[OFF_ROOM_COUNT] = self.rooms.len() as u8;
        bytes[OFF_GROUP_COUNT] = self.groups.len() as u8;
        bytes[OFF_SEQ..OFF_SEQ + 4].copy_from_slice(&self.seq.to_le_bytes());

        for (index, room) in self.rooms.iter().enumerate() {
            let at = OFF_ROOMS + index * ROOM_LEN;
            let row = &mut bytes[at..at + ROOM_LEN];
            // Bounded by `StoredRoom::name`'s own capacity, which is the field
            // width here.
            let name = room.name.as_bytes();
            row[ROOM_NAME_LEN] = name.len() as u8;
            row[ROOM_NAME..ROOM_NAME + name.len()].copy_from_slice(name);
        }

        for (row, assignment) in self.room_of.iter().enumerate() {
            bytes[OFF_ROOM_OF + row] = match assignment {
                Some(room) => room.0,
                None => ROOM_NONE,
            };
        }

        for (index, group) in self.groups.iter().enumerate() {
            let at = OFF_GROUPS + index * GROUP_LEN;
            let row = &mut bytes[at..at + GROUP_LEN];
            row[GROUP_ADDRESS..GROUP_ADDRESS + 4].copy_from_slice(&group.address.to_le_bytes());
            row[GROUP_CODE..GROUP_CODE + 2].copy_from_slice(&group.next_code.0.to_le_bytes());
            row[GROUP_FLAGS] = if group.code_recovered {
                GROUP_FLAG_CODE_RECOVERED
            } else {
                0
            };
            row[GROUP_MEMBERS..GROUP_MEMBERS + 4]
                .copy_from_slice(&group.members.bits().to_le_bytes());
            let name = group.name.as_bytes();
            row[GROUP_NAME_LEN] = name.len() as u8;
            row[GROUP_NAME..GROUP_NAME + name.len()].copy_from_slice(name);
        }

        let checksum = CRC.checksum(&bytes[..OFF_CRC]);
        bytes[OFF_CRC..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Read the header, and nothing else.
    ///
    /// The checksum is verified here, so a header this returns describes bytes
    /// that are whole. What it does **not** do is decode a single room, which
    /// is the point: a scan of the ring needs each slot's sequence number to
    /// find the newest record, and decoding four estates to compare four `u32`s
    /// costs stack on a device that has very little.
    pub fn header(bytes: &[u8; ESTATE_RECORD_LEN]) -> Result<EstateHeader, EstateRecordError> {
        if bytes.iter().all(|byte| *byte == 0xFF) {
            return Err(EstateRecordError::Blank);
        }
        if u32::from_le_bytes(word(bytes, OFF_MAGIC)) != MAGIC {
            return Err(EstateRecordError::Magic);
        }
        if u32::from_le_bytes(word(bytes, OFF_CRC)) != CRC.checksum(&bytes[..OFF_CRC]) {
            return Err(EstateRecordError::Checksum);
        }

        let version = u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]);
        if version != VERSION {
            return Err(EstateRecordError::Version(version));
        }

        let rooms = bytes[OFF_ROOM_COUNT];
        if rooms as usize > ESTATE_ROOM_CAPACITY {
            return Err(EstateRecordError::RoomCount(rooms));
        }
        let groups = bytes[OFF_GROUP_COUNT];
        if groups as usize > ESTATE_GROUP_CAPACITY {
            return Err(EstateRecordError::GroupCount(groups));
        }

        Ok(EstateHeader {
            seq: u32::from_le_bytes(word(bytes, OFF_SEQ)),
            rooms: rooms as usize,
            groups: groups as usize,
        })
    }

    /// Hand every room in `bytes` to `visit`, one at a time.
    ///
    /// **All or nothing: if any row anywhere in the record is refused, nothing
    /// is visited at all**, and the error names the row. That is the same rule
    /// [`crate::ShadeRecord::for_each`] enforces and it is here for a related
    /// reason: rooms and groups take their ids from position too, so loading
    /// the survivors of a bad record would renumber the rest and move every
    /// membership off the group it belonged to.
    ///
    /// `visit` is given the row index, which is the id
    /// `somfy_domain::Registry::add_room` will assign, so a caller can check
    /// the two agree rather than assume it.
    ///
    /// # Why three walks and not one visitor with three closures
    ///
    /// Because the caller that matters holds one `&mut Registry` and would need
    /// to lend it to all three at once, which does not borrow. Splitting them
    /// is what `ShadeRecord` already does for its shades and its links, for the
    /// same reason and with the same consequence: each walk validates the whole
    /// record, so any of them can be called alone and none can visit half an
    /// estate.
    ///
    /// The order the registry needs is rooms, then assignments, then groups: a
    /// room must exist before a shade can be assigned to it, and a group before
    /// a shade can join it.
    pub fn for_each_room(
        bytes: &[u8; ESTATE_RECORD_LEN],
        mut visit: impl FnMut(RoomId, StoredRoom),
    ) -> Result<EstateHeader, EstateRecordError> {
        let header = validate(bytes)?;
        for index in 0..header.rooms {
            visit(RoomId(index as u8), decode_room(bytes, index)?);
        }
        Ok(header)
    }

    /// Hand every shade row that is in a room to `visit`, with the room.
    ///
    /// Rows with no room are skipped rather than visited with a `None`: the
    /// caller is placing assignments, and "this shade is in no room" is what
    /// not calling it already means. See [`EstateRecord::for_each_room`] for
    /// the all-or-nothing rule and why the walks are separate.
    pub fn for_each_assignment(
        bytes: &[u8; ESTATE_RECORD_LEN],
        mut visit: impl FnMut(ShadeId, RoomId),
    ) -> Result<EstateHeader, EstateRecordError> {
        let header = validate(bytes)?;
        for shade in 0..ESTATE_SHADE_CAPACITY {
            if let Some(room) = decode_assignment(bytes, shade) {
                visit(ShadeId(shade as u8), room);
            }
        }
        Ok(header)
    }

    /// Hand every group in `bytes` to `visit`, one at a time. See
    /// [`EstateRecord::for_each_room`] for the all-or-nothing rule and why the
    /// walks are separate.
    pub fn for_each_group(
        bytes: &[u8; ESTATE_RECORD_LEN],
        mut visit: impl FnMut(GroupId, StoredGroup),
    ) -> Result<EstateHeader, EstateRecordError> {
        let header = validate(bytes)?;
        for index in 0..header.groups {
            visit(GroupId(index as u8), decode_group(bytes, index)?);
        }
        Ok(header)
    }

    /// Read a slot's bytes back as a whole estate, or say precisely why they
    /// are not one.
    ///
    /// The checksum is verified **before** any field is interpreted, so a torn
    /// write is reported as [`EstateRecordError::Checksum`] rather than as
    /// whatever its half-written header happens to spell.
    pub fn decode(bytes: &[u8; ESTATE_RECORD_LEN]) -> Result<EstateRecord, EstateRecordError> {
        let mut rooms: Vec<StoredRoom, ESTATE_ROOM_CAPACITY> = Vec::new();
        let mut room_of = [None; ESTATE_SHADE_CAPACITY];
        let mut groups: Vec<StoredGroup, ESTATE_GROUP_CAPACITY> = Vec::new();
        // Every push below is infallible: each walk yields at most the count
        // the header carries, and both are bounded by the capacity.
        let header = EstateRecord::for_each_room(bytes, |_, room| {
            let _ = rooms.push(room);
        })?;
        EstateRecord::for_each_assignment(bytes, |shade, room| {
            room_of[shade.0 as usize] = Some(room)
        })?;
        EstateRecord::for_each_group(bytes, |_, group| {
            let _ = groups.push(group);
        })?;
        Ok(EstateRecord {
            seq: header.seq,
            rooms,
            room_of,
            groups,
        })
    }
}

/// Read the header and check every row and every reference, without decoding
/// anything into the caller's hands.
///
/// This is the all-or-nothing gate the three walks share: each of them runs it
/// first, so a record with one bad group places no rooms either.
fn validate(bytes: &[u8; ESTATE_RECORD_LEN]) -> Result<EstateHeader, EstateRecordError> {
    let header = EstateRecord::header(bytes)?;
    for index in 0..header.rooms {
        decode_room(bytes, index)?;
    }
    check_assignments(bytes, &header)?;
    let mut seen: [u32; ESTATE_GROUP_CAPACITY] = [0; ESTATE_GROUP_CAPACITY];
    for index in 0..header.groups {
        let decoded = decode_group(bytes, index)?;
        if seen[..index].contains(&decoded.address) {
            return Err(EstateRecordError::DuplicateAddress {
                group: GroupId(index as u8),
                address: decoded.address,
            });
        }
        seen[index] = decoded.address;
    }
    Ok(header)
}

/// What a record says about itself before any room is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstateHeader {
    /// Monotonic write counter, wrapping at [`u32::MAX`]. What orders records
    /// around the ring.
    pub seq: u32,
    /// Rooms the record carries, already checked against the capacity.
    pub rooms: usize,
    /// Groups the record carries, already checked against the capacity.
    pub groups: usize,
}

/// One room row. Panic-free by construction: `index` is below a `rooms` count
/// [`EstateRecord::header`] has bounded by [`ESTATE_ROOM_CAPACITY`], and a full
/// block fits (asserted at compile time above).
fn decode_room(
    bytes: &[u8; ESTATE_RECORD_LEN],
    index: usize,
) -> Result<StoredRoom, EstateRecordError> {
    let at = OFF_ROOMS + index * ROOM_LEN;
    let row = &bytes[at..at + ROOM_LEN];
    let name = read_name(row, ROOM_NAME_LEN, ROOM_NAME, Row::Room(index))?;
    Ok(StoredRoom { name })
}

/// One group row. Panic-free for the same reason [`decode_room`] is.
fn decode_group(
    bytes: &[u8; ESTATE_RECORD_LEN],
    index: usize,
) -> Result<StoredGroup, EstateRecordError> {
    let at = OFF_GROUPS + index * GROUP_LEN;
    let row = &bytes[at..at + GROUP_LEN];
    let group = GroupId(index as u8);

    let address = u32::from_le_bytes([
        row[GROUP_ADDRESS],
        row[GROUP_ADDRESS + 1],
        row[GROUP_ADDRESS + 2],
        row[GROUP_ADDRESS + 3],
    ]);
    // The rule a shade's address is held to, applied to a group because a group
    // *is* a virtual remote — `somfy_domain::ShadeConfig::new` refuses both
    // sentinels and the 24-bit ceiling, and a stored address the domain would
    // refuse is the class of disagreement this whole file exists to prevent.
    if address == 0 || address >= ADDRESS_SENTINEL {
        return Err(EstateRecordError::GroupAddress { group, address });
    }

    let flags = row[GROUP_FLAGS];
    if flags & !GROUP_FLAGS_KNOWN != 0 {
        return Err(EstateRecordError::GroupFlags { group, raw: flags });
    }

    Ok(StoredGroup {
        name: read_name(row, GROUP_NAME_LEN, GROUP_NAME, Row::Group(index))?,
        address,
        next_code: RollingCode(u16::from_le_bytes([row[GROUP_CODE], row[GROUP_CODE + 1]])),
        code_recovered: flags & GROUP_FLAG_CODE_RECOVERED != 0,
        members: Members::from_bits(u32::from_le_bytes([
            row[GROUP_MEMBERS],
            row[GROUP_MEMBERS + 1],
            row[GROUP_MEMBERS + 2],
            row[GROUP_MEMBERS + 3],
        ])),
    })
}

/// One shade row's room, or `None` when it is in no room.
fn decode_assignment(bytes: &[u8; ESTATE_RECORD_LEN], shade: usize) -> Option<RoomId> {
    match bytes[OFF_ROOM_OF + shade] {
        ROOM_NONE => None,
        room => Some(RoomId(room)),
    }
}

/// Every assignment names a room the record actually has.
///
/// Checked over the whole array rather than over the shade table's length,
/// because this record does not know how long that is — see
/// [`EstateRecord::room_of`]. An entry for a shade row that does not exist is
/// still required to name a real room, which costs nothing and keeps the
/// invariant one sentence long.
fn check_assignments(
    bytes: &[u8; ESTATE_RECORD_LEN],
    header: &EstateHeader,
) -> Result<(), EstateRecordError> {
    for shade in 0..ESTATE_SHADE_CAPACITY {
        if let Some(room) = decode_assignment(bytes, shade) {
            if room.0 as usize >= header.rooms {
                return Err(EstateRecordError::RoomIndex {
                    shade: ShadeId(shade as u8),
                    room: room.0,
                });
            }
        }
    }
    Ok(())
}

/// A length-prefixed name out of a row, checked rather than trusted: these
/// lengths come off a device.
fn read_name(
    row: &[u8],
    len_at: usize,
    name_at: usize,
    at: Row,
) -> Result<String<MAX_NAME_LEN>, EstateRecordError> {
    let len = row[len_at] as usize;
    if len > MAX_NAME_LEN {
        return Err(EstateRecordError::NameLength { at, len });
    }
    let text = core::str::from_utf8(&row[name_at..name_at + len])
        .map_err(|_| EstateRecordError::NotUtf8 { at })?;
    // Infallible: `len` has just been bounded by the string's own capacity.
    Ok(String::try_from(text).unwrap_or_default())
}

/// Four bytes at `at`, as an array. Panic-free by construction: every call site
/// passes an offset a compile-time assertion has already placed inside the
/// record.
fn word(bytes: &[u8; ESTATE_RECORD_LEN], at: usize) -> [u8; 4] {
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_slot_is_blank_and_not_damage() {
        let blank = [0xFFu8; ESTATE_RECORD_LEN];
        assert_eq!(EstateRecord::header(&blank), Err(EstateRecordError::Blank));
    }

    #[test]
    fn an_empty_estate_round_trips() {
        let record = EstateRecord::empty(7);
        assert_eq!(EstateRecord::decode(&record.encode()), Ok(record));
    }

    /// The property the ring depends on: a writer proves a write landed by
    /// reading the slot back and comparing bytes, so two equal records must
    /// encode identically.
    #[test]
    fn equal_estates_encode_identically() {
        let one = EstateRecord::empty(3);
        let two = EstateRecord::empty(3);
        assert_eq!(one.encode(), two.encode());
    }

    #[test]
    fn the_unassigned_sentinel_is_not_a_room_a_record_can_name() {
        assert!(ESTATE_ROOM_CAPACITY < ROOM_NONE as usize);
    }
}
