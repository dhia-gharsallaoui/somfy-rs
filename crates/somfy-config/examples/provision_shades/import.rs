//! Reading a shade table out of an exported backup, instead of typing one.
//!
//! ## Why this exists, and what it takes out of a person's hands
//!
//! Every field the interactive path asks for can be got wrong and corrected:
//! a mistyped name is renamed, a wrong travel time is remeasured, a wrong kind
//! is re-provisioned. **The next rolling code is not one of those.** A motor
//! stores the last code it accepted and rejects anything at or below it as a
//! replay, so a value entered too low is a motor that ignores the controller
//! entirely — indistinguishable from a broken transmitter, and recoverable
//! only by walking to the shade and pairing it again.
//!
//! The controller being replaced already knows that value, and the backup it
//! exports carries it. So does its name, its address, its kind, and its
//! measured travel times. Reading them is strictly better than copying them by
//! eye from another device's screen, and the rolling code is the reason.
//!
//! ## What is carried, and into which of two images
//!
//! The backup describes a whole installation and this import writes **two**
//! records for **two** flash regions, from one read of one file:
//!
//! - the **shade table** — names, addresses, kinds, travel times, the
//!   next-to-send rolling code, and the wall remotes linked to each shade;
//! - the **estate** — the rooms, which room each shade is in, and the groups
//!   with their names, members and virtual-remote identities.
//!
//! They are written together because they are one thing: a group's membership
//! and a room assignment are both *rows of the shade table*, so an estate
//! beside a different table names the wrong shades. See
//! `somfy_config::EstateRecord`.
//!
//! **Still not written:** network credentials (the operator re-enters them —
//! design spec §3.4), the broker settings (a different region and a different
//! tool: `provision --from-backup`), and the live positions the old controller
//! was tracking, which a fresh controller has no business believing anyway — it
//! re-establishes them by driving a shade to an end stop.
//!
//! The one omission with teeth is the **"my" favourite**. `somfy_domain::Shade`
//! models it (`my_pos: Option<Pos>`) and acts on it — a `My` press with no
//! favourite set is a *no-op in the domain* while the motor still recalls its
//! own, so the position estimate silently walks away from the shade — but
//! `ShadeConfig` has no field to provision one into. [`Import`] therefore
//! counts the favourites it had to drop, so the tool can say so rather than let
//! a person discover it.
//!
//! Shade flags (sun and wind sensor bits, `SimMy`) are dropped without a count:
//! nothing in this firmware models any of them, so there is no behaviour to
//! lose and nothing a person could act on.
//!
//! ## Two things the backup cannot tell us, and only one of them matters
//!
//! Neither a **linked remote's** rolling code nor, on a version 19 to 22
//! backup, a **group's** is in the file: the old controller keeps both in NVS,
//! which an export does not include.
//!
//! They are not the same loss. A linked remote is only ever *listened to* — its
//! address is all that is needed to recognise its frames and move the position
//! estimate — so the missing code costs nothing, and the tool says so in one
//! line rather than warning about it. A group is *transmitted as*, so a
//! fabricated code is a group a motor will reject; that one is a per-group
//! warning ([`Caveat::FabricatedGroupCode`]) **and** a bit in the record
//! (`somfy_config::StoredGroup::code_recovered`), because the warning is read
//! once and the value is stored forever.
//!
//! ## Order is identity, and the backup's own ids are not it
//!
//! Shade ids come from position in this table — first entry is `ShadeId(0)`,
//! and Home Assistant's entity for it is `shade_0`. The backup's own shade ids
//! are whatever the old controller assigned and are **not** carried: a backup
//! holding shades 10 and 11 imports as `ShadeId(0)` and `ShadeId(1)`. Nothing
//! is lost by that — the old controller's entity names were its own — but a
//! second import of a *reordered* backup renames every entity after the change,
//! exactly as reordering by hand does.
//!
//! ## Rules this owes the reader, and why they are rules
//!
//! - **A kind or tilt mode this firmware does not model becomes a roller (or
//!   no tilt) and is reported per shade.** Dropping the shade would silently
//!   shrink the installation; guessing a behaviour would move a garage door
//!   with a shade's travel times. Substituting and saying so is the only one of
//!   the three a person can act on.
//! - **A frame width or a radio protocol other than the one this controller
//!   speaks is reported, because there is nowhere to put either.** The width is
//!   a single setting for the whole installation (`somfy_tasks::TxProfile`) and
//!   the protocol is not a setting at all — `somfy-rts` encodes one — while
//!   `ShadeConfig` has a field for neither. So a shade the old controller drove
//!   some other way imports looking perfectly healthy and will not move. That
//!   is the same failure as a wrong rolling code — a shade that ignores the
//!   controller — arriving by a different road, and it is worth exactly as much
//!   noise.
//! - **Records that did not align exactly are surfaced, never applied
//!   silently.** [`Import::misaligned`] means at least one record's fields did
//!   not land where they were expected — a comma inside a name shifts every
//!   field after it — and a shifted field is not obviously wrong. It is a
//!   *plausible* rolling code that is not the right one, which is the failure
//!   at the top of this file. The tool shows the table and demands confirmation.

