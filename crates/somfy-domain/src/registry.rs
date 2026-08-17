//! Slot-stable registry for shades, groups, and rooms.
//!
//! Fixed capacity — 32 shades, 16 groups, 16 rooms, and at most 32 shades
//! per group — sized to match what a deployed configuration can contain, so
//! any setup migrated from a real device always fits without truncation.
//! Ids are stable slot indices, not just array positions: a shade/group/
//! room must keep the same id even when *other* entries are removed, since
//! ids are stored elsewhere (group membership, backups) and must not
//! silently repoint at a different entry. We implement that contract with
//! `heapless::Vec<Option<T>, N>`: a [`ShadeId`]/[`GroupId`]/[`RoomId`] is
//! the slot index, a hole (`None`) is a removed entry, and a fresh add
//! reuses the lowest free slot before growing.
//!
//! ## Stable within a run is not the same as stable across runs
//!
//! The paragraph above is about one registry's lifetime: while the process
//! lives, an id survives its neighbours being removed. That is not the same
//! promise as an id surviving a **reboot**, and the difference is where shade
//! ids used to leak into a user's Home Assistant.
//!
//! A controller rebuilds its registry from a persisted table at every boot.
//! [`Registry::add_shade`] takes no id and assigns the lowest free slot, so
//! filling an empty registry from a table makes each shade's id its *row
//! position* — and `somfy-mqtt` names a retained discovery topic after that id.
//! Delete a row and every row after it shifts down a slot: entities are
//! renamed, and the topics they moved off keep their retained configs with no
//! device behind them.
//!
//! [`Registry::add_shade_with_id`] is the answer: the caller names the id, the
//! registry refuses a duplicate or an out-of-range one rather than quietly
//! picking something else, and whatever holds the ids becomes the authority on
//! them. Both adds exist because they answer different questions — "put this
//! shade wherever it fits" is right when a person is adding one to a running
//! controller, and "put this shade where it has always been" is right when a
//! record is being replayed.

use crate::{DomainError, Shade, ShadeConfig};
use heapless::{String, Vec};

/// Max shades a deployed configuration can contain, so a migrated setup
/// always fits.
pub const MAX_SHADES: usize = 32;
/// Max groups a deployed configuration can contain, so a migrated setup
/// always fits.
pub const MAX_GROUPS: usize = 16;
/// Max rooms a deployed configuration can contain, so a migrated setup
/// always fits.
pub const MAX_ROOMS: usize = 16;

/// Stable slot index of a shade in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadeId(pub u8);
/// Stable slot index of a group in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupId(pub u8);
/// Stable slot index of a room in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomId(pub u8);

struct Group {
    name: String<32>,
    /// Group membership, bounded at 32 members — the most shades a
    /// deployed group configuration can contain, so a migrated group
    /// always fits.
    members: Vec<ShadeId, MAX_SHADES>,
}

struct Room {
    name: String<32>,
    /// A room holds at most every shade once; bounded by the shade capacity.
    members: Vec<ShadeId, MAX_SHADES>,
}

