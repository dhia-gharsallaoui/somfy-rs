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
//! ## What is not carried across
//!
//! The backup describes a whole installation; this record holds shades. **Not
//! written here:** rooms, groups, network settings, each shade's room
//! assignment, and the rolling codes of remotes linked to a shade — those last
//! are not in the exported file at all. Nor are the live positions the old
//! controller was tracking, which a fresh controller has no business believing
//! anyway: it re-establishes them by driving a shade to an end stop.
//!
//! The one omission with teeth is the **"my" favourite**. `somfy_domain::Shade`
//! models it (`my_pos: Option<Pos>`) and acts on it — a `My` press with no
//! favourite set is a *no-op in the domain* while the motor still recalls its
//! own, so the position estimate silently walks away from the shade — but
//! `ShadeConfig` has no field to provision one into. [`Import`] therefore
//! counts the favourites it had to drop, along with the rooms and groups and
//! linked remotes, so the tool can say so rather than let a person discover it.
//!
//! Shade flags (sun and wind sensor bits, `SimMy`) are dropped without a count:
//! nothing in this firmware models any of them, so there is no behaviour to
//! lose and nothing a person could act on.
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
    LinkedRemote, ShadeError, StoredShade, MAX_LINKED_REMOTES, MAX_LINKS, SHADE_TABLE_CAPACITY,
};
use somfy_domain::{
    DomainError, FrameWidth, RadioProtocol, ShadeConfig, ShadeId, ShadeKind, TiltMode,
};
use somfy_migrate::{
    parse_backup, MigrateError, MigrationData, MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION,
};

/// The frame width this controller transmits, and therefore the only one a
/// shade can be imported as. It is a single setting for the whole installation
/// — `somfy_tasks::TxProfile::default` — so a shade paired at any other width
/// is carried across as data and cannot be driven.
const TRANSMITTED_BIT_LENGTH: u8 = 56;

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
/// Two of these are substitutions — something else was written in the field.
/// The third is not, and could not be: there is no field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caveat {
    /// A shade-kind byte outside the set this firmware models. Imported as
    /// [`ShadeKind::Roller`].
    Kind(u8),
    /// A tilt-mode byte outside the set this firmware models. Imported as
    /// [`TiltMode::None`].
    TiltMode(u8),
    /// A frame width other than [`TRANSMITTED_BIT_LENGTH`]. Nothing is
    /// substituted, because `ShadeConfig` has no width to substitute into: the
    /// shade is imported exactly as it stands and will be transmitted to at the
    /// controller's width, which is not the one its motor is paired for.
    FrameWidth(u8),
    /// A radio protocol other than [`TRANSMITTED_PROTOCOL`]. Nothing is
    /// substituted, for the same reason and with the same consequence: the
    /// shade is provisioned, appears in Home Assistant, and does not move.
    Protocol(u8),
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
                "the old controller drove this shade with {bits}-bit frames and this one sends \
                 {TRANSMITTED_BIT_LENGTH}-bit — there is no per-shade width to import it into, \
                 so the shade will be provisioned and will not respond"
            ),
            Caveat::Protocol(raw) => write!(
                f,
                "the old controller drove this shade with radio protocol {raw:#04X} and this \
                 one speaks only {TRANSMITTED_PROTOCOL:#04X} — there is no per-shade protocol \
                 to import it into, so the shade will be provisioned and will not respond"
            ),
        }
    }
}

/// One caveat, and which shade it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// The shade's index in the imported table, which is also its `ShadeId`.
    pub index: usize,
    /// The shade's name, so the warning names something a person recognises.
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
    /// Rooms the backup carried. None are written here, and neither is any
    /// shade's room assignment.
    pub rooms: usize,
    /// Groups the backup carried. None are written here.
    pub groups: usize,
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

    let mut shades: heapless::Vec<StoredShade, SHADE_TABLE_CAPACITY> = heapless::Vec::new();
    let mut links: heapless::Vec<LinkedRemote, MAX_LINKS> = heapless::Vec::new();
    let mut wanted_links = 0usize;
    let mut warnings: Vec<Warning> = Vec::new();
    let mut favourites = 0usize;

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
                index,
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

        // And the two that used to be neither substituted nor stored: a width
        // and a protocol the record now carries. Both still mean a shade that
        // is provisioned and inert — the transmit width is per-controller and
        // only RTS is implemented — so both are still reported. What has
        // changed is that the value survives to the device, which is what lets
        // the device say which shade it cannot drive instead of being silent.
        match FrameWidth::from_raw(migrated.bit_length) {
            // The ordinary case: a width this controller transmits.
            Some(width) if migrated.bit_length == TRANSMITTED_BIT_LENGTH => {
                config.frame_width = width
            }
            // A real width this controller does not transmit. Stored
            // faithfully, and reported because the shade will not move.
            Some(width) => {
                config.frame_width = width;
                note(Caveat::FrameWidth(migrated.bit_length));
            }
            // Not a frame width at all. Left at the constructor's default and
            // reported, for the same reason an unmodelled shade kind is.
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
    }

    Ok(Import {
        shades,
        warnings,
        skipped_resyncs: data.skipped_resyncs,
        version: data.version,
        rooms: data.rooms.len(),
        groups: data.groups.len(),
        links,
        favourites,
    })
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
