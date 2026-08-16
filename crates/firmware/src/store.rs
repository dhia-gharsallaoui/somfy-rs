//! The rolling-code store, on real flash.
//!
//! Everything worth testing — the record's encoding and its validity check, the
//! slot ring, the erase-unit arithmetic — lives in `somfy-store` and is covered
//! by host tests, because nothing in this crate can be compiled for the host at
//! all. What is left here is flash I/O: find the partition, read slots out of
//! it, erase a sector, write a record, read it back.
//!
//! ## The one guarantee
//!
//! [`RollingCodeStore::commit`] must not return `Ok` until the value would
//! survive a power loss, because `somfy_store::transmit` puts a frame on the
//! air the instant it does. Two things back that up:
//!
//! - `esp-storage`'s write is the ROM SPI-flash routine, which programs the
//!   page and polls the device until the program cycle has finished. There is
//!   no write-behind cache to flush — the bytes are in the array when it
//!   returns.
//! - That is a claim about somebody else's code, so [`FlashStore::commit`]
//!   **reads the record back and compares it** before returning. A write that
//!   silently did not land becomes a returned error, and no frame is sent.
//!
//! ## What a torn write leaves behind
//!
//! Power lost part-way through a commit leaves the target slot holding some
//! programmed words and some erased ones. `Record::decode` rejects it — the
//! checksum covers every byte — so on the next boot that slot contributes no
//! sequence number, `newest_slot` skips it, and the **previous** record is
//! still the newest. The controller resumes from the last code it durably
//! stored, which is the code it had not yet transmitted: nothing is lost, and
//! the motor is never sent a code behind the one it has already accepted.
//!
//! The wreckage itself stays in that slot until the ring laps round and erases
//! its sector. That matters, because flash programming only clears bits: the
//! next commit must **step over** it rather than write into it, or it would
//! produce another unreadable record and the ring would never advance again.
//! `SectorRing::write_slot` does the stepping, and its docs carry the argument.
//!
//! An interrupted *erase* is no different. A partly-erased sector reads back as
//! blank slots, or as slots whose checksum now fails; it cannot read as a
//! plausible older record, because erasing only sets bits back to one and a
//! record cannot pass its checksum that way. And `SectorRing` guarantees the
//! sector being erased never holds the newest record, so what is destroyed was
//! already superseded.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions;
use esp_storage::{FlashStorage, FlashStorageError};
use somfy_rts::RollingCode;
use somfy_store::{
    newest_slot, CodeTable, Record, RecordError, RollingCodeStore, SectorRing, SlotWrite,
    TableError, RECORD_LEN,
};

/// Partition holding the rolling-code ring. Defined by `partitions.csv` in this
/// crate; `espflash.toml` points espflash at that file, so an ordinary
/// `espflash flash` writes it.
///
/// Looked up by label rather than by a compiled-in offset on purpose. A
/// hardcoded address is a compiled-in default of exactly the kind
/// `docs/specs/2026-08-15-config-integrity-requirements.md` R1 warns about: it
/// keeps working right up until the app partition grows past it, and then
/// quietly writes rolling codes over program text.
pub const PARTITION_LABEL: &str = "rollcode";

/// Flash erase unit, and therefore the alignment the partition must sit on.
const SECTOR: usize = <FlashStorage as NorFlash>::ERASE_SIZE;

/// Largest ring this build can scan.
///
/// The scan holds one sequence number per slot in a fixed array — there is no
/// allocator — so a larger partition could only be scanned in part, and a
/// partly-scanned ring would sometimes name an *older* record as newest.
/// [`FlashStore::mount`] rejects anything bigger rather than let the two
/// disagree quietly. 64 slots is 16 KB, twice what `partitions.csv` reserves.
const MAX_SLOTS: usize = 64;

/// Bytes of partition table read at mount. Entries are 32 bytes, so this covers
/// 32 partitions — far more than any layout here — while staying well under
/// `partitions::PARTITION_TABLE_MAX_LEN`.
const PARTITION_TABLE_BYTES: usize = 1024;

/// Slots read per flash transaction while scanning. Four at a time keeps the
/// scan to a handful of transactions without putting a whole sector on the
/// stack.
const SCAN_SLOTS: usize = 4;

