//! The persisted shade table, on real flash.
//!
//! Same shape as [`crate::config`] and [`crate::store`], and for the same
//! reason: everything worth testing — the record's encoding, its validity
//! check, the rules that decide whether a decoded shade is one the registry
//! will accept, the slot arithmetic — lives on the host side in `somfy-config`
//! and `somfy-store`. What is left here is flash I/O.
//!
//! ## This region is now writable, and what that changed
//!
//! It used to be read-only to the firmware, written only by the host-side
//! `provision_shades` example and flashed with `espflash write-bin`. That is
//! what made "add a shade" a task requiring a cable, and it is what
//! [`ShadeStore::store`] ends.
//!
//! A firmware that can write can also corrupt, so the write path is the same
//! one the rolling-code store already uses and has been proved on hardware
//! with: erase the sector only when the slot starts one, step over the wreckage
//! a torn write leaves rather than writing into it, and **read the bytes back
//! and compare before believing them**. What is different is the verification:
//! this record's encoding is deterministic — equal records encode identically,
//! which the format guarantees so that exactly this check is possible — so the
//! read-back is compared byte for byte instead of decoded. That is both
//! stronger and cheaper: it catches a difference in a padding byte, and it
//! needs a 256-byte window rather than a second 2 KB buffer.
//!
//! ## Where the flash comes from
//!
//! Nowhere here. This store holds a partition offset and a ring, and every
//! operation takes the flash as an argument.
//!
//! The reason is that there is exactly one flash peripheral and
//! [`crate::store::FlashStore`] owns it for the life of the program — it must,
//! because a rolling code has to be committed before every transmission. A
//! second owner is not expressible, and inventing one with `steal` would be an
//! `unsafe` assertion that two writers to one chip never overlap. Borrowing
//! makes it a fact: [`FlashStore::with_flash`](crate::store::FlashStore::with_flash)
//! hands out `&mut FlashStorage` for the length of one call, so a shade write
//! cannot be interleaved with a rolling-code commit by construction rather than
//! by scheduling.
//!
//! ## Why an unreadable region does not stop the controller
//!
//! Same answer as [`crate::config`], for the same reason: losing this costs the
//! ability to command shades until somebody re-provisions, while
//! [`crate::store`] refuses on damage because losing *that* costs a physical
//! re-pairing at every motor. So this store reports damage and answers "no
//! shades", and the radio keeps receiving and decoding either way.
//!
//! The one place the two meet is seeding, and there the strict rule wins:
//! `somfy_store::seed_if_absent` is told what this survey found, and a
//! rolling-code region reporting damage is not seeded at all.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{self, PartitionType};
use esp_storage::{FlashStorage, FlashStorageError};
use somfy_config::{LinkedRemote, ShadeRecord, ShadeRecordError, StoredShade, SHADE_RECORD_LEN};
use somfy_store::{newest_slot, SectorRing, SlotWrite};

/// Partition holding the shade ring. Defined by `partitions.csv` in this crate,
/// and looked up by label for the same reason the other two regions are: a
/// compiled-in offset keeps working right up until the app partition grows past
/// it, and then reads shades out of program text.
pub const PARTITION_LABEL: &str = "shades";

/// Flash erase unit, and therefore the alignment the partition must sit on.
const SECTOR: usize = <FlashStorage as NorFlash>::ERASE_SIZE;

/// Largest ring this build will scan, for the same reason [`crate::store`] has
/// one: the scan holds a sequence number per slot in a fixed array, and a
/// partly-scanned ring would sometimes name an older record as newest. 16 slots
/// is 32 KB, four times what `partitions.csv` reserves.
const MAX_SLOTS: usize = 16;

/// Bytes of partition table read at mount. See [`crate::store`].
const PARTITION_TABLE_BYTES: usize = 1024;