use somfy_config::{
    EstateRecord, LinkedRemote, Members, ShadeError, StoredGroup, StoredRoom, StoredShade,
    ESTATE_GROUP_CAPACITY, ESTATE_ROOM_CAPACITY, MAX_LINKED_REMOTES, MAX_LINKS,
    SHADE_TABLE_CAPACITY,
};
use somfy_domain::{
    DomainError, FrameWidth, RadioProtocol, RoomId, ShadeConfig, ShadeId, ShadeKind, TiltMode,
};
use somfy_migrate::{
    parse_backup, MigrateError, MigrationData, MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION,
};

/// The lowest backup version whose file carries a **group's** rolling code.
///
/// Not a number picked here: it is where `somfy_migrate::parse_group_record`
/// stops fabricating. Below it the old controller keeps a group's code outside
/// the file it exports, so the parser writes
/// `RollingCode(1)` — a value indistinguishable from a real code that happens
/// to be 1. At this version and above, the file has the real one.
///
/// Everything [`Caveat::FabricatedGroupCode`] says follows from this one
/// comparison, which is why it is a named constant rather than a `>= 23`
/// buried in a branch.
const GROUP_CODE_MIN_VERSION: u8 = 23;

/// The room id a shade carries in a backup when it is in no room.
///
/// **Two values mean it, and both have to be honoured.** Deleting a room on the
/// old controller writes `0` into every shade that was in it, while a shade
/// that was never assigned one is written as `255`. Treating either as a room
/// to look up would produce a warning about a room nobody ever assigned.
const ROOM_UNASSIGNED: [u8; 2] = [0, 255];

/// The radio-protocol discriminant a shade must carry to be one this firmware
/// can drive.
///
/// **Where the value comes from, since an unexplained constant is a fabricated
/// one.** It is not read off a protocol table: it is the value the backup
/// reader substitutes when a shade record has no protocol field at all, which
/// `somfy_migrate::parse_shade_record` documents and applies for the versions
/// predating it. A file with no protocol field describes shades that were
/// driven the ordinary way, so the ordinary way is what this discriminant
/// spells — and `somfy-rts` encodes exactly one protocol, with no field on
/// `ShadeConfig` to select another. See `docs/provenance.md`.
///
/// The claim being made is therefore narrow and checkable: **a shade whose
/// stored protocol differs from the absent-field default was deliberately set
/// to something else, and this firmware cannot honour the difference.** What
/// the other values *are* is not asserted here, because nothing here needs it.
const TRANSMITTED_PROTOCOL: u8 = 0x00;

