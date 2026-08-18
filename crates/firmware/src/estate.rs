//! The persisted rooms and groups, on real flash.
//!
//! Same shape as [`crate::shades`] next door and, like that module before Plan
//! 6 gave it a writer, **read-only here**. Everything worth testing — the
//! record's encoding, its validity checks, the rules that decide whether a
//! decoded group is one the registry will accept — lives on the host side in
//! `somfy_config::EstateRecord`. What is left is flash I/O.
//!
//! ## Why there is no `store`
//!
//! Because there is nothing to call it. `crate::edits::ShadeEdit` is a
//! vocabulary of shade changes; rooms and groups have no edit, no API route
//! that creates one, and no screen. A write path with no producer is code that
//! has never run, and this file would rather say so than carry one.
//!
//! The host-side `provision_shades` example writes this region, from the same
//! import that writes the shade table beside it — and it must be the same
//! import, because a group's membership and a room assignment are **rows of
//! the shade table**. See `somfy_config::estate`'s module docs.
//!
//! ## Why an unreadable region does not stop the controller
//!
//! It costs less than any of the other three. Losing the rolling codes costs a
//! physical re-pairing at every motor, losing the shade table costs the ability
//! to command anything, losing the credentials costs the network — and losing
//! this costs the *arrangement*: every shade still exists, still moves and
//! still keeps its code, and what is gone is which room it is in and which
//! group it moves with. So a damaged or absent region is reported and the boot
//! continues with no rooms and no groups, which is also exactly what a board
//! that has never been imported into looks like.
//!
//! **A board flashed with an older partition table has no such region at all**,
//! and that is the ordinary case rather than a fault: this partition is new,
//! and every already-provisioned board is missing it until it is reflashed.
//! [`EstateStoreError::PartitionMissing`] is what says so.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{self, PartitionType};
use esp_storage::{FlashStorage, FlashStorageError};
use somfy_config::{EstateRecord, EstateRecordError, StoredGroup, StoredRoom, ESTATE_RECORD_LEN};
use somfy_domain::{GroupId, RoomId, ShadeId};
use somfy_store::{newest_slot, SectorRing, SlotWrite};

/// Partition holding the estate ring. Looked up by label for the same reason
/// the other three regions are: a compiled-in offset keeps working right up
/// until the app partition grows past it, and then reads rooms out of program
/// text.
pub const PARTITION_LABEL: &str = "estate";

/// Flash erase unit, and therefore the alignment the partition must sit on.
const SECTOR: usize = <FlashStorage as NorFlash>::ERASE_SIZE;

/// Largest ring this build will scan. The scan holds a sequence number per slot
/// in a fixed array, and a partly-scanned ring would sometimes name an older
/// record as newest. 16 slots is 32 KB, four times what `partitions.csv`
/// reserves.
const MAX_SLOTS: usize = 16;

/// Bytes of partition table read at mount. See [`crate::store`].
const PARTITION_TABLE_BYTES: usize = 1024;

// The same three relationships the other regions assert, restated for this
// record length. A divergence would neither fail to build nor fail to link.
const _: () = assert!(
    ESTATE_RECORD_LEN.is_multiple_of(<FlashStorage as NorFlash>::WRITE_SIZE),
    "an estate record must be a whole number of flash write words"
);
const _: () = assert!(
    SECTOR.is_multiple_of(ESTATE_RECORD_LEN),
    "estate records must tile the flash erase sector exactly"
);
const _: () = assert!(
    ESTATE_RECORD_LEN.is_multiple_of(<FlashStorage as ReadNorFlash>::READ_SIZE),
    "an estate record must be a whole number of flash read units"
);

/// Bytes compared at a time when reading a written record back.
///
/// The same figure and the same argument as `crate::shades`: 256 is one SPI NOR
/// page, it tiles a 2 KiB record exactly, and comparing the whole record at once
/// would need a second 2 KiB buffer on a stack that has one.
const VERIFY_WINDOW: usize = 256;

