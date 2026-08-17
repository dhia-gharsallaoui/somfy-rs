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
//!
//! ## What this store does NOT protect against
//!
//! The guarantee above is about **torn writes** — a commit interrupted
//! part-way. It is not a general damage guarantee, and the difference matters.
//!
//! If a *completed* record is destroyed some other way — a failing sector, bit
//! rot, a stray write — the scan sees a damaged slot where a valid one used to
//! be and falls back to the newest record it can still read, which is an
//! **older** one. `load` then answers with a code the motor has already
//! accepted, and nothing here can tell that apart from a run of torn writes:
//! both leave damaged slots ahead of a valid record, and the store has no
//! second copy to check against. Only the `damaged` count in [`Survey`] hints
//! at it, which is why a boot that reports damage on a device that was not
//! power-cut deserves a look rather than a shrug.
//!
//! The fix is redundancy — a second copy of the newest record in the other
//! sector — which is a change to the record layout and belongs with the Plan 6
//! rewrite rather than bolted on here. What is *not* acceptable, and is
//! handled, is the store quietly deciding a damaged region is a blank one; see
//! [`Scan::newest_or_refuse`].
//!
//! ## Two hazards this puts on the rest of the firmware
//!
//! Both come from `esp-storage` and neither is visible from the signatures, so
//! they are recorded here for whoever wires the tasks up.
//!
//! - **Every flash operation runs with interrupts disabled on this core.**
//!   `esp-storage`'s `critical-section` feature is on by default and holds an
//!   interrupt-disabling lock across the whole ROM call. Reads are short, but a
//!   4 KB sector erase — one commit in [`somfy_store::SectorRing::slots_per_sector`],
//!   so one button press in 16 — is tens of milliseconds, and the datasheet
//!   worst case is a few hundred. RMT reception during that window is simply
//!   lost. Committing off the radio task does not help; the core is the core.
//!
//! - **A commit fails outright while the second core is running.**
//!   `FlashStorage`'s default multi-core strategy is to refuse the write rather
//!   than risk it, which is the right default — the alternative parks the other
//!   core mid-execution, and parking it mid-frame is worse than a failed
//!   commit, since a failed commit at least stops the transmission cleanly.
//!   But it means that the day something starts the app core, **every commit
//!   returns `Flash(OtherCoreRunning)` and nothing can transmit at all**. That
//!   is a deliberate, loud failure and not a silent one, but it is a decision
//!   waiting to be made rather than one already made: `multicore_auto_park()`
//!   is the opt-in, and it belongs to whoever owns the tasks.

// `ReadNorFlash` rather than `ReadStorage`, even for `capacity`: both traits
// carry a `read`, and importing both makes every call site ambiguous.
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{self, PartitionType};
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
    /// A slot that decoded during the scan read back differently the second
    /// time. Two reads of the same cells disagreed, which is failing flash.
    Unstable { slot: usize },
    /// The ring holds no readable record, but it is not blank either — so
    /// whether it once held rolling codes is unknowable. Reported rather than
    /// treated as a first boot; see [`Scan::newest_or_refuse`].
    Unreadable { damaged: usize, slots: usize },
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
    /// Slots holding a record that passed its checksum.
    valid: usize,
    /// Slots never written since the last erase.
    blank: usize,
    /// Slots holding something that is neither.
    damaged: usize,
}