/// A value the backup carried that this firmware cannot use as it stands.
///
/// Three of these are substitutions — something else was written in the field,
/// and the operator is told what. The fourth is not, and could not be: there is
/// no field to substitute into.
///
/// **[`Caveat::FrameWidth`] used to be a fourth non-substitution and is not
/// any more.** The controller transmitted one width for the whole installation
/// when this tool was written, so a shade paired at the other one imported
/// looking healthy and never moved. Each shade now carries its own width and
/// is transmitted at it, so a *recognised* width is no longer a caveat at all —
/// what is left is a bit length that is not one of the protocol's two, which is
/// substituted like an unmodelled shade kind and reported for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caveat {
    /// A shade-kind byte outside the set this firmware models. Imported as
    /// [`ShadeKind::Roller`].
    Kind(u8),
    /// A tilt-mode byte outside the set this firmware models. Imported as
    /// [`TiltMode::None`].
    TiltMode(u8),
    /// A bit length that is not one of the protocol's two widths. Imported at
    /// [`somfy_domain::FrameWidth::Bits56`], which is `ShadeConfig::new`'s
    /// default and the width nearly every RTS motor in the field uses.
    ///
    /// **A width the protocol does have — 56 or 80 — is not a caveat**: the
    /// record carries it and the transmitter honours it per shade.
    FrameWidth(u8),
    /// A radio protocol other than [`TRANSMITTED_PROTOCOL`]. Nothing is
    /// substituted, because there is nothing to substitute into: `somfy-rts`
    /// has no byte layout for RTW, RTV or the general-purpose kinds at either
    /// width. The shade is provisioned, appears in Home Assistant, and does not
    /// move.
    Protocol(u8),
    /// A shade names a room the backup does not carry. Imported into no room.
    ///
    /// Should not occur: deleting a room on the old controller clears the room
    /// id on every shade that was in it. So this is either a hand-edited file
    /// or a record that did not align, and both are worth a line rather than a
    /// silent rearrangement of somebody's installation.
    UnknownRoom(u8),
    /// A group lists a shade the backup does not carry. Dropped from the group.
    ///
    /// **This one is expected**, and it is why a dangling member does not
    /// refuse the import: deleting a shade on the old controller clears its
    /// slot and does *not* remove its id from any group, so a group outliving
    /// its members is the ordinary state of a real installation. Dropping the
    /// member is the only thing that can be done with an id nothing answers to;
    /// saying so is what stops a group that quietly moves fewer shades than the
    /// old controller's did.
    MissingMember(u8),
    /// A group's rolling code could not be recovered from the backup, so the
    /// stored one is a fabrication.
    ///
    /// Backup format versions 19 to 22 keep a group's rolling code in NVS and
    /// not in the exported file, so `somfy_migrate::parse_group_record`
    /// substitutes `RollingCode(1)`. A motor rejects any code at or below the
    /// last it accepted, so **that group will not actuate** until it is
    /// re-paired or the real code is entered — and nothing about the number
    /// says so, because `1` is a value a real group could be at.
    ///
    /// It costs nothing today, because v1.0 executes a group command by
    /// transmitting to each member shade rather than as the group. It costs a
    /// walk to a motor the first time anything transmits as the group, which
    /// is why the fact is written into the record as well as printed here —
    /// see `somfy_config::StoredGroup::code_recovered`.
    FabricatedGroupCode {
        /// The backup's format version, which is why it could not be read.
        version: u8,
    },
}

impl core::fmt::Display for Caveat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Caveat::Kind(raw) => write!(
                f,
                "shade kind {raw:#04X} is not one this firmware models — imported as a roller, \
                 so it will be driven with a roller's commands and travel times"
            ),
            Caveat::TiltMode(raw) => write!(
                f,
                "tilt mode {raw:#04X} is not one this firmware models — imported as none, \
                 which is what every tilt mode does today in any case"
            ),
            Caveat::FrameWidth(bits) => write!(
                f,
                "bit length {bits} is not one of the protocol's two frame widths — imported at \
                 56-bit, which is what nearly every RTS motor uses, so a shade that was really \
                 paired at 80 will not respond"
            ),
            Caveat::Protocol(raw) => write!(
                f,
                "the old controller drove this shade with radio protocol {raw:#04X} and this \
                 one speaks only {TRANSMITTED_PROTOCOL:#04X} — there is no per-shade protocol \
                 to import it into, so the shade will be provisioned and will not respond"
            ),
            Caveat::UnknownRoom(raw) => write!(
                f,
                "this shade was in room {raw}, which the backup does not contain — imported \
                 into no room, so put it back in one on the new controller"
            ),
            Caveat::MissingMember(raw) => write!(
                f,
                "this group listed shade {raw}, which the backup does not contain — the old \
                 controller leaves a deleted shade in its groups, so the group is imported \
                 without it"
            ),
            Caveat::FabricatedGroupCode { version } => write!(
                f,
                "this group's rolling code is NOT in a version {version} backup — the old \
                 controller kept it outside the file — so the imported code is a placeholder \
                 and a motor would reject it as a replay. Nothing transmits as a group today, \
                 so this costs nothing yet; re-pair the group or set its code by hand before \
                 anything does"
            ),
        }
    }
}

/// What a [`Warning`] is about.
///
/// Two kinds rather than one, because the import now writes groups as well as
/// shades. The row is the id each will take on the device, so a warning names
/// what a person will see in the UI rather than what the old controller called
/// it.
///
/// **There is deliberately no room variant.** The one thing that can go wrong
/// with a room — a shade naming one the backup does not carry — is a fact about
/// the *shade*, and reporting it against the room would name the thing that is
/// missing rather than the thing that lost it. Everything else about a room is
/// either fine or a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// A shade, by its row in the imported table — which is its `ShadeId`.
    Shade(usize),
    /// A group, by its row — which is its `GroupId`.
    Group(usize),
}

impl core::fmt::Display for Subject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Subject::Shade(index) => write!(f, "ShadeId({index})"),
            Subject::Group(index) => write!(f, "GroupId({index})"),
        }
    }
}