// The verification window has to tile the record and stay aligned, or the last
// comparison would run past the slot and `esp-storage` would answer an unaligned
// read by copying through a 4 KB buffer on this stack.
const _: () = assert!(
    ESTATE_RECORD_LEN.is_multiple_of(VERIFY_WINDOW)
        && VERIFY_WINDOW.is_multiple_of(<FlashStorage as ReadNorFlash>::READ_SIZE),
    "the read-back window must tile a record and be a whole number of read units"
);

/// Why the estate store could not do what was asked.
///
/// Each payload exists to be printed, and rustc's dead-code analysis
/// deliberately does not count a derived `Debug` as a read.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstateStoreError {
    /// The partition table could not be read or parsed.
    PartitionTable(partitions::Error),
    /// No partition labelled [`PARTITION_LABEL`]. **The ordinary state of every
    /// board provisioned before this region existed**, not a fault.
    PartitionMissing,
    /// The partition exists but is the wrong shape for the ring.
    PartitionGeometry { offset: u32, len: u32 },
    /// The flash refused a read.
    Flash(FlashStorageError),
    /// A slot index outside the ring. Unreachable — every index comes from the
    /// ring itself — but an error rather than a panic, because a panic here
    /// would take the radio off the air over a list of room names.
    SlotOutOfRange { slot: usize },
    /// The ring holds readable records and cannot say which is newest, so a
    /// write would have to restart the sequence counter from zero next to
    /// records numbered far higher — and `newest_slot`'s wrapping comparison
    /// would then rank the estate just written as the oldest thing in the
    /// region. The same refusal the other three regions make, for the same
    /// reason.
    Unstable { valid: usize },
    /// The bytes read back are not the bytes written. Not retried: an estate
    /// the flash did not take is one the caller has to be told about, because
    /// the alternative is a restore that reports success and leaves the rooms
    /// and groups as they were.
    NotDurable,
}

impl From<FlashStorageError> for EstateStoreError {
    fn from(error: FlashStorageError) -> Self {
        Self::Flash(error)
    }
}

/// What a scan of the estate ring found. A diagnostic, printed at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EstateSurvey {
    /// Slots in the ring.
    pub slots: usize,
    /// Slots holding a record that passed its checksum.
    pub valid: usize,
    /// Slots never written since the last erase.
    pub blank: usize,
    /// Slots holding something that is neither — a torn write, damage, or an
    /// estate this firmware refuses.
    pub damaged: usize,
    /// Sequence number of the newest valid record, if there is one.
    pub newest_seq: Option<u32>,
    /// Why the first non-blank slot that failed did so. Actionable in the same
    /// way the shade store's is: "group 1 is at address 0" names the group to
    /// re-import, where a bare damaged count leaves an operator guessing.
    pub first_error: Option<EstateRecordError>,
}

/// One slot's bytes, aligned so `esp-storage` reads them directly rather than
/// detouring through a 4 KB temporary on this stack.
#[repr(C, align(4))]
struct Slot([u8; ESTATE_RECORD_LEN]);

/// One read-back window, aligned for the same reason [`Slot`] is.
#[repr(C, align(4))]
#[cfg_attr(
    not(feature = "http"),
    allow(
        dead_code,
        reason = "only a restore and an export reach this, and both need a web \
                  server; a radio-only image has neither"
    )
)]
struct Window([u8; VERIFY_WINDOW]);

/// The flash-backed estate: where the ring is, and how it is carved up.
///
/// Holds no flash — see [`crate::shades`] for why one owner lends it rather
/// than two owning it.
pub struct EstateStore {
    /// Absolute flash offset of the partition.
    base: u32,
    ring: SectorRing,
}

impl EstateStore {
    /// Find the estate partition.
    ///
    /// **Call this from `main`, not from a task**, for the same reason the
    /// other three say so: the partition table costs about 1 KB of stack here
    /// plus `esp-storage`'s 4 KB sector buffer on the unaligned read path.
    pub fn mount(flash: &mut FlashStorage<'_>) -> Result<Self, EstateStoreError> {
        let capacity = flash.capacity() as u64;
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(flash, &mut buffer)
                .map_err(EstateStoreError::PartitionTable)?;
            let entry = table
                .iter()
                .find(|entry| {
                    // Label *and* type, so a label match alone cannot read an
                    // estate out of an app partition somebody named `estate`.
                    entry.label_as_str() == PARTITION_LABEL
                        && matches!(entry.partition_type(), PartitionType::Data(_))
                })
                .ok_or(EstateStoreError::PartitionMissing)?;
            (entry.offset(), entry.len())
        };