/// Bytes compared at a time when proving a write landed.
///
/// A whole second slot buffer would be the obvious way and costs 2 KB of stack
/// next to the 2 KB the encoded record already occupies, on a chip whose main
/// stack is 14,588 bytes. A window costs 256 and answers the same question,
/// because the comparison is over bytes that are all in hand.
const VERIFY_WINDOW: usize = 256;

// The same three relationships the other two regions assert, restated for this
// record length. A divergence would neither fail to build nor fail to link.
const _: () = assert!(
    SHADE_RECORD_LEN.is_multiple_of(<FlashStorage as NorFlash>::WRITE_SIZE),
    "a shade record must be a whole number of flash write words"
);
const _: () = assert!(
    SECTOR.is_multiple_of(SHADE_RECORD_LEN),
    "shade records must tile the flash erase sector exactly"
);
const _: () = assert!(
    SHADE_RECORD_LEN.is_multiple_of(<FlashStorage as ReadNorFlash>::READ_SIZE),
    "a shade record must be a whole number of flash read units"
);
// The verification window has to tile the record and stay aligned, or the last
// comparison would run past the slot and `esp-storage` would answer an
// unaligned read by copying through a 4 KB buffer on this stack.
const _: () = assert!(
    SHADE_RECORD_LEN.is_multiple_of(VERIFY_WINDOW)
        && VERIFY_WINDOW.is_multiple_of(<FlashStorage as ReadNorFlash>::READ_SIZE),
    "the read-back window must tile a record and be a whole number of read units"
);

/// Why the shade store could not do what was asked.
///
/// Each payload exists to be printed, and rustc's dead-code analysis
/// deliberately does not count a derived `Debug` as a read.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeStoreError {
    /// The partition table could not be read or parsed.
    PartitionTable(partitions::Error),
    /// No partition labelled [`PARTITION_LABEL`]. The device was flashed
    /// without this crate's `partitions.csv`, or with an older one — which is
    /// the ordinary state of a board provisioned before shades existed.
    PartitionMissing,
    /// The partition exists but is the wrong shape for the ring.
    PartitionGeometry { offset: u32, len: u32 },
    /// The flash refused a read or a write.
    Flash(FlashStorageError),
    /// A slot index outside the ring. Unreachable — every index comes from the
    /// ring itself — but an error rather than a panic, because a panic here
    /// would take the radio off the air over a shade table.
    SlotOutOfRange { slot: usize },
    /// The ring holds readable records and cannot say which is newest, so a
    /// write would have to restart the sequence counter from zero next to
    /// records numbered far higher — and `newest_slot`'s wrapping comparison
    /// would then rank the shade table just written as the oldest thing in the
    /// region. The same refusal `crate::config::ConfigStore::store` makes, for
    /// the same reason.
    Unstable { valid: usize },
    /// The bytes read back are not the bytes written. Nothing is retried here:
    /// a table the flash did not take is a table the caller must be told about,
    /// because the alternative is a controller that believes in a shade the
    /// next boot will not find.
    NotDurable,
}

impl From<FlashStorageError> for ShadeStoreError {
    fn from(error: FlashStorageError) -> Self {
        Self::Flash(error)
    }
}

/// What a scan of the shade ring found. A diagnostic, printed at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShadeSurvey {
    /// Slots in the ring.
    pub slots: usize,
    /// Slots holding a record that passed its checksum and its validation.
    pub valid: usize,
    /// Slots never written since the last erase.
    pub blank: usize,
    /// Slots holding something that is neither — a torn write, damage, or a
    /// table this firmware refuses.
    pub damaged: usize,
    /// Sequence number of the newest valid record, if there is one.
    pub newest_seq: Option<u32>,
    /// Why the first non-blank slot that failed did so.
    ///
    /// Kept because the reasons here are *actionable* in a way the other two
    /// regions' are not: "entry 2 carries a zero down_time_ms" names the shade
    /// to re-provision, while a bare damaged count leaves an operator guessing
    /// which of them the tool refused.
    pub first_error: Option<ShadeRecordError>,
}

