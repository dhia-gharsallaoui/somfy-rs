//! Slot-stable registry for shades, groups, and rooms.
//!
//! Port of the fixed-array containers in `SomfyShadeController` (Somfy.h:6-11):
//! `SOMFY_MAX_SHADES` 32, `SOMFY_MAX_GROUPS` 16, `SOMFY_MAX_ROOMS` 16, and
//! `SOMFY_MAX_GROUPED_SHADES` 32. The C++ addresses shades by their fixed array
//! slot, so an id must stay valid when *other* entries are removed. We model the
//! same contract with `heapless::Vec<Option<T>, N>`: a [`ShadeId`]/[`GroupId`]/
//! [`RoomId`] is the slot index, a hole (`None`) is a removed entry, and a fresh
//! add reuses the lowest free slot before growing.

use crate::{DomainError, Shade, ShadeConfig};
use heapless::{String, Vec};

/// Max shades (Somfy.h:6, `SOMFY_MAX_SHADES`).
pub const MAX_SHADES: usize = 32;
/// Max groups (Somfy.h:7, `SOMFY_MAX_GROUPS`).
pub const MAX_GROUPS: usize = 16;
/// Max rooms (Somfy.h:10, `SOMFY_MAX_ROOMS`).
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
    /// Group membership, bounded at `SOMFY_MAX_GROUPED_SHADES` = 32 (Somfy.h:9).
    members: Vec<ShadeId, MAX_SHADES>,
}

struct Room {
    name: String<32>,
    /// A room holds at most every shade once; bounded by the shade capacity.
    members: Vec<ShadeId, MAX_SHADES>,
}

/// Slot-array registry: ids are stable slot indices, like the C++ fixed arrays
/// in `SomfyShadeController` (Somfy.h). Removing an entry leaves a hole that the
/// next add of the same kind reuses, so live ids never shift.
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

    /// Add a shade. Rejects a duplicate radio address ([`DomainError::DuplicateAddress`])
    /// and a full registry ([`DomainError::RegistryFull`]). Reuses the lowest
    /// free slot before growing, keeping existing ids stable.
    pub fn add_shade(&mut self, config: ShadeConfig) -> Result<ShadeId, DomainError> {
        if self
            .shades
            .iter()
            .flatten()
            .any(|s| s.config.address == config.address)
        {
            return Err(DomainError::DuplicateAddress);
        }
        let shade = Shade::new(config);
        if let Some(slot) = self.shades.iter().position(Option::is_none) {
            self.shades[slot] = Some(shade);
            return Ok(ShadeId(slot as u8));
        }
        self.shades
            .push(Some(shade))
            .map_err(|_| DomainError::RegistryFull)?;
        Ok(ShadeId((self.shades.len() - 1) as u8))
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
    /// remote address (`Shade::is_linked`, Somfy.cpp:2191-2199). Pure read.
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
    /// group already holds `SOMFY_MAX_GROUPED_SHADES` members.
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