        let geometry = || EstateStoreError::PartitionGeometry { offset: base, len };
        // Checked in 64-bit arithmetic, because the point is to catch a table
        // written for a larger flash than the one it is being read on.
        if base as u64 + len as u64 > capacity {
            return Err(geometry());
        }
        if !(base as usize).is_multiple_of(SECTOR) {
            return Err(geometry());
        }
        let ring = SectorRing::new(len as usize, ESTATE_RECORD_LEN, SECTOR).ok_or_else(geometry)?;
        if ring.layout().slot_count() > MAX_SLOTS {
            return Err(geometry());
        }

        Ok(Self { base, ring })
    }

    /// Where the ring lives and how it is carved up: offset, slots, slot bytes.
    pub fn geometry(&self) -> (u32, usize, usize) {
        (
            self.base,
            self.ring.layout().slot_count(),
            self.ring.layout().slot_len(),
        )
    }

    /// Hand every room, room assignment and group in the newest readable estate
    /// to `visitor`, and report what the scan saw getting there.
    ///
    /// The order is the registry's: a room must exist before a shade can be
    /// assigned to it, and a group before a shade can join it.
    ///
    /// **Nothing here ever holds a whole decoded estate.** The slot's *bytes*
    /// live in one 2 KB buffer, and rows are decoded one at a time straight
    /// into whatever the visitor does with them.
    /// `somfy_config::EstateRecord`'s all-or-nothing rule still holds: an
    /// estate with one bad row places nothing, because a row's position is its
    /// id and skipping one renumbers the rest — and because each of the three
    /// walks validates the whole record before visiting anything, that holds
    /// across all three rather than within each.
    pub fn load_with(
        &mut self,
        flash: &mut FlashStorage<'_>,
        visitor: &mut impl EstateVisitor,
    ) -> Result<EstateSurvey, EstateStoreError> {
        let mut buffer = Slot([0u8; ESTATE_RECORD_LEN]);
        let scan = self.scan(flash, &mut buffer)?;

        let mut survey = EstateSurvey {
            slots: scan.valid + scan.blank + scan.damaged,
            valid: scan.valid,
            blank: scan.blank,
            damaged: scan.damaged,
            newest_seq: scan.newest.map(|(_, seq)| seq),
            first_error: scan.first_error,
        };

        if let Some((slot, _)) = scan.newest {
            // The winner is read a second time rather than kept from the first
            // pass, for the same reason the other stores do it: which slot wins
            // is only known once every sequence number is in hand.
            let offset = self.offset(slot)?;
            flash.read(offset, &mut buffer.0)?;
            // A record whose header passed a moment ago and whose rows do not
            // decode is reported rather than partly loaded. `first_error` is
            // overwritten deliberately: this is the estate that was going to be
            // used, so it is the error worth naming.
            //
            // Three walks rather than one, because the visitor is `&mut` and a
            // single call taking three closures would have to lend it three
            // times at once. They read the same buffer, so this is three passes
            // over 2 KB of RAM rather than three of flash.
            let placed = EstateRecord::for_each_room(&buffer.0, |id, room| visitor.room(id, room))
                .and_then(|_| {
                    EstateRecord::for_each_assignment(&buffer.0, |shade, room| {
                        visitor.assign(shade, room)
                    })
                })
                .and_then(|_| {
                    EstateRecord::for_each_group(&buffer.0, |id, group| visitor.group(id, group))
                });
            if let Err(error) = placed {
                survey.first_error = Some(error);
            }
        }
        Ok(survey)
    }

    /// Append `record` to the ring and prove the bytes landed.
    ///
    /// **The firmware did not write this region until a restore could.** Until
    /// then the host tool was the only writer, because an estate names shades by
    /// *row of the shade table* and one written beside a different table points
    /// at the wrong shades — so the two have always had to be written together,
    /// and the only thing that wrote both was `provision_shades`. A restore is
    /// the second thing that writes both, in the same order and from the same
    /// import, which is why this could be added without breaking that coupling.
    ///
    /// The sequence number is **this store's**, not the caller's: it comes from
    /// the ring's own `next_write`, so a caller cannot hand back a stale one and
    /// have a later record rank as older than the one it replaces. Whatever
    /// `record.seq` held is overwritten. `crate::shades::ShadeStore::store`
    /// carries the same rule and the same argument for it.
    #[cfg_attr(
        not(feature = "http"),
        allow(
            dead_code,
            reason = "only a restore and an export reach this, and both need a web \
                      server; a radio-only image has neither"
        )
    )]
    pub fn store(
        &mut self,
        flash: &mut FlashStorage<'_>,
        record: &EstateRecord,
    ) -> Result<u32, EstateStoreError> {
        let mut buffer = Slot([0u8; ESTATE_RECORD_LEN]);
        let scan = self.scan(flash, &mut buffer)?;

        // A write may not proceed on a ring that holds readable records and
        // cannot name a newest one. `crate::config::ConfigStore::store` carries
        // the full argument and it is the same one here: `next_write(None)` aims
        // at slot 0 with sequence 0, erasing a sector that may hold the only
        // readable estate and then writing a record every later scan will rank
        // as ancient.
        if scan.newest.is_none() && scan.valid > 0 {
            return Err(EstateStoreError::Unstable { valid: scan.valid });
        }

        let newest = scan.newest.map(|(slot, seq)| SlotWrite { slot, seq });
        let aim = self.ring.layout().next_write(newest);
        let slot_count = self.ring.layout().slot_count();
        let slot = self
            .ring
            .write_slot(aim.slot, &scan.free[..slot_count])
            .ok_or(EstateStoreError::SlotOutOfRange { slot: aim.slot })?;

        let stamped = EstateRecord {
            seq: aim.seq,
            rooms: record.rooms.clone(),
            room_of: record.room_of,
            groups: record.groups.clone(),
        };
        self.append(flash, slot, &stamped)?;
        Ok(aim.seq)
    }

    /// Append `record` at `slot`, erasing that slot's sector first if it starts
    /// one, then prove the bytes landed.
    #[cfg_attr(
        not(feature = "http"),
        allow(
            dead_code,
            reason = "only a restore and an export reach this, and both need a web \
                      server; a radio-only image has neither"
        )
    )]
    fn append(
        &mut self,
        flash: &mut FlashStorage<'_>,
        slot: usize,
        record: &EstateRecord,
    ) -> Result<(), EstateStoreError> {
        let offset = self.offset(slot)?;

        if let Some(sector) = self.ring.erase_before(slot) {
            let from = self.base + sector as u32;
            flash.erase(from, from + SECTOR as u32)?;
        }

        // Through `Slot` rather than straight from `encode`: a bare `[u8; N]` is
        // byte-aligned, and `esp-storage` answers an unaligned buffer by copying
        // it through a 4 KB sector buffer on this stack.
        let bytes = Slot(record.encode());
        flash.write(offset, &bytes.0)?;

        // Durability verified rather than assumed, and verified against the
        // *bytes* — which the format makes possible by guaranteeing that equal
        // records encode identically. Decoding the read-back instead would
        // compare two records and miss a padding byte the flash did not take.
        let mut window = Window([0u8; VERIFY_WINDOW]);
        for at in (0..ESTATE_RECORD_LEN).step_by(VERIFY_WINDOW) {
            flash.read(offset + at as u32, &mut window.0)?;
            if window.0 != bytes.0[at..at + VERIFY_WINDOW] {
                return Err(EstateStoreError::NotDurable);
            }
        }
        Ok(())
    }

    /// Where the newest readable record is, in absolute flash bytes.
    ///
    /// **For the backup export, and for nothing else.** A backup carries the
    /// estate record *verbatim*, so that the decoder reading it back is the
    /// same one the boot path already uses rather than a second reader that
    /// could disagree — and copying it verbatim means reading its bytes, not
    /// its fields. `crate::restore` streams them out sixty-four at a time.
    ///
    /// `None` is a region with nothing readable in it, which is an ordinary
    /// state for a board that has never been provisioned.
    #[cfg_attr(
        not(feature = "http"),
        allow(
            dead_code,
            reason = "only a restore and an export reach this, and both need a web \
                      server; a radio-only image has neither"
        )
    )]
    pub fn newest_offset(
        &mut self,
        flash: &mut FlashStorage<'_>,
    ) -> Result<Option<u32>, EstateStoreError> {
        let mut buffer = Slot([0u8; ESTATE_RECORD_LEN]);
        let scan = self.scan(flash, &mut buffer)?;
        scan.newest.map(|(slot, _)| self.offset(slot)).transpose()
    }

    /// Read every slot's header: which is newest, which are erased, and a tally.
    ///
    /// Headers only. Which slot wins is a question about four sequence numbers,
    /// and decoding four estates to answer it would cost stack this boot does
    /// not have.
    fn scan(
        &mut self,
        flash: &mut FlashStorage<'_>,
        buffer: &mut Slot,
    ) -> Result<Scan, EstateStoreError> {
        let slot_count = self.ring.layout().slot_count();
        let mut sequences = [None; MAX_SLOTS];
        let mut free = [false; MAX_SLOTS];
        let (mut valid, mut blank, mut damaged) = (0, 0, 0);
        let mut first_error = None;

        for (slot, sequence) in sequences.iter_mut().enumerate().take(slot_count) {
            let offset = self
                .ring
                .layout()
                .offset(slot)
                .map(|offset| self.base + offset as u32)
                .ok_or(EstateStoreError::SlotOutOfRange { slot })?;
            flash.read(offset, &mut buffer.0)?;
            match EstateRecord::header(&buffer.0) {
                Ok(header) => {
                    *sequence = Some(header.seq);
                    valid += 1;
                }
                Err(EstateRecordError::Blank) => {
                    blank += 1;
                    free[slot] = true;
                }
                Err(error) => {
                    damaged += 1;
                    first_error = first_error.or(Some(error));
                }
            }
        }

        // `newest_slot` owns the wrap-around comparison, so the ordering rule
        // stays in one host-tested place rather than being re-derived here.
        let newest = newest_slot(&sequences[..slot_count])
            .and_then(|slot| sequences[slot].map(|seq| (slot, seq)));
        Ok(Scan {
            newest,
            free,
            valid,
            blank,
            damaged,
            first_error,
        })
    }

    /// Absolute flash offset of `slot`.
    fn offset(&self, slot: usize) -> Result<u32, EstateStoreError> {
        self.ring
            .layout()
            .offset(slot)
            .map(|offset| self.base + offset as u32)
            .ok_or(EstateStoreError::SlotOutOfRange { slot })
    }
}