/// One slot's bytes, aligned so `esp-storage` reads them directly rather than
/// detouring through a 4 KB temporary on this stack.
#[repr(C, align(4))]
struct Slot([u8; SHADE_RECORD_LEN]);

/// One comparison window, aligned for the same reason.
#[repr(C, align(4))]
struct Window([u8; VERIFY_WINDOW]);

/// The flash-backed shade table: where the ring is, and how it is carved up.
///
/// Holds no flash — see this module's docs for why one owner lends it rather
/// than two owning it.
pub struct ShadeStore {
    /// Absolute flash offset of the partition.
    base: u32,
    ring: SectorRing,
}

impl ShadeStore {
    /// Find the shade partition.
    ///
    /// **Call this from `main`, not from a task**, for the same reason the
    /// other two say so: the partition table costs about 1 KB of stack here
    /// plus `esp-storage`'s 4 KB sector buffer on the unaligned read path.
    pub fn mount(flash: &mut FlashStorage<'_>) -> Result<Self, ShadeStoreError> {
        let capacity = flash.capacity() as u64;
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(flash, &mut buffer)
                .map_err(ShadeStoreError::PartitionTable)?;
            let entry = table
                .iter()
                .find(|entry| {
                    // Label *and* type, so a label match alone cannot read a
                    // shade table out of an app partition somebody named
                    // `shades`.
                    entry.label_as_str() == PARTITION_LABEL
                        && matches!(entry.partition_type(), PartitionType::Data(_))
                })
                .ok_or(ShadeStoreError::PartitionMissing)?;
            (entry.offset(), entry.len())
        };

        let geometry = || ShadeStoreError::PartitionGeometry { offset: base, len };
        // Checked in 64-bit arithmetic, because the point is to catch a table
        // written for a larger flash than the one it is being read on.
        if base as u64 + len as u64 > capacity {
            return Err(geometry());
        }
        if !(base as usize).is_multiple_of(SECTOR) {
            return Err(geometry());
        }
        // A `SectorRing` rather than a bare `SlotLayout`: the geometry rules it
        // enforces are what a writer needs, and this store now has one.
        let ring = SectorRing::new(len as usize, SHADE_RECORD_LEN, SECTOR).ok_or_else(geometry)?;
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

    /// Hand every shade in the newest readable table to `shade`, every linked
    /// remote to `link`, and report what the scan saw getting there.
    ///
    /// A survey with no `newest_seq` means no slot holds a readable table —
    /// what a board that has never been provisioned with shades looks like, and
    /// also what one whose table is damaged looks like. The rest of the survey
    /// is what tells those apart, and the caller prints it either way.
    ///
    /// **Nothing here ever holds a whole decoded table.** One is 2,320 bytes,
    /// against a main stack of 14,588 on the tightest chip this builds for and
    /// an 8,016-byte `StateMachine` already standing in `main`'s frame. So the
    /// slot's *bytes* live here — one 2 KB buffer, reused for every slot — and
    /// the shades are decoded one at a time straight into whatever `shade`
    /// does with them, which is 72 bytes at once. `somfy_config::ShadeRecord`'s
    /// all-or-nothing rule still holds: a table with one bad entry — or one bad
    /// link — visits nothing.
    ///
    /// The links are visited **after** every shade, which is the order the
    /// caller needs: a remote cannot be linked to a shade that is not in the
    /// registry yet.
    pub fn load_with(
        &mut self,
        flash: &mut FlashStorage<'_>,
        shade: impl FnMut(usize, StoredShade),
        link: impl FnMut(LinkedRemote),
    ) -> Result<(ShadeSurvey, Option<ShadeRecordHeader>), ShadeStoreError> {
        let mut buffer = Slot([0u8; SHADE_RECORD_LEN]);
        let scan = self.scan(flash, &mut buffer)?;

        let mut survey = ShadeSurvey {
            slots: scan.valid + scan.blank + scan.damaged,
            valid: scan.valid,
            blank: scan.blank,
            damaged: scan.damaged,
            newest_seq: scan.newest.map(|(_, seq)| seq),
            first_error: scan.first_error,
        };

        let mut header = None;
        if let Some((slot, _)) = scan.newest {
            // The winner is read a second time rather than kept from the first
            // pass, for the same reason the other two stores do it: which slot
            // wins is only known once every sequence number is in hand.
            let offset = self.offset(slot)?;
            flash.read(offset, &mut buffer.0)?;
            // A record whose header passed a moment ago and whose entries do
            // not decode is reported rather than partly loaded. `first_error`
            // is overwritten deliberately: this is the table that was going to
            // be used, so it is the error worth naming.
            match ShadeRecord::for_each(&buffer.0, shade) {
                Ok(read) => match ShadeRecord::for_each_link(&buffer.0, link) {
                    Ok(_) => {
                        header = Some(ShadeRecordHeader {
                            slot,
                            seq: read.seq,
                            announced: read.announced,
                            count: read.count,
                        })
                    }
                    Err(error) => survey.first_error = Some(error),
                },
                Err(error) => survey.first_error = Some(error),
            }
        }
        Ok((survey, header))
    }