// A record must be a whole number of flash words or `NorFlash::write` rejects
// the offset outright, and it must tile the erase sector or one erase would
// take half of two records. `somfy-store` chose 256 for exactly these reasons,
// but to do so it had to write the flash figures down itself; these tie its
// constant to what the hardware crate actually reports. Same spirit as
// `rmt_tx`'s `MAX_TICKS` guard: a divergence would neither fail to build nor
// fail to link, it would corrupt a counter at the moment the ring first wraps,
// months after the edit that caused it.
const _: () = assert!(
    RECORD_LEN.is_multiple_of(<FlashStorage as NorFlash>::WRITE_SIZE),
    "a record must be a whole number of flash write words"
);
const _: () = assert!(
    SECTOR.is_multiple_of(RECORD_LEN),
    "records must tile the flash erase sector exactly"
);
const _: () = assert!(
    RECORD_LEN.is_multiple_of(<FlashStorage as ReadNorFlash>::READ_SIZE),
    "a record must be a whole number of flash read units"
);

/// Why the store could not do what was asked.
///
/// Every variant is a refusal, never a fallback. There is deliberately no
/// "region missing, starting fresh" path: a store that invents a counter when
/// it cannot find its flash would replay codes the motor has already seen and
/// cost the user a re-pairing procedure at the shade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// The partition table could not be read or parsed.
    PartitionTable(partitions::Error),
    /// No partition labelled [`PARTITION_LABEL`]. The device was flashed
    /// without this crate's `partitions.csv`.
    PartitionMissing,
    /// The partition exists but is the wrong shape for the ring: misaligned,
    /// not a whole number of sectors, fewer than two sectors, or more slots
    /// than [`MAX_SLOTS`].
    PartitionGeometry { offset: u32, len: u32 },
    /// The flash refused a read, write or erase.
    Flash(FlashStorageError),
    /// The record read back after a write is not the record written. The bytes
    /// did not reach the array, whatever the write reported.
    NotDurable,
    /// A slot that decoded during the scan would not decode when read again.
    /// Two reads of the same cells disagreed, which is failing flash.
    Unstable { slot: usize },
    /// The newest record already names `somfy_store::MAX_CODES` addresses and
    /// this commit is for another one.
    TableFull,
    /// Not a 24-bit RTS remote address.
    Address(u32),
    /// A slot index outside the ring. Unreachable — every index here comes
    /// from the ring itself — but an error rather than a panic, because a
    /// panic would take the whole controller off the air.
    SlotOutOfRange { slot: usize },
}

impl From<FlashStorageError> for StoreError {
    fn from(error: FlashStorageError) -> Self {
        Self::Flash(error)
    }
}

impl From<TableError> for StoreError {
    fn from(error: TableError) -> Self {
        match error {
            TableError::Full => Self::TableFull,
            TableError::Address(address) => Self::Address(address),
        }
    }
}

/// What a scan of the ring found. A diagnostic, not part of the store's
/// contract.
///
/// Worth printing at boot: it is the difference between "this device has never
/// stored a code" and "this device's codes are gone", which
/// `docs/specs/2026-08-15-config-integrity-requirements.md` R1 requires be
/// distinguishable — and which no amount of "the store initialised OK" can
/// tell you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Survey {
    /// Slots in the ring.
    pub slots: usize,
    /// Slots holding a record that passed its checksum.
    pub valid: usize,
    /// Slots never written since the last erase.
    pub blank: usize,
    /// Slots holding something that is neither blank nor a valid record — a
    /// torn write, or damage.
    pub damaged: usize,
    /// Sequence number of the newest valid record, if there is one.
    pub newest_seq: Option<u32>,
    /// Addresses the newest valid record carries.
    pub addresses: usize,
}

/// One slot's bytes, aligned so `esp-storage` reads and writes them directly.
///
/// Without the alignment every transaction detours through a 4 KB temporary
/// buffer that `esp-storage` places on the *caller's* stack — a hidden cost
/// that would land on whichever Embassy task ends up owning the store.
#[repr(C, align(4))]
struct Slot([u8; RECORD_LEN]);

/// [`SCAN_SLOTS`] slots' bytes, aligned for the same reason.
#[repr(C, align(4))]
struct ScanBuffer([u8; RECORD_LEN * SCAN_SLOTS]);