impl Scan {
    /// The newest record, refusing when the ring cannot say whether there was
    /// one.
    ///
    /// `Ok(None)` is reserved for a ring that is genuinely **blank** — a first
    /// boot. No valid record *plus* something damaged is a different fact: the
    /// region held bytes that are not a record, and there is no way to tell
    /// whether they were once somebody's rolling code. Treating that as a first
    /// boot is how a store starts counting from 1 on a controller that had
    /// three shades paired to it, and every one of them then needs a physical
    /// re-pairing.
    ///
    /// It is also the path a format bump would take: bump `Record`'s version
    /// and every existing record decodes as damaged, so without this the store
    /// would erase a perfectly good region and reseed rather than reporting
    /// that it found records it could not read.
    ///
    /// Refusing is recoverable — a person re-seeds, or erases the region with
    /// `espflash erase-parts rollcode` — and accepting is not. That asymmetry
    /// is the whole argument, and it is why even the narrow case that *is*
    /// explainable (a torn first write leaves exactly one damaged slot in an
    /// otherwise blank region) is refused too rather than special-cased.
    fn newest_or_refuse(&self) -> Result<Option<(usize, Record)>, StoreError> {
        match (self.newest, self.damaged) {
            (Some(newest), _) => Ok(Some(newest)),
            (None, 0) => Ok(None),
            (None, damaged) => Err(StoreError::Unreadable {
                damaged,
                slots: self.valid + self.blank + damaged,
            }),
        }
    }
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
        let capacity = flash.capacity() as u64;
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(&mut flash, &mut buffer)
                .map_err(StoreError::PartitionTable)?;
            let entry = table
                .iter()
                .find(|entry| {
                    // Label *and* type: a label match alone would happily mount
                    // an app partition somebody named `rollcode`, and the first
                    // erase would take the firmware with it.
                    entry.label_as_str() == PARTITION_LABEL
                        && matches!(entry.partition_type(), PartitionType::Data(_))
                })
                .ok_or(StoreError::PartitionMissing)?;
            (entry.offset(), entry.len())
        };

        let geometry = || StoreError::PartitionGeometry { offset: base, len };
        // The table is data read off the device, so it is checked rather than
        // trusted — in 64-bit arithmetic, because the point is to catch a
        // partition that runs off the end of a smaller flash than the one this
        // table was written for. Without this, `partitions.csv`'s 0x200000 on a
        // 2 MB board mounts happily and then fails every single operation with
        // `OutOfBounds`, at the far end of the codebase from the cause.
        if base as u64 + len as u64 > capacity {
            return Err(geometry());
        }
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

    /// Lend the flash peripheral to another region of the same chip.
    ///
    /// # Why this exists at all
    ///
    /// There is one flash peripheral, and this store owns it for the life of
    /// the program — it must, because a rolling code has to be committed before
    /// every transmission and the store that does that cannot be handed around.
    /// The shade table lives on the same chip and is now written at runtime, so
    /// something has to reconcile the two.
    ///
    /// The alternatives were both worse. A second `FlashStorage` obtained by
    /// stealing the peripheral is an `unsafe` assertion that two writers never
    /// overlap, with nothing checking it. Splitting this store into "geometry"
    /// and "flash owner" would restructure the one module in this firmware that
    /// has been proved on hardware across reboots.
    ///
    /// A borrow makes the guarantee structural instead: `&mut self` here is the
    /// same `&mut self` a commit needs, so a shade write and a rolling-code
    /// commit cannot be in flight together, and the borrow ends when the
    /// closure returns.
    ///
    /// **This does not make the flash safe to write anywhere.** The callee can
    /// address any offset on the chip, including this store's own region and the
    /// app partition. What it buys is exclusion, not confinement; the confining
    /// is each region's own partition lookup, which is why every one of them
    /// resolves its base by label rather than by a compiled-in offset.
    #[allow(
        dead_code,
        reason = "the controller's only caller is the shade store; `config-check` \
                  includes this file by path and has no shade region"
    )]
    pub fn with_flash<T>(&mut self, f: impl FnOnce(&mut FlashStorage<'d>) -> T) -> T {
        f(&mut self.flash)
    }

    /// Walk every slot and report what is there.
    ///
    /// Unlike [`RollingCodeStore::load`] this never refuses: it is the
    /// diagnostic that explains *why* the store is refusing, so it has to be
    /// readable in exactly the states the store will not act on.
    pub fn survey(&mut self) -> Result<Survey, StoreError> {
        let scan = self.scan()?;
        Ok(Survey {
            slots: scan.valid + scan.blank + scan.damaged,
            valid: scan.valid,
            blank: scan.blank,
            damaged: scan.damaged,
            newest_seq: scan.newest.map(|(_, record)| record.seq),
            addresses: scan.newest.map_or(0, |(_, record)| record.table.len()),
        })
    }

    /// Read every slot: the newest valid record, which slots are erased, and a
    /// tally of what each one held.
    ///
    /// The winner is read a second time rather than kept from the first pass:
    /// which slot wins is only known once every sequence number is in hand, and
    /// holding all 64 decoded records to avoid one 256-byte read would cost
    /// several KB of stack in a task that has better uses for it. The second
    /// read is checked against the first, so cells that answer differently
    /// twice are reported rather than silently believed.
    fn scan(&mut self) -> Result<Scan, StoreError> {
        let slot_count = self.ring.layout().slot_count();
        let mut sequences = [None; MAX_SLOTS];
        let mut free = [false; MAX_SLOTS];
        let (mut valid, mut blank, mut damaged) = (0, 0, 0);

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
                match Record::decode(&record) {
                    Ok(record) => {
                        sequences[slot + index] = Some(record.seq);
                        valid += 1;
                    }
                    // Only an erased slot can take a write without an erase
                    // first. A damaged one is emphatically not free: writing
                    // over it would AND into the wreckage.
                    Err(RecordError::Blank) => {
                        free[slot + index] = true;
                        blank += 1;
                    }
                    Err(_) => damaged += 1,
                }
            }
            slot += batch;
        }

        // `newest_slot` owns the wrap-around comparison, so the ordering rule
        // lives in one host-tested place rather than being re-derived here.
        let newest = match newest_slot(&sequences[..slot_count]) {
            None => None,
            Some(slot) => match self.read_slot(slot)? {
                Ok(record) if Some(record.seq) == sequences[slot] => Some((slot, record)),
                _ => return Err(StoreError::Unstable { slot }),
            },
        };
        Ok(Scan {
            newest,
            free,
            valid,
            blank,
            damaged,
        })
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

    /// `Ok(None)` means the ring holds no record for `address` — either it is
    /// blank, or its newest record does not name this address. Both are facts
    /// to report. A read failure is `Err`, so is a region that holds unreadable
    /// bytes and no record, and none of them is ever answered with a
    /// plausible-looking starting value.
    fn load(&mut self, address: u32) -> Result<Option<RollingCode>, Self::Error> {
        Ok(self
            .scan()?
            .newest_or_refuse()?
            .and_then(|(_, record)| record.table.get(address)))
    }

    /// Append a record carrying every address's code, with `address` updated.
    ///
    /// The whole table is rewritten rather than just the one entry, because
    /// that is what lets the ring erase an old sector without carrying anything
    /// forward. `somfy_store::Record`'s module docs give the full argument.
    fn commit(&mut self, address: u32, code: RollingCode) -> Result<(), Self::Error> {
        let scan = self.scan()?;
        // Not `scan.newest`: a ring with nothing readable in it and damage in
        // it is not a blank ring, and starting a fresh seq-0 table there would
        // discard every other address's code without a word. It would also
        // restart the sequence counter underneath `newest_slot`, whose circular
        // comparison then reads any slot that later reads clean as *newer* —
        // rolling the code backwards.
        let newest = scan.newest_or_refuse()?;
        let mut record = match newest {
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
            .next_write(newest.map(|(slot, record)| SlotWrite {
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