/// One caveat, and which shade, room or group it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Which entity, and its row in the imported table — which is also the id
    /// it will have on the device.
    pub subject: Subject,
    /// Its name, so the warning names something a person recognises.
    pub name: String,
    /// What could not be carried across as it stands.
    pub caveat: Caveat,
}

/// A shade table recovered from a backup, and everything about the recovery a
/// person has to be told before it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The shades, in the order their ids will follow.
    pub shades: heapless::Vec<StoredShade, SHADE_TABLE_CAPACITY>,
    /// Every value that could not be carried across as it stands. Empty is the
    /// ordinary case.
    pub warnings: Vec<Warning>,
    /// Records whose fields did not align exactly. **Nonzero means at least one
    /// value in this table may be wrong**, including a rolling code — see the
    /// module docs.
    pub skipped_resyncs: u16,
    /// The backup's format version, for the report.
    pub version: u8,
    /// The rooms, the room each shade is in, and the groups — everything the
    /// backup describes that is not a shade.
    ///
    /// A **second record for a second region**, written from the same import
    /// as `shades` and only meaningful beside it: a group's membership and a
    /// room assignment are both *rows of the shade table*, so importing one
    /// without the other would leave references pointing at whatever was there
    /// before. See `somfy_config::EstateRecord`.
    pub estate: EstateRecord,
    /// Every linked remote the backup carried, ready for the record's pool.
    ///
    /// **Their rolling codes are not in the file** — the old controller kept
    /// those outside the backup — and that is fine here, because a linked
    /// remote is only ever *listened to*. Recognising a wall remote's frames
    /// needs its address; transmitting as one would need its code, and this
    /// controller never does that.
    pub links: heapless::Vec<LinkedRemote, MAX_LINKS>,
    /// Shades whose "my" favourite was set on the old controller and could not
    /// be provisioned — see this module's docs, which is where the consequence
    /// is. Counted rather than warned per shade: a favourite is common enough
    /// that one `!!` line each would bury the two caveats that mean a shade
    /// will not move at all.
    pub favourites: usize,
}

impl Import {
    /// Whether any record's fields failed to align, which is the condition that
    /// makes this table something to confirm rather than something to write.
    pub fn misaligned(&self) -> bool {
        self.skipped_resyncs > 0
    }
}

/// Why a backup was refused, in whole.
///
/// Every one of these refuses the **entire** table rather than importing what
/// it can. That is the same rule `ShadeRecord::for_each` enforces on the
/// device, and for the same reason: ids come from position, so dropping the
/// third shade renumbers the fourth and fifth, and in Home Assistant that is
/// half an installation quietly renamed to route around one bad field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The bytes are not a backup this tool can read.
    Unreadable(MigrateError),
    /// The backup parsed and holds no shades. An empty table is a thing an
    /// operator can mean — the interactive path writes one — but a backup with
    /// nothing in it is far likelier to be the wrong file.
    NoShades,
    /// More shades than the table holds. Unreachable while the parser's own
    /// capacity and [`SHADE_TABLE_CAPACITY`] are both 32 — the parser refuses
    /// first, as [`Refusal::Unreadable`] — and kept because it is the refusal
    /// that catches them ever differing.
    TooManyShades,
    /// A shade with no name. The interactive path cannot produce one either
    /// (an empty name is how a person ends the list), and an unnamed shade is
    /// an unnamed entity in Home Assistant.
    Unnamed {
        /// The shade's index in the backup's shade order.
        index: usize,
    },
    /// A shade the domain's own rules refuse: a sentinel address, a name that
    /// does not fit, or a travel time of zero.
    Shade {
        /// The shade's index in the backup's shade order.
        index: usize,
        /// Its name, so the refusal names something a person recognises.
        name: String,
        /// Why it was refused.
        error: ShadeError,
    },
    /// Two shades at one radio address. The record refuses such a table on the
    /// device, so importing it would produce a file that cannot be loaded.
    DuplicateAddress {
        /// The later of the two.
        index: usize,
        /// The earlier of the two.
        first: usize,
        /// The later one's name.
        name: String,
        /// The address they share.
        address: u32,
    },
    /// More linked remotes than one record can carry. The per-shade bound is
    /// the domain's seven and is not what runs out; the record's shared pool is
    /// [`somfy_config::MAX_LINKS`] across the whole table, and a big enough
    /// installation can exceed it. Refused rather than truncated: a dropped
    /// link is a wall remote whose presses stop correcting the position
    /// estimate, and nothing would say which one.
    TooManyLinks {
        /// Links the backup carried.
        wanted: usize,
        /// Links a record holds.
        held: usize,
    },
    /// A linked remote the domain's own rules refuse — a sentinel address, a
    /// duplicate, the shade's own address, or an eighth remote on one shade.
    Link {
        /// The shade's index in the imported table.
        index: usize,
        /// The shade's name.
        name: String,
        /// The remote's address.
        address: u32,
        /// Why it was refused.
        error: DomainError,
    },
    /// Two rooms with the same id in the backup, so a shade assigned to that
    /// id does not say which room it means.
    DuplicateRoomId {
        /// The later of the two rooms, by row.
        index: usize,
        /// Its name.
        name: String,
        /// The id they share.
        room_id: u8,
    },
    /// A group at an address no remote can have. A group **is** a virtual
    /// remote in the controller being replaced, allocated out of the same
    /// address space as the shades, so it is held to a remote's rule: `0` and
    /// `0xFFFFFF` are the sentinels the domain refuses.
    GroupAddress {
        /// The group's row in the imported table.
        index: usize,
        /// Its name.
        name: String,
        /// The address the backup carried.
        address: u32,
    },
    /// Two entities at one radio address, at least one of them a group.
    ///
    /// Refused rather than imported, because the record would then hold two
    /// rolling codes for one remote and no way to say which is current. It
    /// should not happen — the old controller's address allocator checks shades
    /// *and* groups before handing one out — so it is evidence of a hand-edited
    /// file or a record that did not align.
    GroupAddressClash {
        /// The group's row in the imported table.
        index: usize,
        /// Its name.
        name: String,
        /// The address it shares.
        address: u32,
        /// What else already holds it.
        with: Clash,
    },
    /// More rooms or groups than a record holds. Unreachable while the
    /// parser's own capacities and the record's are both 16 — the parser
    /// refuses first, as [`Refusal::Unreadable`] — and kept because it is the
    /// refusal that catches them ever differing.
    TooManyEstate {
        /// Which of the two ran out.
        what: &'static str,
        /// How many the record holds.
        held: usize,
    },
}