/// What one pass over the ring found.
struct Scan {
    /// The newest valid record and the slot holding it.
    newest: Option<(usize, Record)>,
    /// `free[i]` — slot `i` is erased, so a record can be written into it
    /// without erasing anything first. Indices past the ring stay false.
    free: [bool; MAX_SLOTS],
}

/// The flash-backed [`RollingCodeStore`].
pub struct FlashStore<'d> {
    flash: FlashStorage<'d>,
    /// Absolute flash offset of the partition.
    base: u32,
    ring: SectorRing,
}

impl<'d> FlashStore<'d> {
    /// Find the rolling-code partition and take ownership of the flash.
    ///
    /// Fails rather than falling back if the partition is absent or the wrong
    /// shape. That is the point: a device flashed without this crate's
    /// partition table has nowhere durable to keep a rolling code, and it is
    /// far better for it to say so at boot than to transmit happily and lose
    /// every code at the next reset.
    ///
    /// **Call this from `main`, not from a task.** It wants roughly 5 KB of
    /// stack — 1 KB for the partition table here, plus the 4 KB sector buffer
    /// `esp-storage`'s unaligned `ReadStorage::read` path puts on the caller's
    /// stack. Every later operation is far cheaper (see [`ScanBuffer`]), so the
    /// store can be handed to a modestly-sized task afterwards; it is only
    /// mounting that is expensive.
    pub fn mount(mut flash: FlashStorage<'d>) -> Result<Self, StoreError> {
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(&mut flash, &mut buffer)
                .map_err(StoreError::PartitionTable)?;
            let entry = table
                .iter()
                .find(|entry| entry.label_as_str() == PARTITION_LABEL)
                .ok_or(StoreError::PartitionMissing)?;
            (entry.offset(), entry.len())
        };

        let geometry = || StoreError::PartitionGeometry { offset: base, len };
        // Erases address whole sectors of the *flash*, not of the partition, so
        // a partition that does not start on a sector boundary would have every
        // erase spill into its neighbour.
        if !(base as usize).is_multiple_of(SECTOR) {
            return Err(geometry());
        }
        let ring = SectorRing::new(len as usize, RECORD_LEN, SECTOR).ok_or_else(geometry)?;
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

    /// Walk every slot and report what is there.
    pub fn survey(&mut self) -> Result<Survey, StoreError> {
        let mut survey = Survey {
            slots: self.ring.layout().slot_count(),
            ..Survey::default()
        };
        let scan = self.scan(|decoded| match decoded {
            Ok(_) => survey.valid += 1,
            Err(RecordError::Blank) => survey.blank += 1,
            Err(_) => survey.damaged += 1,
        })?;
        if let Some((_, record)) = scan.newest {
            survey.newest_seq = Some(record.seq);
            survey.addresses = record.table.len();
        }
        Ok(survey)
    }

    /// Read every slot, show each decode attempt to `observe`, and report both
    /// the newest valid record and which slots are erased.
    ///
    /// The winner is read a second time rather than kept from the first pass:
    /// which slot wins is only known once every sequence number is in hand, and
    /// holding all 64 decoded records to avoid one 256-byte read would cost
    /// several KB of stack in a task that has better uses for it.
    fn scan(
        &mut self,
        mut observe: impl FnMut(&Result<Record, RecordError>),
    ) -> Result<Scan, StoreError> {
        let slot_count = self.ring.layout().slot_count();
        let mut sequences = [None; MAX_SLOTS];
        let mut free = [false; MAX_SLOTS];

        let mut buffer = ScanBuffer([0u8; RECORD_LEN * SCAN_SLOTS]);
        let mut slot = 0;
        while slot < slot_count {
            let batch = SCAN_SLOTS.min(slot_count - slot);
            let offset = self.offset(slot)?;
            let bytes = &mut buffer.0[..batch * RECORD_LEN];
            self.flash.read(offset, bytes)?;

            for index in 0..batch {
                let mut record = [0u8; RECORD_LEN];
                record.copy_from_slice(&bytes[index * RECORD_LEN..][..RECORD_LEN]);
                let decoded = Record::decode(&record);
                observe(&decoded);
                match decoded {
                    Ok(record) => sequences[slot + index] = Some(record.seq),
                    // Only an erased slot can take a write without an erase
                    // first. A damaged one is emphatically not free: writing
                    // over it would AND into the wreckage.
                    Err(RecordError::Blank) => free[slot + index] = true,
                    Err(_) => {}
                }
            }
            slot += batch;
        }

        // `newest_slot` owns the wrap-around comparison, so the ordering rule
        // lives in one host-tested place rather than being re-derived here.
        let newest = match newest_slot(&sequences[..slot_count]) {
            None => None,
            Some(slot) => match self.read_slot(slot)? {
                Ok(record) => Some((slot, record)),
                Err(_) => return Err(StoreError::Unstable { slot }),
            },
        };
        Ok(Scan { newest, free })
    }