/// Slot-array registry: ids are stable slot indices into fixed-capacity
/// storage. Removing an entry leaves a hole that the next add of the same
/// kind reuses, so live ids never shift.
#[derive(Default)]
pub struct Registry {
    shades: Vec<Option<Shade>, MAX_SHADES>,
    groups: Vec<Option<Group>, MAX_GROUPS>,
    rooms: Vec<Option<Room>, MAX_ROOMS>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Add a shade, letting the registry choose its id. Rejects a duplicate
    /// radio address ([`DomainError::DuplicateAddress`]) and a full registry
    /// ([`DomainError::RegistryFull`]). Reuses the lowest free slot before
    /// growing, keeping existing ids stable.
    ///
    /// **The id it chooses is a consequence of the order shades are added in**,
    /// so a caller that has an id of its own — one read back from a record, say
    /// — wants [`Registry::add_shade_with_id`] instead. See that method for why
    /// the difference reaches as far as Home Assistant.
    pub fn add_shade(&mut self, config: ShadeConfig) -> Result<ShadeId, DomainError> {
        if self.holds_address(config.address) {
            return Err(DomainError::DuplicateAddress);
        }
        let slot = self
            .shades
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.shades.len());
        if slot >= MAX_SHADES {
            return Err(DomainError::RegistryFull);
        }
        self.place_shade(slot, config)
    }

    /// Add a shade **at the id the caller names**, so that whatever holds the
    /// id is the authority on it and insertion order is not.
    ///
    /// ## Why this exists
    ///
    /// A shade's id is not an internal detail. `somfy-mqtt` builds the
    /// discovery topic `…/cover/<node>/shade_<id>/config` out of it, and Home
    /// Assistant names an entity after that topic. So an id that moves is an
    /// entity that is renamed, and — because discovery configs are published
    /// *retained* — the topic it moved off keeps its old config on the broker
    /// with no device behind it, which only a person with an MQTT client can
    /// clear.
    ///
    /// With [`Registry::add_shade`] the id is the lowest free slot at the
    /// moment of the call, so filling an empty registry from a table makes the
    /// id the row's *position*: delete the second row of three and the third
    /// shade takes the second's id, entity and all. Reading the id from the
    /// table instead makes deleting and reordering rows safe, which is the
    /// whole of the difference.
    ///
    /// ## What it refuses, and in which order
    ///
    /// - [`DomainError::IdOutOfRange`] if `id` names no slot ([`MAX_SHADES`] is
    ///   one past the last). Checked first: no config could make such a call
    ///   satisfiable, so reporting anything about the shade would point at the
    ///   wrong field.
    /// - [`DomainError::DuplicateAddress`] if another slot already holds that
    ///   radio address — the same rule, raised the same way, as
    ///   [`Registry::add_shade`], so moving a call site between the two cannot
    ///   change which error a bad address produces.
    /// - [`DomainError::DuplicateId`] if the slot is occupied. Refused rather
    ///   than overwritten: a table with two rows claiming one id does not say
    ///   which shade belongs there, and silently keeping the last would delete
    ///   a provisioned shade.
    ///
    /// [`DomainError::RegistryFull`] is **not** among them and cannot be: an id
    /// in range either names a hole, which this fills, or an occupant, which it
    /// refuses.
    pub fn add_shade_with_id(
        &mut self,
        id: ShadeId,
        config: ShadeConfig,
    ) -> Result<ShadeId, DomainError> {
        let slot = id.0 as usize;
        if slot >= MAX_SHADES {
            return Err(DomainError::IdOutOfRange);
        }
        if self.holds_address(config.address) {
            return Err(DomainError::DuplicateAddress);
        }
        if self.shades.get(slot).is_some_and(Option::is_some) {
            return Err(DomainError::DuplicateId);
        }
        self.place_shade(slot, config)
    }

    /// True if any live slot already answers to `address`.
    fn holds_address(&self, address: u32) -> bool {
        self.shades
            .iter()
            .flatten()
            .any(|s| s.config.address == address)
    }

    /// Put a shade in `slot`, growing the slot array with holes to reach it.
    ///
    /// Both callers have already established that `slot` is below
    /// [`MAX_SHADES`] and free, so neither push can fail — the vector's
    /// capacity *is* [`MAX_SHADES`]. The `RegistryFull` map is a backstop
    /// against that reasoning being invalidated later, not an expected path.
    fn place_shade(&mut self, slot: usize, config: ShadeConfig) -> Result<ShadeId, DomainError> {
        while self.shades.len() <= slot {
            self.shades
                .push(None)
                .map_err(|_| DomainError::RegistryFull)?;
        }
        self.shades[slot] = Some(Shade::new(config));
        Ok(ShadeId(slot as u8))
    }

    pub fn shade(&self, id: ShadeId) -> Option<&Shade> {
        self.shades.get(id.0 as usize)?.as_ref()
    }

    pub fn shade_mut(&mut self, id: ShadeId) -> Option<&mut Shade> {
        self.shades.get_mut(id.0 as usize)?.as_mut()
    }

    /// Remove a shade and drop it from every group and room. Returns
    /// [`DomainError::NotFound`] if the slot is empty or out of range.
    pub fn remove_shade(&mut self, id: ShadeId) -> Result<(), DomainError> {
        let slot = self
            .shades
            .get_mut(id.0 as usize)
            .ok_or(DomainError::NotFound)?;
        if slot.take().is_none() {
            return Err(DomainError::NotFound);
        }
        for g in self.groups.iter_mut().flatten() {
            g.members.retain(|m| *m != id);
        }
        for r in self.rooms.iter_mut().flatten() {
            r.members.retain(|m| *m != id);
        }
        Ok(())
    }

    /// Route an RX frame to a shade: matches the shade's own OR any linked
    /// remote address (`Shade::is_linked`). Pure read.
    pub fn shade_by_address(&self, addr: u32) -> Option<ShadeId> {
        self.shades
            .iter()
            .enumerate()
            .find(|(_, s)| s.as_ref().is_some_and(|s| s.is_linked(addr)))
            .map(|(i, _)| ShadeId(i as u8))
    }

    pub fn shades(&self) -> impl Iterator<Item = (ShadeId, &Shade)> {
        self.shades
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (ShadeId(i as u8), s)))
    }

    /// The shades an operator has reported working — a subset of
    /// [`Registry::shades`].
    ///
    /// # Why this is a named method and not a filter at each call site
    ///
    /// Because the filter is a *claim*, and a reader of the call site cannot
    /// check it. What it says is: these are the shades a motor has been taught
    /// to answer, as far as anybody has told us. The complement is shades whose
    /// address this controller invented and nobody has yet driven, which
    /// transmit perfectly and move nothing.
    ///
    /// **Everything that publishes a shade to the outside world walks this
    /// rather than [`Registry::shades`]** — the boot-time Home Assistant
    /// announcement and the runtime one both do — because an entity that
    /// accepts commands and drives nothing is worse than no entity at all. The
    /// local API deliberately does *not*: an unconfirmed shade has to be
    /// commandable, or there would be no way to test it and therefore no way to
    /// ever confirm it.
    ///
    /// See [`PairingState`](crate::PairingState) for why "an operator reported
    /// it" is the strongest claim a one-way protocol allows.
    pub fn confirmed_shades(&self) -> impl Iterator<Item = (ShadeId, &Shade)> {
        self.shades()
            .filter(|(_, shade)| shade.config.pairing_state.is_confirmed())
    }

    fn named<T>(name: &str, make: impl FnOnce(String<32>) -> T) -> Result<T, DomainError> {
        let mut n: String<32> = String::new();
        n.push_str(name).map_err(|_| DomainError::NameTooLong)?;
        Ok(make(n))
    }

    /// Add a group. Fails with [`DomainError::NameTooLong`] if `name` exceeds
    /// 32 bytes and [`DomainError::RegistryFull`] at capacity. Reuses the
    /// lowest free slot before growing, keeping existing ids stable.
    pub fn add_group(&mut self, name: &str) -> Result<GroupId, DomainError> {
        let group = Self::named(name, |name| Group {
            name,
            members: Vec::new(),
        })?;
        if let Some(slot) = self.groups.iter().position(Option::is_none) {
            self.groups[slot] = Some(group);
            return Ok(GroupId(slot as u8));
        }
        self.groups
            .push(Some(group))
            .map_err(|_| DomainError::RegistryFull)?;
        Ok(GroupId((self.groups.len() - 1) as u8))
    }

    /// Add a shade to a group (idempotent). Returns [`DomainError::NotFound`]
    /// if the shade or group slot is empty, [`DomainError::RegistryFull`] if the
    /// group already holds its maximum of 32 members.
    pub fn group_add_shade(&mut self, g: GroupId, s: ShadeId) -> Result<(), DomainError> {
        if self.shade(s).is_none() {
            return Err(DomainError::NotFound);
        }
        let group = self
            .groups
            .get_mut(g.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(DomainError::NotFound)?;
        if group.members.contains(&s) {
            return Ok(());
        }
        group.members.push(s).map_err(|_| DomainError::RegistryFull)
    }

    /// True if `g` names a live group slot. Lets a caller distinguish a
    /// missing group (no such slot) from an existing but empty one, since
    /// [`Registry::group_shades`] yields nothing for both.
    pub fn group_exists(&self, g: GroupId) -> bool {
        self.groups.get(g.0 as usize).is_some_and(Option::is_some)
    }

    pub fn group_shades(&self, g: GroupId) -> impl Iterator<Item = ShadeId> + '_ {
        self.groups
            .get(g.0 as usize)
            .and_then(Option::as_ref)
            .into_iter()
            .flat_map(|grp| grp.members.iter().copied())
    }

    /// Group name, or `None` if the slot is empty or out of range. Stored for
    /// Plan 3/5 serialization.
    pub fn group_name(&self, g: GroupId) -> Option<&str> {
        self.groups
            .get(g.0 as usize)
            .and_then(Option::as_ref)
            .map(|grp| grp.name.as_str())
    }

    /// Add a room. Fails with [`DomainError::NameTooLong`] if `name` exceeds
    /// 32 bytes and [`DomainError::RegistryFull`] at capacity. Reuses the
    /// lowest free slot before growing, keeping existing ids stable.
    pub fn add_room(&mut self, name: &str) -> Result<RoomId, DomainError> {
        let room = Self::named(name, |name| Room {
            name,
            members: Vec::new(),
        })?;
        if let Some(slot) = self.rooms.iter().position(Option::is_none) {
            self.rooms[slot] = Some(room);
            return Ok(RoomId(slot as u8));
        }
        self.rooms
            .push(Some(room))
            .map_err(|_| DomainError::RegistryFull)?;
        Ok(RoomId((self.rooms.len() - 1) as u8))
    }

    /// Assign a shade to a room. A shade lives in at most one room, so this
    /// first removes it from every room, then adds it to the target (moves, not
    /// duplicates).
    pub fn room_assign(&mut self, r: RoomId, s: ShadeId) -> Result<(), DomainError> {
        if self.shade(s).is_none() {
            return Err(DomainError::NotFound);
        }
        if self
            .rooms
            .get(r.0 as usize)
            .and_then(Option::as_ref)
            .is_none()
        {
            return Err(DomainError::NotFound);
        }
        for room in self.rooms.iter_mut().flatten() {
            room.members.retain(|m| *m != s);
        }
        let room = self
            .rooms
            .get_mut(r.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(DomainError::NotFound)?;
        // Infallible in practice: `members` is capped at MAX_SHADES and holds
        // DISTINCT shade ids (a shade lives in at most one room, and the retain
        // above just removed `s` from every room, including this target). With
        // at most MAX_SHADES distinct shades in existence, a room that no longer
        // contains `s` always has a free slot for it, so this push cannot
        // overflow. The `RegistryFull` map is a defensive backstop, not an
        // expected path.
        room.members.push(s).map_err(|_| DomainError::RegistryFull)
    }

    pub fn room_shades(&self, r: RoomId) -> impl Iterator<Item = ShadeId> + '_ {
        self.rooms
            .get(r.0 as usize)
            .and_then(Option::as_ref)
            .into_iter()
            .flat_map(|room| room.members.iter().copied())
    }

    /// Room name, or `None` if the slot is empty or out of range. Stored for
    /// Plan 3/5 serialization.
    pub fn room_name(&self, r: RoomId) -> Option<&str> {
        self.rooms
            .get(r.0 as usize)
            .and_then(Option::as_ref)
            .map(|room| room.name.as_str())
    }
}