/// Where a loaded estate is put.
///
/// A trait rather than three closures, and the reason is the caller: it holds
/// one `&mut somfy_domain::Registry` and every one of the three needs it, which
/// three closures cannot express. One `&mut impl EstateVisitor` can, because the
/// borrow is the visitor's own.
pub trait EstateVisitor {
    /// A room, at the id the registry is expected to give it.
    fn room(&mut self, id: RoomId, room: StoredRoom);
    /// A shade's room. The shade is a **row of the shade table**, which is why
    /// this may only be called after the shades are in the registry.
    fn assign(&mut self, shade: ShadeId, room: RoomId);
    /// A group, at the id the registry is expected to give it, with its members
    /// as rows of the shade table.
    fn group(&mut self, id: GroupId, group: StoredGroup);
}

/// What one pass over the ring's headers found.
struct Scan {
    newest: Option<(usize, u32)>,
    /// Which slots are erased, so a write can step over the wreckage a torn
    /// write leaves rather than programming into it. `SectorRing::write_slot`
    /// carries the argument, and it is the reason a single power cut cannot
    /// wedge the ring permanently.
    #[cfg_attr(
        not(feature = "http"),
        allow(
            dead_code,
            reason = "read only by `store`, which exists for the restore path"
        )
    )]
    free: [bool; MAX_SLOTS],
    valid: usize,
    blank: usize,
    damaged: usize,
    first_error: Option<EstateRecordError>,
}