    /// Read one slot and try to decode it.
    fn read_slot(&mut self, slot: usize) -> Result<Result<Record, RecordError>, StoreError> {
        let offset = self.offset(slot)?;
        let mut buffer = Slot([0u8; RECORD_LEN]);
        self.flash.read(offset, &mut buffer.0)?;
        Ok(Record::decode(&buffer.0))
    }

    /// Append `record` at `slot`, erasing that slot's sector first if it starts
    /// one, then prove the bytes landed.
    fn append(&mut self, slot: usize, record: &Record) -> Result<(), StoreError> {
        let offset = self.offset(slot)?;

        if let Some(sector) = self.ring.erase_before(slot) {
            let from = self.base + sector as u32;
            self.flash.erase(from, from + SECTOR as u32)?;
        }

        // Through `Slot` rather than straight from `encode`: a bare `[u8; N]`
        // is byte-aligned, and `esp-storage` answers an unaligned buffer by
        // copying it through a 4 KB sector buffer on this stack. Four bytes of
        // alignment here is the difference between a 256-byte write and a 4 KB
        // stack spike on every single commit.
        let bytes = Slot(record.encode());
        self.flash.write(offset, &bytes.0)?;

        // Durability, verified rather than assumed. Until these bytes read
        // back as what was written, nothing may go on the air.
        match self.read_slot(slot)? {
            Ok(written) if written == *record => Ok(()),
            _ => Err(StoreError::NotDurable),
        }
    }

    /// Absolute flash offset of `slot`.
    fn offset(&self, slot: usize) -> Result<u32, StoreError> {
        self.ring
            .layout()
            .offset(slot)
            .map(|offset| self.base + offset as u32)
            .ok_or(StoreError::SlotOutOfRange { slot })
    }
}

impl RollingCodeStore for FlashStore<'_> {
    type Error = StoreError;

    /// `Ok(None)` means the ring holds no record for `address` — either it has
    /// never been written, or the newest record does not name this address.
    /// Both are facts to report. A read failure is `Err`, and neither is ever
    /// answered with a plausible-looking starting value.
    fn load(&mut self, address: u32) -> Result<Option<RollingCode>, Self::Error> {
        Ok(self
            .scan(|_| {})?
            .newest
            .and_then(|(_, record)| record.table.get(address)))
    }

    /// Append a record carrying every address's code, with `address` updated.
    ///
    /// The whole table is rewritten rather than just the one entry, because
    /// that is what lets the ring erase an old sector without carrying anything
    /// forward. `somfy_store::Record`'s module docs give the full argument.
    fn commit(&mut self, address: u32, code: RollingCode) -> Result<(), Self::Error> {
        let scan = self.scan(|_| {})?;
        let mut record = match scan.newest {
            Some((_, record)) => record,
            None => Record {
                seq: 0,
                table: CodeTable::new(),
            },
        };
        record.table.set(address, code)?;

        let aim = self
            .ring
            .layout()
            .next_write(scan.newest.map(|(slot, record)| SlotWrite {
                slot,
                seq: record.seq,
            }));
        record.seq = aim.seq;

        // Where the ring points is where the write goes *unless* that slot
        // still holds something — the half-written record a previous commit
        // left when it lost power, most likely. `write_slot` steps over it;
        // see its docs for why writing into it anyway would wedge the store.
        let slot_count = self.ring.layout().slot_count();
        let slot = self
            .ring
            .write_slot(aim.slot, &scan.free[..slot_count])
            .ok_or(StoreError::SlotOutOfRange { slot: aim.slot })?;

        self.append(slot, &record)
    }
}
