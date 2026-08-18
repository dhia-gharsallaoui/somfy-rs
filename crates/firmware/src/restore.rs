//! Backup and restore: the staging region, the boot-side applier, and the
//! export stream.
//!
//! # A restore is staged and applied at the next boot, and that is not a choice
//!
//! An upload of a backup writes a flash region and restarts. The boot path
//! reads it, validates it, and either writes the shade table, the estate and
//! the rolling codes, or refuses. Three reasons, and the first is arithmetic.
//!
//! **It does not fit anywhere else.** A C++ ESPSomfy-RTS backup is about twelve
//! kilobytes and has to be parsed whole — `somfy_migrate::parse_backup` takes
//! one contiguous `&[u8]`, its `Reader` is a private cursor over that slice with
//! no way to resume, and a short buffer is indistinguishable from a truncated
//! file. The decoded `MigrationData` is another five and a half kilobytes, and
//! `somfy_config::import` builds a `ShadeRecord` and an `EstateRecord` beside
//! it. Against that: the ESP32-S3's Wi-Fi heap clears its own measured
//! announcement peak by about seven kilobytes, the state task's chain has under
//! a kilobyte of headroom before [`crate::heap`]'s compile-time assertion
//! fires, and a connection task's future is DRAM the same heap is subtracted
//! from. **At boot there is room**: the main stack is sixty-six kilobytes,
//! nothing has been spawned, and [`apply`] is `#[inline(never)]` so its frame is
//! given back before the state task's future is materialised on top of it.
//!
//! **The boot path already knows how to do the rest.** It reads these same
//! regions, seeds rolling codes through `somfy_store::seed_if_absent`, and
//! announces the result. Applying a restore to a *running* controller would
//! mean a second code path that tears down a registry the state task owns and a
//! broker session mid-flight, with the announcement ordering restated rather
//! than reused.
//!
//! **There is precedent in this firmware, twice.** A firmware upload answers
//! `202` and restarts. A broker settings change answers `202` and restarts,
//! because the retained topics of the superseded namespaces have to be cleared
//! by the boot path that already does it correctly — `api::routes::restart_for_mqtt`
//! carries that argument and it is the same one.
//!
//! What it costs is that a refusal arrives after a reboot rather than in the
//! response. **The cheap half is not deferred**: an upload that is not a backup
//! at all, or is larger than the region, is refused immediately with a code
//! naming which.
//!
//! # A restore cannot walk a rolling code backwards
//!
//! Nothing here enforces that, and that is the point. Every code goes through
//! `somfy_store::seed_if_absent`, whose commit sits inside the branch where the
//! read said the address had nothing stored — there is no parameter that
//! reaches the other branch, so an overwrite is not something this code
//! declines to do, it is something it cannot express. A backup taken a month
//! ago, restored onto the board it came from, plants nothing: every address
//! already has a code, and the stored one — the one the motors have actually
//! been driven with — wins.
//!
//! The case that *does* plant is a fresh board, which is what a backup is for.
//!
//! # Refusal, not repair
//!
//! Spec R3, and it is why the applier is all-or-nothing. A backup with one bad
//! shade writes **nothing**: `ShadeRecord::for_each` already refuses a record
//! with one bad row for the reason that ids come from position, so importing
//! what parses would renumber everything after the gap and rename half an
//! installation in Home Assistant. [`Fault`] is what the refusal is reported
//! as, and it is stored in flash so the answer survives the restart that
//! produced it.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{self, Error as PartitionError, PartitionType};
use esp_storage::{FlashStorage, FlashStorageError};
use somfy_api::{
    ApiErrorCode, BackupContentsDto, BackupFormatDto, RestoreOutcomeDto, RestoreReportDto,
};
use somfy_backup::{Codes, BACKUP_LEN};

/// Partition holding a staged backup and the report of the last one.
///
/// Looked up by label, like the other four regions and for the same reason a
/// compiled-in offset was rejected there: it keeps working right up until the
/// app partition grows past it.
pub const PARTITION_LABEL: &str = "import";

/// Flash erase unit, and the alignment the partition must sit on.
const SECTOR: usize = <FlashStorage as NorFlash>::ERASE_SIZE;

/// Flash write granularity.
const WORD: usize = <FlashStorage as NorFlash>::WRITE_SIZE;

/// The partition table's own read buffer, as the other stores size it.
const PARTITION_TABLE_BYTES: usize = partitions::PARTITION_TABLE_MAX_LEN;

/// Bytes of a staged file this device will accept.
///
/// **16,384, and the figure is the worst backup a supported controller can
/// produce, rounded up to a whole number of erase sectors.** A C++
/// ESPSomfy-RTS v25 backup with every collection full is: a ~65-byte header,
/// 16 rooms at about 64 bytes, **32 shades at exactly 276** — the size
/// `somfy_migrate`'s own `parses_real_fixed_width_record` test asserts — 16
/// groups at 200, the repeater, settings and transceiver records, and a net
/// record with three 64-byte topic strings. That is about 14.5 KB. This
/// device's own `RTSB` container is 4,420.
///
/// It is also **the size of the buffer [`apply`] reads it into**, on the main
/// stack at boot. That is the other half of the figure and the reason it is not
/// simply "the rest of the region": every byte here is a byte of the deepest
/// chain [`crate::heap::REQUIRED_STACK_BYTES`] has to cover.
///
/// An upload larger than this is refused with
/// [`ApiErrorCode::BackupTooLarge`] before a byte of flash is written.
pub const STAGE_MAX_BYTES: usize = 16 * 1024;

/// Where the staged file starts inside the region.
///
/// One sector in. The first sector holds [`State`], which has to be erased and
/// rewritten on a path that must not disturb the file beside it — and an erase
/// is a whole sector whether it wants to be or not.
const STAGE_OFFSET: usize = SECTOR;

/// The region this module needs, in bytes.
const REGION_BYTES: usize = STAGE_OFFSET + STAGE_MAX_BYTES;

const _: () = assert!(
    STAGE_MAX_BYTES.is_multiple_of(SECTOR),
    "the staged file must be a whole number of erase sectors, or erasing it \
     would take part of the sector after it",
);
const _: () = assert!(
    BACKUP_LEN <= STAGE_MAX_BYTES,
    "this device's own backup must fit the region it is staged in",
);

/// Bytes written to flash at a time while staging.
///
/// The same page the update path uses, and for the same two reasons: it is the
/// SPI NOR page-program unit, so it is the largest write that is one program
/// operation, and it divides [`SECTOR`] exactly so a page never straddles an
/// erase unit. It is also literally the same buffer — a staged backup crosses
/// from the web server to the state task through [`crate::ota`]'s page channel,
/// because an upload is an upload and two channels would be two buffers out of
/// the DRAM the Wi-Fi driver's heap is carved from.
pub const PAGE_BYTES: usize = crate::ota::PAGE_BYTES;

// ---------------------------------------------------------------------------
// The state record
// ---------------------------------------------------------------------------

/// Marks a state record this build wrote. See `somfy_store::Record`'s `RTSC`.
const MAGIC: u32 = u32::from_le_bytes(*b"RTSI");

/// The version this build writes and reads.
const VERSION: u16 = 1;

/// The record, in bytes. One SPI NOR page, so it is one program operation.
const STATE_LEN: usize = 128;

const _: () = assert!(
    STATE_LEN.is_multiple_of(WORD),
    "NorFlash::write takes word-aligned lengths"
);

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_OUTCOME: usize = 6;
const OFF_FORMAT: usize = 7;
const OFF_LENGTH: usize = 8;
const OFF_SHADES: usize = 12;
const OFF_ROOMS: usize = 13;
const OFF_GROUPS: usize = 14;
const OFF_WARNINGS: usize = 15;
const OFF_FAULT: usize = 16;
const OFF_ROW: usize = 17;
const OFF_FLAGS: usize = 18;
const OFF_SSID_LEN: usize = 19;
const OFF_BROKER_LEN: usize = 20;
const OFF_SSID: usize = 24;
const OFF_BROKER: usize = 56;
const OFF_CRC: usize = STATE_LEN - 4;