    /// Append `record` to the ring and prove the bytes landed.
    ///
    /// The sequence number is **this store's**, not the caller's: it comes from
    /// the ring's own `next_write`, so a caller cannot hand back a stale one and
    /// have a later record rank as older than the one it replaces. Whatever
    /// `record.seq` held is overwritten.
    pub fn store(
        &mut self,
        flash: &mut FlashStorage<'_>,
        record: &ShadeRecord,
    ) -> Result<u32, ShadeStoreError> {
        let mut buffer = Slot([0u8; SHADE_RECORD_LEN]);
        let scan = self.scan(flash, &mut buffer)?;

        // A write may not proceed on a ring that holds readable records and
        // cannot name a newest one — `crate::config::ConfigStore::store` carries
        // the full argument, and it is the same one here: `next_write(None)`
        // aims at slot 0 with sequence 0, erasing a sector that may hold the
        // only readable table and then writing a record every later scan will
        // rank as ancient.
        if scan.newest.is_none() && scan.valid > 0 {
            return Err(ShadeStoreError::Unstable { valid: scan.valid });
        }

        let newest = scan.newest.map(|(slot, seq)| SlotWrite { slot, seq });
        let aim = self.ring.layout().next_write(newest);

        // Where the ring points is where the write goes *unless* that slot
        // still holds something. `write_slot` steps over the wreckage a torn
        // write leaves; writing into it would only clear more bits.
        let slot_count = self.ring.layout().slot_count();
        let slot = self
            .ring
            .write_slot(aim.slot, &scan.free[..slot_count])
            .ok_or(ShadeStoreError::SlotOutOfRange { slot: aim.slot })?;

        let stamped = ShadeRecord {
            seq: aim.seq,
            announced: record.announced,
            shades: record.shades.clone(),
            links: record.links.clone(),
        };
        self.append(flash, slot, &stamped)?;
        Ok(aim.seq)
    }

    /// Append `record` at `slot`, erasing that slot's sector first if it starts
    /// one, then prove the bytes landed.
    fn append(
        &mut self,
        flash: &mut FlashStorage<'_>,
        slot: usize,
        record: &ShadeRecord,
    ) -> Result<(), ShadeStoreError> {
        let offset = self.offset(slot)?;

        if let Some(sector) = self.ring.erase_before(slot) {
            let from = self.base + sector as u32;
            flash.erase(from, from + SECTOR as u32)?;
        }

        // Through `Slot` rather than straight from `encode`: a bare `[u8; N]`
        // is byte-aligned, and `esp-storage` answers an unaligned buffer by
        // copying it through a 4 KB sector buffer on this stack.
        let bytes = Slot(record.encode());
        flash.write(offset, &bytes.0)?;

        // Durability, verified rather than assumed — and verified against the
        // *bytes*, which the format makes possible by guaranteeing that equal
        // records encode identically. Decoding the read-back instead would
        // compare two records and miss a padding byte the flash did not take;
        // it would also need a second 2 KB buffer where this needs 256.
        let mut window = Window([0u8; VERIFY_WINDOW]);
        for at in (0..SHADE_RECORD_LEN).step_by(VERIFY_WINDOW) {
            flash.read(offset + at as u32, &mut window.0)?;
            if window.0 != bytes.0[at..at + VERIFY_WINDOW] {
                return Err(ShadeStoreError::NotDurable);
            }
        }
        Ok(())
    }

