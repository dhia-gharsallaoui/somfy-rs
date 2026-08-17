//! The persisted shade table, on real flash.
//!
//! Same shape as [`crate::config`] and [`crate::store`], and for the same
//! reason: everything worth testing — the record's encoding, its validity
//! check, the rules that decide whether a decoded shade is one the registry
//! will accept, the slot arithmetic — lives on the host side in `somfy-config`
//! and `somfy-store`. What is left here is flash I/O.
//!
//! ## This region is read-only to the firmware
//!
//! There is no `store` here, and that is deliberate rather than unfinished.
//! The shade table is written by the host-side `provision_shades` example and
//! flashed with `espflash write-bin`, exactly as the Wi-Fi and MQTT record is,
//! so the controller has no path that can renumber a shade id, drop a shade, or
//! half-write the table. A firmware that cannot write cannot corrupt.
//!
//! What it costs is that changing a shade needs a cable. That is the same cost
//! the credentials already carry, and Plan 6's configuration store is where it
//! is paid off.
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
use somfy_config::{ShadeRecord, ShadeRecordError, StoredShade, SHADE_RECORD_LEN};
use somfy_store::{newest_slot, SectorRing};

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
    /// The flash refused a read.
    Flash(FlashStorageError),
    /// A slot index outside the ring. Unreachable — every index comes from the
    /// ring itself — but an error rather than a panic, because a panic here
    /// would take the radio off the air over a shade table.
    SlotOutOfRange { slot: usize },
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

/// The flash-backed shade table.
pub struct ShadeStore<'d> {
    flash: FlashStorage<'d>,
    /// Absolute flash offset of the partition.
    base: u32,
    ring: SectorRing,
}

impl<'d> ShadeStore<'d> {
    /// Find the shade partition and take ownership of the flash.
    ///
    /// **Call this from `main`, not from a task**, for the same reason the
    /// other two say so: the partition table costs about 1 KB of stack here
    /// plus `esp-storage`'s 4 KB sector buffer on the unaligned read path.
    pub fn mount(mut flash: FlashStorage<'d>) -> Result<Self, ShadeStoreError> {
        let capacity = flash.capacity() as u64;
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(&mut flash, &mut buffer)
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
        // A `SectorRing` rather than a bare `SlotLayout` even though nothing
        // here writes: the geometry rules it enforces are what a writer would
        // need, and a region shaped so that no writer could ever use it safely
        // is worth refusing at the point it is mounted rather than at the point
        // one is added.
        let ring = SectorRing::new(len as usize, SHADE_RECORD_LEN, SECTOR).ok_or_else(geometry)?;
        if ring.layout().slot_count() > MAX_SLOTS {
            return Err(geometry());
        }

        Ok(Self { flash, base, ring })
    }

    /// Where the ring lives and how it is carved up: offset, slots, slot bytes.
    pub fn geometry(&self) -> (u32, usize, usize) {
        (
            self.base,
            self.ring.layout().slot_count(),
            self.ring.layout().slot_len(),
        )
    }

    /// Hand every shade in the newest readable table to `visit`, and report
    /// what the scan saw getting there.
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
    /// the shades are decoded one at a time straight into whatever `visit`
    /// does with them, which is 72 bytes at once. `somfy_config::ShadeRecord`'s
    /// all-or-nothing rule still holds: a table with one bad entry visits
    /// nothing.
    pub fn load_with(
        &mut self,
        visit: impl FnMut(usize, StoredShade),
    ) -> Result<ShadeSurvey, ShadeStoreError> {
        let slot_count = self.ring.layout().slot_count();
        let mut sequences = [None; MAX_SLOTS];
        let (mut valid, mut blank, mut damaged) = (0, 0, 0);
        let mut first_error = None;
        let mut buffer = Slot([0u8; SHADE_RECORD_LEN]);

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
            self.flash.read(offset, &mut buffer.0)?;
            // The header only: which slot is newest is a question about four
            // sequence numbers, and decoding four tables to answer it would
            // cost more stack than the whole read.
            match ShadeRecord::header(&buffer.0) {
                Ok(header) => {
                    *sequence = Some(header.seq);
                    valid += 1;
                }
                // Only an erased slot has never been written. Anything else is
                // a torn write, damage, or a table this firmware will not run.
                Err(ShadeRecordError::Blank) => blank += 1,
                Err(error) => {
                    damaged += 1;
                    first_error = first_error.or(Some(error));
                }
            }
        }

        // `newest_slot` owns the wrap-around comparison, so the ordering rule
        // stays in one host-tested place rather than being re-derived here.
        //
        // The winner is read a second time rather than kept from the first
        // pass, for the same reason the other two stores do it: which slot wins
        // is only known once every sequence number is in hand.
        let winner = newest_slot(&sequences[..slot_count]);
        if let Some(slot) = winner {
            let offset = self.offset(slot)?;
            self.flash.read(offset, &mut buffer.0)?;
            // A record whose header passed a moment ago and whose entries do
            // not decode is reported rather than partly loaded. `first_error`
            // is overwritten deliberately: this is the table that was going to
            // be used, so it is the error worth naming.
            if let Err(error) = ShadeRecord::for_each(&buffer.0, visit) {
                first_error = Some(error);
            }
        }
        Ok(ShadeSurvey {
            slots: valid + blank + damaged,
            valid,
            blank,
            damaged,
            newest_seq: winner.and_then(|slot| sequences[slot]),
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