const _: () = assert!(
    OFF_BROKER + 16 <= OFF_CRC,
    "the broker field overruns the checksum"
);

/// No row is named.
const ROW_NONE: u8 = 0xFF;

/// A passphrase was stored for the SSID this record names.
const FLAG_PSK: u8 = 0b0000_0001;
/// A password was stored for the broker this record names.
const FLAG_BROKER_PASSWORD: u8 = 0b0000_0010;

/// The checksum, table-free.
///
/// `NoTable` rather than `somfy_backup::CRC`'s tabled form: this runs over 124
/// bytes a handful of times in a device's life, where a table would buy
/// microseconds. The container's own checksum runs over four kilobytes and uses
/// the table.
const CRC: crc::Crc<u32, crc::NoTable> = crc::Crc::<u32, crc::NoTable>::new(&crc::CRC_32_ISO_HDLC);

/// Why a staged restore was refused.
///
/// **A vocabulary of this module's own, deliberately, because it goes into
/// flash.** [`ApiErrorCode`] is a wire enum whose variants are ordered for
/// readability and grow as the API does; persisting its discriminant would tie
/// the meaning of a byte in flash to the position of a variant in a file
/// somebody will reorder. These have explicit values and only ever gain new
/// ones at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Fault {
    /// Nothing went wrong.
    None = 0,
    /// The staged bytes are neither an `RTSB` container nor a readable C++
    /// backup.
    NotRecognised = 1,
    /// Truncated, or a checksum that does not match.
    Damaged = 2,
    /// A format version this build does not read.
    UnsupportedVersion = 3,
    /// A flash write that did not read back, or a region that is not there.
    Unwritable = 4,
    /// A shade with no name.
    NameEmpty = 5,
    /// A name over the domain's thirty-two bytes.
    NameTooLong = 6,
    /// A shade kind `somfy_domain::ShadeKind::from_raw` does not model.
    InvalidKind = 7,
    /// A tilt mode `somfy_domain::TiltMode::from_raw` does not model.
    InvalidTiltMode = 8,
    /// A travel time of zero, which is a shade that can never be positioned.
    TravelTimeZero = 9,
    /// An address the RTS protocol cannot carry.
    InvalidAddress = 10,
    /// More shades, rooms or groups than the registry holds.
    RegistryFull = 11,
    /// Two shades at one address.
    AddressInUse = 12,
}

impl Fault {
    /// The value this reads back as, or `None` for a byte no build wrote.
    const fn from_raw(raw: u8) -> Option<Fault> {
        match raw {
            0 => Some(Fault::None),
            1 => Some(Fault::NotRecognised),
            2 => Some(Fault::Damaged),
            3 => Some(Fault::UnsupportedVersion),
            4 => Some(Fault::Unwritable),
            5 => Some(Fault::NameEmpty),
            6 => Some(Fault::NameTooLong),
            7 => Some(Fault::InvalidKind),
            8 => Some(Fault::InvalidTiltMode),
            9 => Some(Fault::TravelTimeZero),
            10 => Some(Fault::InvalidAddress),
            11 => Some(Fault::RegistryFull),
            12 => Some(Fault::AddressInUse),
            _ => None,
        }
    }

    /// What the UI is told.
    ///
    /// **Most of these are the ordinary shade codes**, not a second vocabulary:
    /// a name over thirty-two bytes is [`ApiErrorCode::NameTooLong`] whether it
    /// was typed into a form or read out of a file, and the UI already
    /// translates it. `RestoreReportDto::row` is what says which record it came
    /// from.
    const fn code(self) -> Option<ApiErrorCode> {
        match self {
            Fault::None => None,
            Fault::NotRecognised => Some(ApiErrorCode::BackupNotRecognised),
            Fault::Damaged => Some(ApiErrorCode::BackupDamaged),
            Fault::UnsupportedVersion => Some(ApiErrorCode::BackupUnsupportedVersion),
            Fault::Unwritable => Some(ApiErrorCode::BackupUnwritable),
            Fault::NameEmpty => Some(ApiErrorCode::NameEmpty),
            Fault::NameTooLong => Some(ApiErrorCode::NameTooLong),
            Fault::InvalidKind => Some(ApiErrorCode::InvalidKind),
            Fault::InvalidTiltMode => Some(ApiErrorCode::InvalidTiltMode),
            Fault::TravelTimeZero => Some(ApiErrorCode::TravelTimeZero),
            Fault::InvalidAddress => Some(ApiErrorCode::InvalidAddress),
            Fault::RegistryFull => Some(ApiErrorCode::RegistryFull),
            Fault::AddressInUse => Some(ApiErrorCode::AddressInUse),
        }
    }
}

/// What the region's first sector says about the last upload.
#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
    outcome: RestoreOutcomeDto,
    format: Option<BackupFormatDto>,
    /// Bytes of the staged file, when one is staged.
    length: u32,
    shades: u8,
    rooms: u8,
    groups: u8,
    warnings: u8,
    fault: Fault,
    row: Option<u8>,
    contents: Option<BackupContentsDto>,
}

impl State {
    /// The state of a device that has never been sent a backup.
    const fn nothing() -> State {
        State {
            outcome: RestoreOutcomeDto::None,
            format: None,
            length: 0,
            shades: 0,
            rooms: 0,
            groups: 0,
            warnings: 0,
            fault: Fault::None,
            row: None,
            contents: None,
        }
    }