    /// Read every slot's header: which is newest, which are erased, and a tally.
    ///
    /// Headers only. Which slot wins is a question about four sequence numbers,
    /// and decoding four tables to answer it would cost 2,320 bytes of stack
    /// per table.
    fn scan(
        &mut self,
        flash: &mut FlashStorage<'_>,
        buffer: &mut Slot,
    ) -> Result<Scan, ShadeStoreError> {
        let slot_count = self.ring.layout().slot_count();
        let mut sequences = [None; MAX_SLOTS];
        let mut free = [false; MAX_SLOTS];
        let (mut valid, mut blank, mut damaged) = (0, 0, 0);
        let mut first_error = None;

        // Iterated over the array rather than over a range of indices, which is
        // the same walk and is what keeps `slot` from being an index into
        // anything that could be shorter than it: `take` bounds it to the ring,
        // and the array bounds it to what this build can scan.
        for (slot, sequence) in sequences.iter_mut().enumerate().take(slot_count) {
            let offset = self
                .ring
                .layout()
                .offset(slot)
                .map(|offset| self.base + offset as u32)
                .ok_or(ShadeStoreError::SlotOutOfRange { slot })?;
            flash.read(offset, &mut buffer.0)?;
            match ShadeRecord::header(&buffer.0) {
                Ok(header) => {
                    *sequence = Some(header.seq);
                    valid += 1;
                }
                // Only an erased slot has never been written, and only an
                // erased slot can take a write without an erase first. Anything
                // else is a torn write, damage, or a table this firmware will
                // not run — and emphatically not free.
                Err(ShadeRecordError::Blank) => {
                    free[slot] = true;
                    blank += 1;
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
    fn offset(&self, slot: usize) -> Result<u32, ShadeStoreError> {
        self.ring
            .layout()
            .offset(slot)
            .map(|offset| self.base + offset as u32)
            .ok_or(ShadeStoreError::SlotOutOfRange { slot })
    }
}

/// What the table that was actually loaded says about itself.
///
/// Separate from [`ShadeSurvey`], which is about the *region*: this is about
/// the one record in it that the controller is running from, and its
/// `announced` set is what a later removal needs in order to name the entities
/// it has to clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadeRecordHeader {
    /// Which slot it came from.
    #[allow(dead_code, reason = "printed at boot; a derived Debug is not a read")]
    pub slot: usize,
    /// Its sequence number.
    #[allow(dead_code, reason = "printed at boot; a derived Debug is not a read")]
    pub seq: u32,
    /// Which shades this device has already published entities for.
    pub announced: somfy_config::Announced,
    /// How many shades it holds.
    #[allow(dead_code, reason = "printed at boot; a derived Debug is not a read")]
    pub count: usize,
}

/// What one pass over the ring found.
struct Scan {
    /// The newest valid record's slot and sequence number.
    newest: Option<(usize, u32)>,
    /// `free[i]` — slot `i` is erased, so a record can be written into it
    /// without erasing anything first. Indices past the ring stay false.
    free: [bool; MAX_SLOTS],
    /// Slots holding a record that passed its checksum.
    valid: usize,
    /// Slots never written since the last erase.
    blank: usize,
    /// Slots holding something that is neither.
    damaged: usize,
    /// Why the first non-blank slot that failed did so.
    first_error: Option<ShadeRecordError>,
}