/// What a group's address collided with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clash {
    /// A shade, by its row in the imported table.
    Shade(usize),
    /// An earlier group, by its row.
    Group(usize),
    /// A wall remote linked to a shade, by that shade's row. A group
    /// transmitting at a wall remote's address is two remotes with one
    /// identity and two independent rolling counters, which is the failure
    /// this whole project was started over.
    LinkedRemote(usize),
}

impl core::fmt::Display for Clash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Clash::Shade(index) => write!(f, "shade {index}"),
            Clash::Group(index) => write!(f, "group {index}"),
            Clash::LinkedRemote(index) => {
                write!(f, "a wall remote linked to shade {index}")
            }
        }
    }
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refusal::Unreadable(MigrateError::UnsupportedVersion(version)) => write!(
                f,
                "the backup declares format version {version}; this tool reads \
                 {MIN_SUPPORTED_VERSION} to {MAX_SUPPORTED_VERSION}"
            ),
            Refusal::Unreadable(MigrateError::UnexpectedEof) => write!(
                f,
                "the backup ends in the middle of a record — it is truncated, and a truncated \
                 backup cannot be told from one whose last shade is missing"
            ),
            Refusal::Unreadable(MigrateError::StringTooLong) => write!(
                f,
                "a name in the backup is longer than the 32 bytes a shade name holds"
            ),
            Refusal::Unreadable(MigrateError::BadNumber) => {
                write!(f, "a numeric field in the backup could not be read")
            }
            Refusal::Unreadable(MigrateError::BadRecord("too_many_shades")) => write!(
                f,
                "the backup holds more shades than the {SHADE_TABLE_CAPACITY} this table has \
                 room for"
            ),
            // Every other `BadRecord` the parser raises is also a capacity
            // overflow — too many rooms, too many groups, too many remotes
            // linked to one shade — so the class is named and the parser's own
            // token is quoted, which is the thing to search for.
            Refusal::Unreadable(MigrateError::BadRecord(what)) => write!(
                f,
                "the backup holds more entries than the format allows, and the reader stopped \
                 at {what:?}"
            ),
            Refusal::NoShades => write!(
                f,
                "the backup holds no shades. If an empty table is what you meant, run this \
                 tool without --from-backup and enter an empty name at the first prompt"
            ),
            Refusal::TooManyShades => write!(
                f,
                "the backup holds more shades than the {SHADE_TABLE_CAPACITY} this table has \
                 room for"
            ),
            Refusal::Unnamed { index } => write!(
                f,
                "shade {index} in the backup has no name, and an unnamed shade is an unnamed \
                 entity; name it on the old controller and export again"
            ),
            Refusal::Shade { index, name, error } => {
                write!(f, "shade {index} {name:?}: {error}")
            }
            Refusal::DuplicateAddress {
                index,
                first,
                name,
                address,
            } => write!(
                f,
                "shade {index} {name:?} is at address {address} ({address:#08X}), which shade \
                 {first} already holds; the table would be refused on the device"
            ),
            Refusal::TooManyLinks { wanted, held } => write!(
                f,
                "the backup links {wanted} remotes to its shades and a record holds {held} in \
                 all; dropping the rest would silently stop those wall remotes correcting a \
                 shade's position, so unlink some on the old controller and export again"
            ),
            Refusal::Link {
                index,
                name,
                address,
                error,
            } => write!(
                f,
                "shade {index} {name:?} has a remote at address {address} ({address:#08X}) the \
                 device would refuse ({error:?})"
            ),
            Refusal::DuplicateRoomId {
                index,
                name,
                room_id,
            } => write!(
                f,
                "room {index} {name:?} has id {room_id}, which an earlier room already has; a \
                 shade assigned to it would not say which room it meant"
            ),
            Refusal::GroupAddress {
                index,
                name,
                address,
            } => write!(
                f,
                "group {index} {name:?} is at address {address} ({address:#08X}), which is not \
                 one a remote can have: 0 and 0xFFFFFF are reserved, and the field is 24 bits \
                 wide"
            ),
            Refusal::GroupAddressClash {
                index,
                name,
                address,
                with,
            } => write!(
                f,
                "group {index} {name:?} is at address {address} ({address:#08X}), which {with} \
                 already holds; two remotes at one address is two rolling-code counters that \
                 will overtake each other"
            ),
            Refusal::TooManyEstate { what, held } => write!(
                f,
                "the backup holds more {what} than the {held} this record has room for"
            ),
        }
    }
}