    fn encode(&self) -> [u8; STATE_LEN] {
        let mut out = [0u8; STATE_LEN];
        out[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
        out[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());
        out[OFF_OUTCOME] = match self.outcome {
            RestoreOutcomeDto::None => 0,
            RestoreOutcomeDto::Staged => 1,
            RestoreOutcomeDto::Applied => 2,
            RestoreOutcomeDto::Refused => 3,
        };
        out[OFF_FORMAT] = match self.format {
            None => 0,
            Some(BackupFormatDto::SomfyRs) => 1,
            Some(BackupFormatDto::EspSomfyRts) => 2,
        };
        out[OFF_LENGTH..OFF_LENGTH + 4].copy_from_slice(&self.length.to_le_bytes());
        out[OFF_SHADES] = self.shades;
        out[OFF_ROOMS] = self.rooms;
        out[OFF_GROUPS] = self.groups;
        out[OFF_WARNINGS] = self.warnings;
        out[OFF_FAULT] = self.fault as u8;
        out[OFF_ROW] = self.row.unwrap_or(ROW_NONE);
        if let Some(contents) = &self.contents {
            let mut flags = 0u8;
            if contents.psk_was_set {
                flags |= FLAG_PSK;
            }
            if contents.broker_password_was_set {
                flags |= FLAG_BROKER_PASSWORD;
            }
            out[OFF_FLAGS] = flags;
            if let Some(ssid) = &contents.ssid {
                let bytes = ssid.as_bytes();
                out[OFF_SSID_LEN] = bytes.len() as u8;
                out[OFF_SSID..OFF_SSID + bytes.len()].copy_from_slice(bytes);
            }
            if let Some(broker) = &contents.broker {
                let bytes = broker.as_bytes();
                out[OFF_BROKER_LEN] = bytes.len() as u8;
                out[OFF_BROKER..OFF_BROKER + bytes.len()].copy_from_slice(bytes);
            }
        }
        let crc = CRC.checksum(&out[..OFF_CRC]);
        out[OFF_CRC..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// Read the record, or [`State::nothing`] for anything that does not check
    /// out.
    ///
    /// **Every failure reads as "nothing staged", which is the safe
    /// direction**: a record that cannot be trusted must not cause a restore,
    /// and the worst it costs is a report an operator has to reproduce. There
    /// is deliberately no error type — a caller could do nothing with one.
    fn decode(bytes: &[u8; STATE_LEN]) -> State {
        if bytes[OFF_MAGIC..OFF_MAGIC + 4] != MAGIC.to_le_bytes() {
            return State::nothing();
        }
        if u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]) != VERSION {
            return State::nothing();
        }
        let stored = u32::from_le_bytes([
            bytes[OFF_CRC],
            bytes[OFF_CRC + 1],
            bytes[OFF_CRC + 2],
            bytes[OFF_CRC + 3],
        ]);
        if stored != CRC.checksum(&bytes[..OFF_CRC]) {
            return State::nothing();
        }
        let Some(outcome) = (match bytes[OFF_OUTCOME] {
            0 => Some(RestoreOutcomeDto::None),
            1 => Some(RestoreOutcomeDto::Staged),
            2 => Some(RestoreOutcomeDto::Applied),
            3 => Some(RestoreOutcomeDto::Refused),
            _ => None,
        }) else {
            return State::nothing();
        };
        let format = match bytes[OFF_FORMAT] {
            1 => Some(BackupFormatDto::SomfyRs),
            2 => Some(BackupFormatDto::EspSomfyRts),
            _ => None,
        };
        let Some(fault) = Fault::from_raw(bytes[OFF_FAULT]) else {
            return State::nothing();
        };
        let length = u32::from_le_bytes([
            bytes[OFF_LENGTH],
            bytes[OFF_LENGTH + 1],
            bytes[OFF_LENGTH + 2],
            bytes[OFF_LENGTH + 3],
        ]);
        if length as usize > STAGE_MAX_BYTES {
            return State::nothing();
        }

        State {
            outcome,
            format,
            length,
            shades: bytes[OFF_SHADES],
            rooms: bytes[OFF_ROOMS],
            groups: bytes[OFF_GROUPS],
            warnings: bytes[OFF_WARNINGS],
            fault,
            row: match bytes[OFF_ROW] {
                ROW_NONE => None,
                row => Some(row),
            },
            contents: read_contents(bytes),
        }
    }

    fn report(&self) -> RestoreReportDto {
        RestoreReportDto {
            outcome: self.outcome,
            format: self.format,
            shades: self.shades,
            rooms: self.rooms,
            groups: self.groups,
            warnings: self.warnings,
            error: self.fault.code().map(somfy_api::ApiErrorDto::from),
            row: self.row,
            contents: self.contents.clone(),
        }
    }
}

/// The non-secret settings a `somfy-rs` backup carried, for the report.
///
/// `None` for a C++ backup, which carries neither — that format keeps network
/// credentials in NVS rather than in the file, so an import can recover *where*
/// to publish and never *as whom*.
fn read_contents(bytes: &[u8; STATE_LEN]) -> Option<BackupContentsDto> {
    let ssid_len = usize::from(bytes[OFF_SSID_LEN]);
    let broker_len = usize::from(bytes[OFF_BROKER_LEN]);
    if ssid_len == 0 && broker_len == 0 && bytes[OFF_FLAGS] == 0 {
        return None;
    }
    let ssid = text(&bytes[OFF_SSID..OFF_SSID + ssid_len.min(32)]);
    let broker = text(&bytes[OFF_BROKER..OFF_BROKER + broker_len.min(16)]);
    Some(BackupContentsDto {
        ssid,
        psk_was_set: bytes[OFF_FLAGS] & FLAG_PSK != 0,
        broker,
        broker_password_was_set: bytes[OFF_FLAGS] & FLAG_BROKER_PASSWORD != 0,
    })
}

/// A record's text field, or `None` for empty or non-UTF-8.
///
/// Non-UTF-8 becomes absent rather than an error for the reason
/// [`State::decode`] gives: this is a *report*, and there is nothing a caller
/// could do about a mangled SSID except be told a different way.
fn text<const N: usize>(bytes: &[u8]) -> Option<heapless::String<N>> {
    if bytes.is_empty() {
        return None;
    }
    let text = core::str::from_utf8(bytes).ok()?;
    let mut out = heapless::String::new();
    out.push_str(text).ok()?;
    Some(out)
}

// ---------------------------------------------------------------------------
// The region
// ---------------------------------------------------------------------------

/// Anything that can go wrong reaching the region.
///
/// **None of them is fatal to the controller.** A board flashed with a table
/// that has no `import` partition — every board provisioned before this feature
/// existed — receives, decodes, tracks and answers Home Assistant exactly as it
/// did. What it cannot do is stage a restore, and the settings screen is told
/// so with [`ApiErrorCode::BackupUnwritable`].
#[allow(dead_code, reason = "each payload exists to be printed by `{:?}`")]
#[derive(Debug)]
pub enum RestoreError {
    /// The partition table could not be read.
    PartitionTable(PartitionError),
    /// There is no `import` region. An older partition table.
    PartitionMissing,
    /// The region is there and is the wrong shape.
    PartitionGeometry {
        /// Where the table says it is.
        offset: u32,
        /// How large the table says it is.
        len: u32,
    },
    /// Flash refused.
    Flash(FlashStorageError),
    /// A write did not read back.
    NotDurable {
        /// Where in the region.
        at: u32,
    },
}

impl From<FlashStorageError> for RestoreError {
    fn from(error: FlashStorageError) -> RestoreError {
        RestoreError::Flash(error)
    }
}

/// The staging region, and the report of the last restore.
pub struct Staging {
    /// Absolute flash offset of the partition.
    base: u32,
    /// How much of the staged file has been written, while one is arriving.
    writing: Option<u32>,
}

/// The last restore's report, where the web server can read it.
///
/// **Not behind [`crate::rpc`]**, unlike everything else that touches flash,
/// and the reason is that it is not flash: it is a value the state task settles
/// once at boot and once per upload, and a `GET` for it must not be able to
/// make the state task wait. The same argument [`crate::diag`] makes for the
/// diagnostics document.
static REPORT: BlockingMutex<CriticalSectionRawMutex, RefCell<RestoreReportDto>> =
    BlockingMutex::new(RefCell::new(RestoreReportDto::nothing()));

/// What the last upload did. `RestoreOutcomeDto::None` on a device that has
/// never been sent one, which is a value and not an absence.
pub fn report() -> RestoreReportDto {
    REPORT.lock(|cell| cell.borrow().clone())
}

fn publish(report: RestoreReportDto) {
    REPORT.lock(|cell| *cell.borrow_mut() = report);
}

impl Staging {
    /// Find the region.
    ///
    /// **Call this from `main`, not from a task**, as the other four stores say:
    /// the partition table costs about a kilobyte of stack here plus
    /// `esp-storage`'s sector buffer on the unaligned read path.
    pub fn mount(flash: &mut FlashStorage<'_>) -> Result<Staging, RestoreError> {
        let capacity = flash.capacity() as u64;
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(flash, &mut buffer)
                .map_err(RestoreError::PartitionTable)?;
            let entry = table
                .iter()
                .find(|entry| {
                    // Label *and* type, so a label match alone cannot stage a
                    // backup into an app partition somebody named `import`.
                    entry.label_as_str() == PARTITION_LABEL
                        && matches!(entry.partition_type(), PartitionType::Data(_))
                })
                .ok_or(RestoreError::PartitionMissing)?;
            (entry.offset(), entry.len())
        };

        let geometry = || RestoreError::PartitionGeometry { offset: base, len };
        // 64-bit arithmetic, because the point is to catch a table written for
        // a larger flash than the one it is being read on.
        if base as u64 + len as u64 > capacity {
            return Err(geometry());
        }
        if !(base as usize).is_multiple_of(SECTOR) || (len as usize) < REGION_BYTES {
            return Err(geometry());
        }

        let mut staging = Staging {
            base,
            writing: None,
        };
        let state = staging.read_state(flash)?;
        publish(state.report());
        Ok(staging)
    }

    fn read_state(&mut self, flash: &mut FlashStorage<'_>) -> Result<State, RestoreError> {
        // Word-aligned, so `esp-storage` does not copy it through a 4 KB sector
        // buffer on this stack. The other four stores use the same trick.
        #[repr(align(4))]
        struct Aligned([u8; STATE_LEN]);
        let mut bytes = Aligned([0u8; STATE_LEN]);
        flash.read(self.base, &mut bytes.0)?;
        Ok(State::decode(&bytes.0))
    }

    fn write_state(
        &mut self,
        flash: &mut FlashStorage<'_>,
        state: &State,
    ) -> Result<(), RestoreError> {
        flash.erase(self.base, self.base + SECTOR as u32)?;
        #[repr(align(4))]
        struct Aligned([u8; STATE_LEN]);
        let bytes = Aligned(state.encode());
        flash.write(self.base, &bytes.0)?;

        // Durability verified rather than assumed, exactly as
        // `crate::store::FlashStore::commit` does and for the same reason: a
        // write that silently did not land would leave a staged restore that
        // never happens, or an applied one that happens twice.
        let mut back = Aligned([0u8; STATE_LEN]);
        flash.read(self.base, &mut back.0)?;
        if back.0 != bytes.0 {
            return Err(RestoreError::NotDurable { at: self.base });
        }
        publish(state.report());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Staging an upload
// ---------------------------------------------------------------------------

impl Staging {
    /// Whether a backup is staged and waiting for the next boot to read it.
    ///
    /// Asked by `crate::start`, which returns early when it is true so that
    /// [`apply`] runs on `crate::entry`'s frame rather than beneath `start`'s
    /// own twenty kilobytes. See [`apply`] for why that matters.
    pub fn pending(&mut self, flash: &mut FlashStorage<'_>) -> bool {
        self.read_state(flash)
            .map(|state| state.outcome == RestoreOutcomeDto::Staged)
            .unwrap_or(false)
    }

    /// Begin staging a file of `declared` bytes.
    ///
    /// Refuses before touching flash where it can: an upload larger than the
    /// region, and one that does not begin like either format this device
    /// reads. **The second check is the one that matters to a person**, because
    /// the overwhelmingly likely mistake is the wrong file — a firmware image
    /// belongs at `POST /api/v1/ota` — and catching it here turns a staged
    /// upload, a restart and a refusal on the far side into an immediate `400`.
    pub fn begin(
        &mut self,
        flash: &mut FlashStorage<'_>,
        declared: u32,
    ) -> Result<(), ApiErrorCode> {
        if declared == 0 || declared as usize > STAGE_MAX_BYTES {
            return Err(ApiErrorCode::BackupTooLarge);
        }
        let state = self
            .read_state(flash)
            .map_err(|_| ApiErrorCode::BackupUnwritable)?;
        if state.outcome == RestoreOutcomeDto::Staged {
            // Refused rather than replaced: the staged file is applied on the
            // next boot, so overwriting it would silently discard something the
            // operator has already been told was accepted.
            return Err(ApiErrorCode::RestoreInProgress);
        }

        // Erase the whole staged area up front rather than a sector at a time
        // as the pages arrive. It costs one long interrupts-disabled window at
        // the start of an upload instead of four spread through it, and — more
        // usefully — it means a staged file is never a mixture of this upload
        // and the last one, whatever happens next.
        let from = self.base + STAGE_OFFSET as u32;
        flash
            .erase(from, from + STAGE_MAX_BYTES as u32)
            .map_err(|_| ApiErrorCode::BackupUnwritable)?;
        self.writing = Some(0);
        Ok(())
    }

    /// Write one page of the file.
    ///
    /// `bytes` is the lent page from [`crate::ota`]'s channel, so nothing is
    /// copied twice and no buffer of this size exists in this module.
    pub fn page(&mut self, flash: &mut FlashStorage<'_>, bytes: &[u8]) -> Result<(), ApiErrorCode> {
        let Some(written) = self.writing else {
            return Err(ApiErrorCode::BackupUnwritable);
        };
        if written as usize + bytes.len() > STAGE_MAX_BYTES {
            return Err(ApiErrorCode::BackupTooLarge);
        }
        // **The first page decides whether this is a file at all.** Done here
        // rather than in `begin`, because `begin` has no bytes yet — the
        // `Content-Length` arrives with the headers and the magic with the body.
        if written == 0 && !recognisable(bytes) {
            return Err(ApiErrorCode::BackupNotRecognised);
        }
        // `NorFlash::write` takes word-aligned lengths, and every page except
        // the last is a whole page. The last one is rounded up: the region was
        // erased, so the bytes past the end are `0xFF` either way and the state
        // record is what says where the file ends.
        let padded = bytes.len().next_multiple_of(WORD);
        let mut page = Page([0xFFu8; PAGE_BYTES]);
        page.0[..bytes.len()].copy_from_slice(bytes);
        flash
            .write(self.base + STAGE_OFFSET as u32 + written, &page.0[..padded])
            .map_err(|_| ApiErrorCode::BackupUnwritable)?;
        self.writing = Some(written + bytes.len() as u32);
        Ok(())
    }

    /// Every byte has arrived. Record that a restore is staged.
    ///
    /// **Nothing is validated here**, and that is the design rather than an
    /// omission — see this module's docs for the arithmetic that puts the
    /// validation at boot. What this does is make the staged file *findable* by
    /// the boot path, which is the only irreversible step and is therefore one
    /// write of one record.
    pub fn finish(&mut self, flash: &mut FlashStorage<'_>) -> Result<(), ApiErrorCode> {
        let Some(written) = self.writing.take() else {
            return Err(ApiErrorCode::BackupUnwritable);
        };
        let state = State {
            outcome: RestoreOutcomeDto::Staged,
            length: written,
            ..State::nothing()
        };
        self.write_state(flash, &state)
            .map_err(|_| ApiErrorCode::BackupUnwritable)
    }

    /// Give up on an upload, leaving nothing staged.
    ///
    /// The staged bytes are left where they are — they are unreachable without
    /// a state record naming them, and erasing them would be a second long
    /// interrupts-disabled window for no gain.
    pub fn abort(&mut self, flash: &mut FlashStorage<'_>) {
        self.writing = None;
        // Best effort: a failure here leaves the previous report in place,
        // which is the state the device was already in.
        let _ = self.write_state(flash, &State::nothing());
    }
}

/// A page, word-aligned.
///
/// `esp-storage` answers an unaligned buffer by copying it through a 4 KiB
/// sector buffer on this stack, which is the one thing the boot path cannot
/// spare. The other four stores wrap their slots the same way.
#[repr(align(4))]
struct Page([u8; PAGE_BYTES]);

/// Whether these bytes begin like something this device can read.
///
/// Two formats. An `RTSB` container announces itself with a magic. A C++
/// ESPSomfy-RTS backup is **text** whose first field is the format version, so
/// it begins with an ASCII digit possibly preceded by the space padding that
/// firmware's `%3u` writes — `somfy_migrate`'s reader skips leading
/// whitespace in exactly that way, so accepting it here matches what the parser
/// will do rather than guessing at it.
///
/// It is a cheap check and it says nothing about validity; that is settled at
/// boot, by the parser.
fn recognisable(bytes: &[u8]) -> bool {
    if somfy_backup::looks_like_backup(bytes) {
        return true;
    }
    bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| byte.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Exporting
// ---------------------------------------------------------------------------

/// Bytes of the container answered per request.
///
/// **64, and it is chosen for a structural reason before a size one.** It
/// divides the container's header and both of its 2 KiB records exactly, so a
/// chunk never straddles two parts of the file and the walk below is four
/// branches rather than an offset calculator — see the assertion under it.
///
/// The size argument agrees. It is a `crate::rpc::Reply` variant, so it is that
/// many bytes of the `Signal` static every reply shares, *and* the scratch a
/// connection task holds across a socket write, so it is four more copies in the
/// DRAM the Wi-Fi driver's heap is carved from: 320 bytes in total. A whole
/// container is 70 round trips, each of which is two executor polls against one
/// flash read — nothing next to the request that caused it.
pub const EXPORT_CHUNK_BYTES: usize = 64;

/// An export in progress, as the state task sees it.
///
/// Just a checksum. **Everything else is read from flash as it is asked for**,
/// which is what lets a four-kilobyte file leave this device without four
/// kilobytes existing anywhere: the header is rebuilt for the two chunks it
/// spans, and the two records are read straight out of their regions.
///
/// A `crc::Digest` is a reference and a `u32`. It borrows `somfy_backup::CRC`,
/// which is a `static` for exactly this reason — a `const` would be
/// materialised afresh at each use and the borrow could not outlive the
/// expression.
pub struct Export {
    /// How many bytes have been digested, which is also how far the walk has
    /// got. Kept so that a request out of order is a refusal rather than a file
    /// whose checksum is quietly wrong.
    at: usize,
    digest: crc::Digest<'static, u32>,
    /// Where the shade record was when the walk began, and where the estate
    /// was.
    ///
    /// **Pinned at the first chunk rather than looked up per chunk, and it is a
    /// correctness fix before it is a speed one.** The shade table is written
    /// *debounced*, so a write can land in the middle of an export — and a walk
    /// that re-read "the newest slot" each time would then take its first
    /// chunks from one record and its last from another, producing a file whose
    /// embedded record fails its own checksum. That is refused on the way back
    /// in, which is honest, and silent to the person who downloaded it, which is
    /// not.
    ///
    /// Pinning the offsets makes an export a read of two *fixed* slots. A write
    /// during it lands in a different slot of the ring, so the file is a
    /// consistent snapshot of the table as it was when the download started —
    /// which is what a person pressing "export" means.
    ///
    /// It also stops the walk mounting both regions seventy times, which is
    /// seventy partition-table reads and seventy ring scans for one file.
    ///
    /// `None` is a region with nothing readable in it, which exports as blank.
    slots: Option<(Option<u32>, Option<u32>)>,
}

impl Export {
    /// Start a walk.
    pub fn new() -> Export {
        Export {
            at: 0,
            digest: somfy_backup::CRC.digest(),
            slots: None,
        }
    }
}

impl Default for Export {
    fn default() -> Export {
        Export::new()
    }
}

/// One chunk of the container, and how many of its bytes are real.
///
/// The last chunk is short; every other one is full. `len == 0` is the end.
pub struct Chunk {
    /// The bytes.
    pub bytes: [u8; EXPORT_CHUNK_BYTES],
    /// How many of them.
    pub len: usize,
}

// **A chunk never straddles a boundary**, which is what keeps the walk below
// four branches instead of an offset calculator. Sixty-four divides the header,
// both records, and therefore everything before the checksum; the checksum is
// the one short chunk, at the end, where a short chunk means "done".
const _: () = assert!(
    somfy_backup::HEADER_LEN.is_multiple_of(EXPORT_CHUNK_BYTES)
        && somfy_config::SHADE_RECORD_LEN.is_multiple_of(EXPORT_CHUNK_BYTES)
        && somfy_config::ESTATE_RECORD_LEN.is_multiple_of(EXPORT_CHUNK_BYTES),
    "an export chunk must not straddle two parts of the container",
);

/// Bytes of the container before the checksum.
const EXPORT_BODY_BYTES: usize = BACKUP_LEN - 4;

/// Answer the next chunk of the container.
///
/// **Nothing four kilobytes long exists anywhere on this path.** The header is
/// rebuilt for each of the five chunks it spans — it is a hundred bytes of
/// field copying against a flash read, so caching it would cost more DRAM than
/// it saved — and the two records are read straight out of their regions, a
/// chunk at a time.
///
/// Requests must arrive in order. One that does not is refused rather than
/// answered, because the checksum is accumulated as the bytes go past and a
/// gap would make it quietly wrong — which is the one failure a backup must not
/// have, since the file would then be refused on the way back in with nothing
/// saying why.
pub fn export_chunk(
    export: &mut Export,
    at: usize,
    store: &mut crate::store::FlashStore<'static>,
    config: &Option<crate::config::ConfigStore>,
    registry: &somfy_domain::Registry,
) -> Result<Chunk, ApiErrorCode> {
    if at == 0 {
        *export = Export::new();
        export.slots = Some(store.with_flash(|flash| {
            (
                crate::shades::ShadeStore::mount(flash)
                    .ok()
                    .and_then(|mut store| store.newest_offset(flash).ok().flatten()),
                crate::estate::EstateStore::mount(flash)
                    .ok()
                    .and_then(|mut store| store.newest_offset(flash).ok().flatten()),
            )
        }));
    }
    // Unreachable: `at == 0` is the only way in, and it fills this.
    let Some((shades, estate)) = export.slots else {
        return Err(ApiErrorCode::BackupUnwritable);
    };
    if at != export.at {
        return Err(ApiErrorCode::BackupUnwritable);
    }
    if at >= BACKUP_LEN {
        return Ok(Chunk {
            bytes: [0; EXPORT_CHUNK_BYTES],
            len: 0,
        });
    }

    let mut chunk = Chunk {
        bytes: [0; EXPORT_CHUNK_BYTES],
        len: EXPORT_CHUNK_BYTES,
    };

    if at < somfy_backup::HEADER_LEN {
        let mut header = [0u8; somfy_backup::HEADER_LEN];
        somfy_backup::write_header(&meta(store, config), &codes(store, registry), &mut header);
        chunk
            .bytes
            .copy_from_slice(&header[at..at + EXPORT_CHUNK_BYTES]);
    } else if at < somfy_backup::OFF_ESTATE {
        read_region(
            store,
            shades,
            at - somfy_backup::OFF_SHADES,
            &mut chunk.bytes,
        )?;
    } else if at < EXPORT_BODY_BYTES {
        read_region(
            store,
            estate,
            at - somfy_backup::OFF_ESTATE,
            &mut chunk.bytes,
        )?;
    } else {
        // The checksum, over everything already emitted. A short chunk, and the
        // only one — which is why the caller can treat "shorter than a chunk"
        // as "this was the last".
        let crc = core::mem::replace(&mut export.digest, somfy_backup::CRC.digest()).finalize();
        chunk.bytes[..4].copy_from_slice(&crc.to_le_bytes());
        chunk.len = 4;
        export.at = BACKUP_LEN;
        return Ok(chunk);
    }

    export.digest.update(&chunk.bytes[..chunk.len]);
    export.at = at + chunk.len;
    Ok(chunk)
}

/// Copy `out.len()` bytes out of a record pinned at [`Export::new`].
///
/// **A region with nothing readable in it exports as blank**, which is what a
/// freshly flashed board's estate looks like and is a legitimate backup: the
/// decoder on the way back in reads a blank record as `Blank` and refuses it,
/// so nothing pretends an empty estate is an estate.
fn read_region(
    store: &mut crate::store::FlashStore<'static>,
    slot: Option<u32>,
    at: usize,
    out: &mut [u8],
) -> Result<(), ApiErrorCode> {
    let Some(offset) = slot else {
        // `0xFF` is what an erased region reads as and what
        // `somfy_backup::decode` calls `Blank`, so the file says "this board had
        // no table" rather than inventing one.
        out.fill(0xFF);
        return Ok(());
    };
    store.with_flash(|flash| {
        flash
            .read(offset + at as u32, out)
            .map_err(|_| ApiErrorCode::BackupUnwritable)
    })
}

/// The non-secret settings the container carries.
///
/// **Reads the configuration region and takes four fields out of it, none of
/// them a secret.** `WifiCredentials::psk` and `MqttSettings::password` are
/// never touched — there is no field in [`somfy_backup::BackupMeta`] they could
/// be written into, which is the same structural rule `somfy_api::settings`
/// keeps and the reason an export is safe to serve from an unauthenticated
/// `GET`.
fn meta(
    store: &mut crate::store::FlashStore<'static>,
    config: &Option<crate::config::ConfigStore>,
) -> somfy_backup::BackupMeta {
    let Some(config) = config else {
        return somfy_backup::BackupMeta::default();
    };
    let Ok((Some(record), _)) = store.with_flash(|flash| config.load(flash)) else {
        // A region that cannot be read exports as "nothing provisioned", which
        // is what boot does with the same failure. It costs the operator the
        // two hints about what to retype and costs the shades nothing.
        return somfy_backup::BackupMeta::default();
    };
    somfy_backup::BackupMeta {
        ssid: record.wifi.as_ref().map(|wifi| {
            let mut ssid = heapless::String::new();
            // Cannot fail: both are `String<MAX_SSID_LEN>`.
            let _ = ssid.push_str(wifi.ssid());
            ssid
        }),
        psk_was_set: record.wifi.as_ref().is_some_and(|wifi| !wifi.is_open()),
        broker: record
            .mqtt
            .as_ref()
            .map(somfy_config::MqttSettings::address),
        broker_password_was_set: record
            .mqtt
            .as_ref()
            .is_some_and(|mqtt| !mqtt.password().is_empty()),
    }
}

/// The live rolling code of every shade this controller knows.
///
/// **The registry supplies the addresses and the store supplies the codes**,
/// which is why this is here rather than reading the shade record: the record's
/// `initial_code` is a *seed*, and what a backup is worth carrying is where the
/// counter has actually reached.
///
/// A shade with no stored code is left out rather than exported as zero. It has
/// never transmitted, so there is nothing to lose, and a zero would be a code
/// that reads as real.
fn codes(
    store: &mut crate::store::FlashStore<'static>,
    registry: &somfy_domain::Registry,
) -> Codes {
    let mut codes = Codes::new();
    for (_, shade) in registry.shades() {
        if let Ok(Some(code)) = somfy_store::RollingCodeStore::load(store, shade.config.address) {
            if !codes.push(shade.config.address, code.0) {
                // Unreachable: `Codes` holds `somfy_domain::MAX_SHADES` and the
                // registry has that many slots. Reported rather than ignored,
                // because a dropped code is a shade that stops obeying after a
                // restore and nothing else would say so.
                crate::logln!(
                    "backup: no room for the rolling code of address {:#08x} — this backup is \
                     incomplete",
                    shade.config.address,
                );
            }
        }
    }
    codes
}

// ---------------------------------------------------------------------------
// Applying, at boot
// ---------------------------------------------------------------------------

/// Apply a staged restore, if there is one.
///
/// Called from `crate::start` immediately after the rolling-code store mounts
/// and **before** the configuration, shade and estate regions are read, so that
/// what the rest of boot loads is what this wrote.
///
/// # Where this is called from, and why it is not `crate::start`
///
/// **`crate::entry`, after `start` has returned, and then the board resets.**
/// That is three unusual things at once and each is measured rather than
/// preferred.
///
/// This function carries [`STAGE_MAX_BYTES`] of stack — sixteen kilobytes for
/// the staged file — and beneath it a C++ backup costs a
/// `somfy_migrate::MigrationData`, the importer's own tables, and the two
/// records to be written. Walked with `-Zemit-stack-sizes` on the ESP32-S3, the
/// whole chain is about 41 KB. `crate::start`'s own frame is 20,144, so calling
/// this from there would have made a 61 KB chain against 66,148 bytes of stack
/// — and, worse, would have made [`crate::heap::REQUIRED_STACK_BYTES`] that
/// figure, which no division of this chip's DRAM can then satisfy with the
/// margin floor intact. Called from `entry`, the chain is 144 + the main task's
/// `poll` + this, which is comfortably under `crate::heap`'s existing boot
/// chain and therefore moves no constant at all.
///
/// **Then it resets**, rather than letting boot carry on with the regions it
/// just rewrote. Restarting is what makes this need no ordering argument: the
/// next boot reads the new configuration through the path that already reads
/// configuration, announces it through the path that already announces, and
/// nothing here has to be a second copy of either. It costs one extra reboot
/// on the one boot after a restore. `api::routes::restart_for_mqtt` reaches for
/// the same front door for the same reason.
///
/// `#[inline(never)]` on top of all that, so the frame is genuinely popped
/// before `entry` resets — the same attribute and the same reasoning that
/// `crate::start_network` carries, where it was worth 18,576 bytes and a boot
/// loop.
///
/// # Nothing here can stop the controller starting
///
/// It returns nothing. A missing region, an unreadable file, a refused record
/// and a flash write that did not land are all *reported* — into the state
/// record, and from there to the diagnostics screen — and the boot carries on
/// with whatever configuration it already had. The same rule `crate::net` and
/// `crate::mqtt` follow, for the same reason: a service that can stop the radio
/// coming up is not a degradable service.
#[inline(never)]
pub fn apply(staging: &mut Staging, store: &mut crate::store::FlashStore<'_>) {
    let state = match store.with_flash(|flash| staging.read_state(flash)) {
        Ok(state) => state,
        Err(error) => {
            crate::logln!(
                "restore: the staging region could not be read ({:?})",
                error
            );
            return;
        }
    };
    if state.outcome != RestoreOutcomeDto::Staged {
        return;
    }

    crate::logln!(
        "restore: a {} byte backup is staged — reading it before anything else is loaded",
        state.length,
    );

    // Sixteen kilobytes on the boot stack. See this function's docs for why
    // that is affordable here and nowhere else in this firmware.
    let mut staged = Staged([0xFF; STAGE_MAX_BYTES]);
    let length = (state.length as usize).min(STAGE_MAX_BYTES);
    if let Err(error) = store
        .with_flash(|flash| flash.read(staging.base + STAGE_OFFSET as u32, &mut staged.0[..length]))
    {
        crate::logln!("restore: the staged backup could not be read ({:?})", error);
        settle(
            staging,
            store,
            State {
                fault: Fault::Unwritable,
                ..refused()
            },
        );
        return;
    }

    let outcome = read_and_write(store, &staged.0[..length]);
    settle(staging, store, outcome);
}

/// The staged file, word-aligned so `esp-storage` reads straight into it rather
/// than copying through a 4 KiB sector buffer on the same stack.
#[repr(align(4))]
struct Staged([u8; STAGE_MAX_BYTES]);

/// The state a refusal starts from.
///
/// Not `const`: `State` carries an `Option<BackupContentsDto>`, whose
/// `heapless::String`s give it a destructor, and a destructor cannot run in a
/// const evaluation.
fn refused() -> State {
    State {
        outcome: RestoreOutcomeDto::Refused,
        ..State::nothing()
    }
}

/// Write the outcome, and say it on the serial line.
fn settle(staging: &mut Staging, store: &mut crate::store::FlashStore<'_>, state: State) {
    match state.outcome {
        RestoreOutcomeDto::Applied => crate::logln!(
            "restore: applied — {} shades, {} rooms, {} groups, {} warnings. Rolling codes were \
             seeded through seed_if_absent, so every address this board already had a code for \
             kept the one it had.",
            state.shades,
            state.rooms,
            state.groups,
            state.warnings,
        ),
        RestoreOutcomeDto::Refused => crate::logln!(
            "restore: refused ({:?}{}) — nothing was written, and this board is running the \
             configuration it had before the upload",
            state.fault,
            RowNote(state.row.unwrap_or(ROW_NONE)),
        ),
        _ => {}
    }
    if let Err(error) = store.with_flash(|flash| staging.write_state(flash, &state)) {
        // The outcome is lost and the *staged* flag with it, and that is the
        // survivable direction: a state record that could not be rewritten still
        // says `Staged`, so the next boot applies the same file again. Applying
        // twice is harmless — both writes are idempotent and `seed_if_absent`
        // cannot move a code — whereas a lost `Applied` would only cost a
        // report.
        crate::logln!("restore: the outcome could not be recorded ({:?})", error);
    }
}

/// Formats `, record N`, or nothing.
struct RowNote(u8);

impl core::fmt::Display for RowNote {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == ROW_NONE {
            Ok(())
        } else {
            write!(formatter, ", record {}", self.0)
        }
    }
}

/// Read the staged bytes as whichever format they are, and write the result.
///
/// **All or nothing.** Every refusal below returns before a byte of the shade or
/// estate region is touched, and the two writes that follow are the last two
/// things this function does. That is spec R3 — refuse, naming the field, never
/// repair — and it is also what `ShadeRecord::for_each` already enforces one
/// level down for the same reason: a shade's id is its row, so importing what
/// parses and skipping what does not renumbers everything after the gap.
fn read_and_write(store: &mut crate::store::FlashStore<'_>, staged: &[u8]) -> State {
    if somfy_backup::looks_like_backup(staged) {
        read_own(store, staged)
    } else {
        read_foreign(store, staged)
    }
}

/// This firmware's own `RTSB` container.
#[inline(never)]
fn read_own(store: &mut crate::store::FlashStore<'_>, staged: &[u8]) -> State {
    let format = Some(BackupFormatDto::SomfyRs);
    let Ok(bytes) = <&[u8; BACKUP_LEN]>::try_from(staged) else {
        // The upload was not the size a container is. Reported as damaged
        // rather than as unrecognised: the magic said what it is, so what is
        // wrong is the transfer.
        return State {
            format,
            fault: Fault::Damaged,
            ..refused()
        };
    };
    let backup = match somfy_backup::decode(bytes) {
        Ok(backup) => backup,
        Err(error) => {
            crate::logln!("restore: {}", error);
            return State {
                format,
                fault: match error {
                    somfy_backup::BackupError::Version(_) => Fault::UnsupportedVersion,
                    somfy_backup::BackupError::Magic | somfy_backup::BackupError::Blank => {
                        Fault::NotRecognised
                    }
                    _ => Fault::Damaged,
                },
                ..refused()
            };
        }
    };

    // **The two records are decoded by `somfy_backup`, not here**, and that is
    // the fix for a bug a live board found: this function used to call the two
    // decoders itself and read `Blank` as damage, so an ESP32-S3 whose estate
    // region had never been written refused its own export. `somfy_backup::decode`
    // could not have caught it — it checks the container, and the records inside
    // are opaque bytes to a checksum — so the reading lives over there where a
    // host test can run the same code this does. `Backup::tables` carries the
    // rule and `somfy-backup/tests/container.rs` holds it.
    let mut table = match backup.shade_table() {
        Ok(table) => table,
        Err(error) => {
            crate::logln!("restore: {}", error);
            return State {
                format,
                fault: Fault::Damaged,
                ..refused()
            };
        }
    };
    let estate = match backup.estate_table() {
        Ok(estate) => estate,
        Err(error) => {
            crate::logln!("restore: {}", error);
            return State {
                format,
                fault: Fault::Damaged,
                ..refused()
            };
        }
    };

    // **The table and the codes are married here, and this is the one place
    // that does it.** The container's shade record carries each shade's *seed*
    // — the code its table was provisioned with — and the live counters travel
    // beside it in the code block. Writing the record as it stands would seed
    // every address from a value the motors passed months ago.
    //
    // Rather than seed separately, the live code is written **into** the record
    // before it is stored, so the boot path's existing `provision_shades` pass
    // — which already seeds from `initial_code` through
    // `somfy_store::seed_if_absent` — is the only thing that ever plants a
    // code. One mechanism rather than two that could disagree.
    graft_codes(&mut table, &backup.codes);

    let contents = Some(BackupContentsDto {
        ssid: backup.meta.ssid.clone(),
        psk_was_set: backup.meta.psk_was_set,
        broker: backup.meta.broker.map(|address| {
            let mut text = heapless::String::new();
            // Cannot fail: the widest dotted quad is fifteen characters and the
            // field holds twenty-one.
            let _ = core::fmt::Write::write_fmt(&mut text, format_args!("{address}"));
            text
        }),
        broker_password_was_set: backup.meta.broker_password_was_set,
    });

    let shades = table.shades.len() as u8;
    let rooms = estate.rooms.len() as u8;
    let groups = estate.groups.len() as u8;
    match write_regions(store, &mut table, &estate) {
        Ok(()) => State {
            outcome: RestoreOutcomeDto::Applied,
            format,
            shades,
            rooms,
            groups,
            contents,
            ..State::nothing()
        },
        Err(fault) => State {
            format,
            fault,
            contents,
            ..refused()
        },
    }
}

/// Put the live rolling codes into the table that is about to be written.
///
/// A shade whose address the block does not mention keeps the seed the record
/// carried. That is the right default rather than a gap: it is what a table
/// exported from a board that had never driven that shade looks like, and
/// `seed_if_absent` will refuse to plant it anyway if the address already has a
/// code.
fn graft_codes(table: &mut somfy_config::ShadeRecord, codes: &Codes) {
    for shade in table.shades.iter_mut() {
        if let Some((_, code)) = codes
            .iter()
            .find(|(address, _)| *address == shade.config.address)
        {
            shade.initial_code = somfy_rts::RollingCode(code);
        }
    }
}

/// A configuration backup exported by a C++ ESPSomfy-RTS controller.
///
/// The parse and the mapping are `somfy_migrate` and `somfy_config::import`,
/// both host-tested, both **the same code the `provision_shades` host tool
/// runs** — which is the whole reason this could be added without a second
/// importer to keep in step. What is left here is turning a refusal into a byte
/// and writing two regions.
#[inline(never)]
fn read_foreign(store: &mut crate::store::FlashStore<'_>, staged: &[u8]) -> State {
    let format = Some(BackupFormatDto::EspSomfyRts);
    let mut warnings = 0usize;
    let imported = match parse_foreign(staged, &mut warnings) {
        Ok(imported) => imported,
        Err(refusal) => {
            crate::logln!("restore: {}", refusal);
            let (fault, row) = fault_of(&refusal);
            return State {
                format,
                fault,
                row,
                ..refused()
            };
        }
    };

    if imported.favourites > 0 {
        crate::logln!(
            "restore: {} shades had a 'my' favourite on the old controller and this firmware has \
             nowhere to keep one — they import without it and it can be set again at the motor",
            imported.favourites,
        );
    }
    // **A misaligned record is reported and applied, where the host tool asks
    // first.** The difference is that the host tool has an operator at a
    // terminal and a boot path does not; refusing outright would throw away a
    // table that is probably right, and applying silently would hide a rolling
    // code that is probably wrong. So it lands, loudly, and the diagnostics
    // screen carries the count.
    if imported.skipped_resyncs > 0 {
        crate::logln!(
            "restore: !! {} records in this backup did not align exactly. At least one value in \
             this table may be wrong, INCLUDING A ROLLING CODE — check every shade moves before \
             trusting it.",
            imported.skipped_resyncs,
        );
    }

    // **Moved out of the import rather than cloned.** Two `heapless::Vec`s of
    // thirty-two `StoredShade`s are about two kilobytes, and a clone would put
    // both copies on this stack at once for no gain — `imported` is not read
    // again except for its estate, which is a disjoint field.
    let mut table = somfy_config::ShadeRecord {
        seq: 0,
        announced: somfy_config::Announced::NONE,
        shades: imported.shades,
        links: imported.links,
    };
    let shades = table.shades.len() as u8;
    let rooms = imported.estate.rooms.len() as u8;
    let groups = imported.estate.groups.len() as u8;
    // Saturating rather than truncating: 688 warnings is a legal import and
    // `RestoreReportDto::warnings` is a `u8`, so a wrapping cast would report
    // 176 of them. The screen's job with this number is to say "look at the
    // log", and 255 says that as well as 688 does.
    let warnings = warnings.min(u8::MAX as usize) as u8;

    match write_regions(store, &mut table, &imported.estate) {
        Ok(()) => State {
            outcome: RestoreOutcomeDto::Applied,
            format,
            shades,
            rooms,
            groups,
            warnings,
            // A C++ backup carries no network credentials at all — that
            // controller keeps them in NVS rather than in the file — so there
            // is nothing to tell the operator to retype and the report says so
            // by leaving this absent.
            ..State::nothing()
        },
        Err(fault) => State {
            format,
            fault,
            ..refused()
        },
    }
}

/// Parse a C++ backup, and give back only what will be written.
///
/// **`#[inline(never)]`, and it is the expensive half being kept in its own
/// frame.** A `somfy_migrate::MigrationData` is about five and a half
/// kilobytes and the importer's own room-index table is two more; neither
/// outlives this call, and inlined they would still be on the stack while the
/// records they produced were written. Measured: it is worth about seven
/// kilobytes on a chain that also carries a sixteen-kilobyte staged file.
///
/// **`read_backup_with`, not `read_backup`.** The collecting form builds a
/// `heapless::Vec<Warning, 688>` — 33,024 bytes, nine tenths of an
/// `somfy_config::import::Import` — which is right for a host tool printing a
/// list and impossible here. The sink form raises the same warnings in the same
/// order and keeps none of them; each is written to the log as it is raised,
/// which is where the report's count points a person anyway. The count itself
/// comes from the importer rather than from the sink, so a sink that forgot to
/// count could not make an import look clean.
#[inline(never)]
fn parse_foreign(
    staged: &[u8],
    warnings: &mut usize,
) -> Result<somfy_config::import::ImportedTable, somfy_config::import::Refusal> {
    // **Two calls, each `#[inline(never)]`, rather than
    // `read_backup_with`.** That function is the two below composed, and
    // composed they share a frame: the parser's own scratch and the mapper's
    // 2 KiB room-index table were live at the same time as the
    // `MigrationData` between them, for no reason except that the compiler had
    // no seam to put them on either side of. Splitting them was worth about
    // twelve kilobytes, measured, on the one chain in this firmware that has
    // none to spare.
    let data = parse_migration(staged).map_err(somfy_config::import::Refusal::Unreadable)?;
    let imported = map_migration(&data)?;
    *warnings = imported.warnings;
    Ok(imported)
}

/// The parser, in its own frame.
#[inline(never)]
fn parse_migration(
    staged: &[u8],
) -> Result<somfy_migrate::MigrationData, somfy_migrate::MigrateError> {
    somfy_migrate::parse_backup(staged)
}

/// The mapping onto this device's records, in its own frame.
///
/// The warning sink logs each caveat as it is raised and keeps none: the
/// collecting form of this call builds a `heapless::Vec<Warning, 688>`, 33,024
/// bytes, which is right for a host tool printing a list and impossible on this
/// stack. `ImportedTable::warnings` is the count, and it is incremented by the
/// importer rather than by the sink, so a sink that forgot to count could not
/// make an import look clean.
#[inline(never)]
fn map_migration(
    data: &somfy_migrate::MigrationData,
) -> Result<somfy_config::import::ImportedTable, somfy_config::import::Refusal> {
    somfy_config::import::import_with(data, &mut |warning| {
        crate::logln!(
            "restore: !! {} {} — {}",
            warning.subject,
            warning.name,
            warning.caveat,
        );
    })
}

/// Which stored byte a refusal becomes, and which record it came from.
///
/// The interesting half is the **row**: a refusal that names a record lets the
/// screen say "shade 12" instead of "a shade", and the operator can look at
/// their old controller and see which one.
fn fault_of(refusal: &somfy_config::import::Refusal) -> (Fault, Option<u8>) {
    use somfy_config::import::Refusal;
    use somfy_config::{ShadeError, TravelField};
    use somfy_domain::DomainError;

    let row = |index: usize| u8::try_from(index).ok();
    match refusal {
        Refusal::Unreadable(somfy_migrate::MigrateError::UnsupportedVersion(_)) => {
            (Fault::UnsupportedVersion, None)
        }
        Refusal::Unreadable(_) => (Fault::NotRecognised, None),
        Refusal::NoShades => (Fault::NotRecognised, None),
        Refusal::TooManyShades | Refusal::TooManyLinks { .. } | Refusal::TooManyEstate { .. } => {
            (Fault::RegistryFull, None)
        }
        Refusal::Unnamed { index } => (Fault::NameEmpty, row(*index)),
        Refusal::Shade { index, error, .. } => (
            match error {
                ShadeError::TravelTimeZero {
                    field: TravelField::Up | TravelField::Down,
                } => Fault::TravelTimeZero,
                ShadeError::Domain(DomainError::NameTooLong) => Fault::NameTooLong,
                // Everything else `ShadeConfig::new` raises is about the
                // address: the domain has no empty-name refusal — a migrated
                // backup may legitimately carry one and refusing it there would
                // lose the shade — so `Refusal::Unnamed` is what catches that,
                // above, and it carries its own row.
                ShadeError::Domain(_) => Fault::InvalidAddress,
            },
            row(*index),
        ),
        Refusal::DuplicateAddress { index, .. } | Refusal::GroupAddressClash { index, .. } => {
            (Fault::AddressInUse, row(*index))
        }
        Refusal::Link { index, .. } => (Fault::InvalidAddress, row(*index)),
        Refusal::DuplicateRoomId { index, .. } => (Fault::AddressInUse, row(*index)),
        Refusal::GroupAddress { index, .. } => (Fault::InvalidAddress, row(*index)),
    }
}

/// Write the shade table and the estate, in that order.
///
/// # Why the *old* announcement bitmap is carried forward
///
/// A retained Home Assistant discovery config outlives the device that
/// published it, so a shade that disappears has to be *retired* — a
/// zero-length publish to the topic it was announced at — and the only thing
/// that can name it afterwards is the `announced` bitmap beside the table.
/// `somfy_config::Catalog::orphans` is what reads it, at the next boot.
///
/// Writing `Announced::NONE` here would therefore leave every entity of the
/// replaced installation on the broker forever, with nothing behind them. That
/// is the exact failure `docs/plans/…-plan6-persistence-ota.md` records as
/// costing 49 retained topics deleted by hand — "deleting config and then
/// discovering the orphans" — so the bitmap the *previous* record carried is
/// read first and stamped onto the new one. Slots that survive are re-announced
/// with their new details; slots that do not are orphans, and the next boot
/// retires them.
///
/// # Why the estate is written second
///
/// It names shades by *row of the shade table*. An estate written beside a
/// different table points at the wrong shades, so the two are written together
/// or not at all — and if the second write fails, the first is reported as a
/// failure too, because a table with a stale estate beside it is worse than
/// neither.
#[inline(never)]
fn write_regions(
    store: &mut crate::store::FlashStore<'_>,
    table: &mut somfy_config::ShadeRecord,
    estate: &somfy_config::EstateRecord,
) -> Result<(), Fault> {
    store.with_flash(|flash| {
        let mut shades = crate::shades::ShadeStore::mount(flash).map_err(|error| {
            crate::logln!("restore: the shades region is unavailable ({:?})", error);
            Fault::Unwritable
        })?;

        // The previous announcement set, so the orphans of the table being
        // replaced can still be named. See this function's docs.
        let (_, header) = shades
            .load_with(flash, |_, _| {}, |_| {})
            .map_err(|error| {
                crate::logln!("restore: the shades region is unreadable ({:?})", error);
                Fault::Unwritable
            })?;
        table.announced = header.map(|header| header.announced).unwrap_or_default();

        let mut estates = crate::estate::EstateStore::mount(flash).map_err(|error| {
            crate::logln!("restore: the estate region is unavailable ({:?})", error);
            Fault::Unwritable
        })?;

        shades.store(flash, table).map_err(|error| {
            crate::logln!(
                "restore: the shade table could not be written ({:?})",
                error
            );
            Fault::Unwritable
        })?;
        estates.store(flash, estate).map_err(|error| {
            crate::logln!(
                "restore: the shade table was written and the estate was not ({:?}) — the rooms \
                 and groups on this board now describe the table it had before",
                error,
            );
            Fault::Unwritable
        })?;
        Ok(())
    })
}