impl core::error::Error for Refusal {}

/// Read a backup's bytes as a shade table, or say why it is not one.
pub fn read_backup(bytes: &[u8]) -> Result<Import, Refusal> {
    let data = parse_backup(bytes).map_err(Refusal::Unreadable)?;
    import(&data)
}

/// Map already-parsed backup data onto the table this tool writes.
///
/// Split from [`read_backup`] so the mapping and refusal rules can be exercised
/// against constructed data, without a backup's bytes standing between the test
/// and the rule it is checking.
pub fn import(data: &MigrationData) -> Result<Import, Refusal> {
    if data.shades.is_empty() {
        return Err(Refusal::NoShades);
    }

    let mut warnings: Vec<Warning> = Vec::new();
    let mut estate = EstateRecord::empty(0);

    // The rooms first, because a shade carries the *backup's* room id and this
    // is what turns it into a row. The old controller's ids are not carried
    // for the same reason its shade ids are not: a row here is a `RoomId`
    // there, and a backup holding rooms 3 and 7 imports as rooms 0 and 1.
    let mut room_row: [Option<usize>; 256] = [None; 256];
    for room in data.rooms.iter() {
        let index = estate.rooms.len();
        if let Some(_earlier) = room_row[room.room_id as usize] {
            return Err(Refusal::DuplicateRoomId {
                index,
                name: room.name.as_str().to_string(),
                room_id: room.room_id,
            });
        }
        room_row[room.room_id as usize] = Some(index);
        estate
            .rooms
            .push(StoredRoom {
                // Infallible: both capacities are 32 bytes.
                name: heapless::String::try_from(room.name.as_str()).unwrap_or_default(),
            })
            .map_err(|_| Refusal::TooManyEstate {
                what: "rooms",
                held: ESTATE_ROOM_CAPACITY,
            })?;
    }

    let mut shades: heapless::Vec<StoredShade, SHADE_TABLE_CAPACITY> = heapless::Vec::new();
    let mut links: heapless::Vec<LinkedRemote, MAX_LINKS> = heapless::Vec::new();
    let mut wanted_links = 0usize;
    let mut favourites = 0usize;
    // The backup's shade ids, by row, so a group's membership can be resolved
    // the same way a room assignment is.
    //
    // **First row wins on a duplicate id**, which is a defensive path rather
    // than a policy: the old controller keys its shades by id — a delete
    // removes every slot matching one — so a file with two shades at one id is
    // one no writer produces. It is not a refusal because this import has
    // already
    // declared the backup's shade ids irrelevant (a row here is the id there),
    // and refusing on the strength of a field the tool does not carry would be
    // refusing on something it has said does not matter.
    let mut shade_row: [Option<usize>; 256] = [None; 256];

    for migrated in data.shades.iter() {
        // The position in the *imported* table, which is the shade's id and
        // what a warning names. Nothing is ever skipped — a shade that cannot
        // be imported refuses the whole table — so it is equally the shade's
        // position in the backup, which is what a refusal names. Taken from the
        // table rather than from an `enumerate` so that stays true by
        // construction rather than by everyone remembering it.
        let index = shades.len();
        let name = migrated.name.as_str();
        if name.is_empty() {
            return Err(Refusal::Unnamed { index });
        }
        let refuse = |error| Refusal::Shade {
            index,
            name: name.to_string(),
            error,
        };

        // Straight through the domain's own constructor, exactly as a
        // hand-entered shade goes, so this refuses precisely what the registry
        // refuses and the address and name rules live in one place.
        let mut config =
            ShadeConfig::new(name, migrated.address).map_err(|e| refuse(ShadeError::Domain(e)))?;

        let mut note = |caveat| {
            warnings.push(Warning {
                subject: Subject::Shade(index),
                name: name.to_string(),
                caveat,
            })
        };

        // The two substitutions. A kind or tilt mode outside the modelled set
        // is imported rather than dropped, and reported rather than assumed —
        // this module's docs say why those beat the alternatives.
        config.kind = ShadeKind::from_raw(migrated.kind_raw).unwrap_or_else(|| {
            note(Caveat::Kind(migrated.kind_raw));
            ShadeKind::Roller
        });
        config.tilt_mode = TiltMode::from_raw(migrated.tilt_mode_raw).unwrap_or_else(|| {
            note(Caveat::TiltMode(migrated.tilt_mode_raw));
            TiltMode::None
        });

        // The width, which the device now honours per shade. **Either of the
        // protocol's two widths imports silently**, because either is one this
        // controller transmits at — that is the whole of what changed when
        // `PlannedTx` started carrying the width, and it is why the old caveat
        // for an 80-bit shade is gone rather than reworded. What is left is a
        // bit length that is not a width at all, which falls back to the
        // constructor's default and is reported for the same reason an
        // unmodelled shade kind is.
        match FrameWidth::from_raw(migrated.bit_length) {
            Some(width) => config.frame_width = width,
            None => note(Caveat::FrameWidth(migrated.bit_length)),
        }
        match RadioProtocol::from_raw(migrated.proto_raw) {
            Some(protocol) if migrated.proto_raw == TRANSMITTED_PROTOCOL => {
                config.protocol = protocol
            }
            Some(protocol) => {
                config.protocol = protocol;
                note(Caveat::Protocol(migrated.proto_raw));
            }
            None => note(Caveat::Protocol(migrated.proto_raw)),
        }

        config.up_time_ms = migrated.up_time_ms;
        config.down_time_ms = migrated.down_time_ms;
        config.tilt_time_ms = migrated.tilt_time_ms;

        // An imported shade keeps the address the old controller was
        // transmitting at, so a motor already obeys it and the setup was
        // finished — on that controller, some time ago, by whoever installed
        // it. `crate::provisioned_pairing_state` carries the argument, and it
        // reads the address rather than the source, so a table that is part
        // import and part fresh allocation gets the right answer per shade
        // without this module having to know which is which.
        config.pairing_state = crate::provisioned_pairing_state(config.address);

        // The same constructor the device decodes through, so a shade this
        // accepts is a shade the device accepts — and the rolling code goes
        // across untouched, which is the whole point of reading a file instead
        // of a person's transcription of one.
        let shade = StoredShade::new(config, migrated.next_code).map_err(refuse)?;

        if let Some(first) = shades
            .iter()
            .position(|placed| placed.config.address == shade.config.address)
        {
            return Err(Refusal::DuplicateAddress {
                index,
                first,
                name: name.to_string(),
                address: shade.config.address,
            });
        }
        let address = shade.config.address;
        shades.push(shade).map_err(|_| Refusal::TooManyShades)?;

        // The wall remotes. **This is the only feedback path this controller
        // has**: RTS is one-way, nothing asks a motor where it is, and a shade
        // whose remotes are unknown decodes their frames, matches them against
        // nothing, and lets its position estimate drift with nothing to say so.
        // The backup carries the addresses, which is the half that matters —
        // the rolling codes are not in the file and are not needed, because a
        // linked remote is listened to and never transmitted as.
        wanted_links += migrated.linked_addresses.len();
        let mut linked_here = 0usize;
        for remote in migrated.linked_addresses.iter().copied() {
            let refuse_link = |error| Refusal::Link {
                index,
                name: name.to_string(),
                address: remote,
                error,
            };
            // The domain's own rules for `Shade::link_remote`, applied here so
            // a table this tool writes is one the device loads whole. A linked
            // remote is **not** a shade and never becomes one: it shares a
            // motor, not an identity, so nothing below touches `shades`.
            if remote == 0 || remote >= 0xFF_FFFF {
                return Err(refuse_link(DomainError::InvalidAddress));
            }
            if remote == address
                || links
                    .iter()
                    .any(|held| held.shade.0 as usize == index && held.address == remote)
            {
                return Err(refuse_link(DomainError::DuplicateAddress));
            }
            linked_here += 1;
            if linked_here > MAX_LINKED_REMOTES {
                return Err(refuse_link(DomainError::RegistryFull));
            }
            links
                .push(LinkedRemote {
                    shade: ShadeId(index as u8),
                    address: remote,
                })
                .map_err(|_| Refusal::TooManyLinks {
                    wanted: wanted_links,
                    held: MAX_LINKS,
                })?;
        }
        // The backup's unset favourite is a negative sentinel, so any position
        // at or above zero is one the old controller was actually holding.
        if migrated.my_position_centi >= 0 {
            favourites += 1;
        }

        shade_row[migrated.shade_id as usize].get_or_insert(index);

        // Which room this shade is in, translated from the backup's room id to
        // the row that will be its `RoomId`.
        if !ROOM_UNASSIGNED.contains(&migrated.room_id) {
            match room_row[migrated.room_id as usize] {
                Some(row) => estate.room_of[index] = Some(RoomId(row as u8)),
                None => warnings.push(Warning {
                    subject: Subject::Shade(index),
                    name: name.to_string(),
                    caveat: Caveat::UnknownRoom(migrated.room_id),
                }),
            }
        }
    }

    // The groups last: every one of them refers to shade rows, so the shade
    // table has to be settled first.
    for migrated in data.groups.iter() {
        let index = estate.groups.len();
        let name = migrated.name.as_str();

        // A group is a virtual remote, so its address is held to a remote's
        // rule — the same one `ShadeConfig::new` applies, restated here rather
        // than reached through a constructor because `ShadeConfig` is a shade
        // and a group is not one.
        if migrated.address == 0 || migrated.address >= 0xFF_FFFF {
            return Err(Refusal::GroupAddress {
                index,
                name: name.to_string(),
                address: migrated.address,
            });
        }
        let clash = shades
            .iter()
            .position(|shade| shade.config.address == migrated.address)
            .map(Clash::Shade)
            .or_else(|| {
                estate
                    .groups
                    .iter()
                    .position(|group| group.address == migrated.address)
                    .map(Clash::Group)
            })
            .or_else(|| {
                links
                    .iter()
                    .find(|link| link.address == migrated.address)
                    .map(|link| Clash::LinkedRemote(link.shade.0 as usize))
            });
        if let Some(with) = clash {
            return Err(Refusal::GroupAddressClash {
                index,
                name: name.to_string(),
                address: migrated.address,
                with,
            });
        }

        let mut members = Members::NONE;
        for id in migrated.member_shade_ids.iter().copied() {
            match shade_row[id as usize] {
                Some(row) => members = members.with(ShadeId(row as u8)),
                // Expected rather than exceptional — see `Caveat::MissingMember`.
                None => warnings.push(Warning {
                    subject: Subject::Group(index),
                    name: name.to_string(),
                    caveat: Caveat::MissingMember(id),
                }),
            }
        }

        // The one warning here that is about a value rather than a reference,
        // and the one this task exists for.
        let code_recovered = data.version >= GROUP_CODE_MIN_VERSION;
        if !code_recovered {
            warnings.push(Warning {
                subject: Subject::Group(index),
                name: name.to_string(),
                caveat: Caveat::FabricatedGroupCode {
                    version: data.version,
                },
            });
        }

        estate
            .groups
            .push(StoredGroup {
                // Infallible: both capacities are 32 bytes.
                name: heapless::String::try_from(name).unwrap_or_default(),
                address: migrated.address,
                next_code: migrated.next_code,
                code_recovered,
                members,
            })
            .map_err(|_| Refusal::TooManyEstate {
                what: "groups",
                held: ESTATE_GROUP_CAPACITY,
            })?;
    }

    Ok(Import {
        shades,
        warnings,
        skipped_resyncs: data.skipped_resyncs,
        version: data.version,
        estate,
        links,
        favourites,
    })
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
