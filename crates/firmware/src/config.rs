//! The persisted device configuration, on real flash.
//!
//! Same shape as [`crate::store`] and for the same reasons: everything worth
//! testing — the record's encoding and its validity check, the credential
//! rules, the slot arithmetic — lives on the host side in `somfy-config` and
//! `somfy-store`, and what is left here is flash I/O.
//!
//! ## This is a stopgap
//!
//! Plan 6 replaces it. It exists now because a firmware with no persisted
//! configuration has no network, and a Plan 5 whose every feature is
//! unobservable would be verified only by the compiler. `somfy_config`'s
//! module docs carry the full argument.
//!
//! ## What is stored here is not protected
//!
//! The Wi-Fi passphrase is written in the clear. Flash encryption is not
//! enabled, so anyone holding the board can read it out with
//! `espflash read-flash`. Said plainly rather than obscured; see
//! [`somfy_config`] for why an obfuscation scheme would be worse than saying
//! so.
//!
//! ## Why this one does not refuse the way the rolling-code store does
//!
//! [`crate::store::FlashStore`] refuses to act on a region it cannot read,
//! because guessing there costs the user a physical re-pairing procedure at
//! every shade. The cost here is a Wi-Fi connection and one re-provisioning
//! step, so this store **reports** damage and answers "no configuration"
//! instead of stopping the controller.
//!
//! That difference is deliberate and it is the degradability requirement in
//! miniature: an unreadable config region must leave the radio working. What
//! it must **not** do is stay quiet about it, so [`ConfigStore::load`] hands
//! back a [`ConfigSurvey`] whose `damaged` count the caller prints either way.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{self, PartitionType};
use esp_storage::{FlashStorage, FlashStorageError};
use heapless::Vec;
use somfy_config::{
    ConfigRecord, MqttSettings, Namespaces, RecordError, WifiCredentials, CONFIG_RECORD_LEN,
};
use somfy_store::{newest_slot, SectorRing, SlotWrite};

/// Partition holding the config ring. Defined by `partitions.csv` in this
/// crate, and looked up by label for the same reason the rolling-code region
/// is: a compiled-in offset keeps working right up until the app partition
/// grows past it, and then writes configuration over program text.
pub const PARTITION_LABEL: &str = "wificfg";

/// Flash erase unit, and therefore the alignment the partition must sit on.
const SECTOR: usize = <FlashStorage as NorFlash>::ERASE_SIZE;

/// Largest ring this build will scan, for the same reason
/// [`crate::store`] has one: the scan holds a sequence number per slot in a
/// fixed array, and a partly-scanned ring would sometimes name an older record
/// as newest.
const MAX_SLOTS: usize = 64;

/// Bytes of partition table read at mount. See [`crate::store`].
const PARTITION_TABLE_BYTES: usize = 1024;

/// Distinct namespace pairs kept from a scan of the ring.
///
/// R5 requires the retained configs published under an *old* `state_root` or
/// `discovery_prefix` to be cleared before the new ones go out, and the only
/// record of the old values is the older records still readable in the ring.
/// **This is the scan's capacity, not the number of stale pairs it yields.**
/// The scan collects every distinct pair it finds, the one in use included, and
/// `load` removes that one afterwards — so three slots deliver at most *two*
/// stale pairs. Two is one more than the case that actually happens, a board
/// re-provisioned once.
///
/// Bounded because each surviving pair becomes a whole `MqttConfig` in the
/// broker task's statically allocated future, which comes out of the same DRAM
/// the main stack is carved from.
///
/// Two further limits are stated rather than solved. Which pairs win is slot
/// order, not age order, because the ring wraps — so a device re-provisioned
/// onto more namespace pairs than this holds may keep the wrong ones.
/// `superseded_truncated` reports when that happened, and the real fix is to
/// enumerate the broker's retained store rather than to read the ring.
pub const MAX_SUPERSEDED: usize = 3;

// The same three relationships `crate::store` asserts, restated for this
// record length. A divergence would neither fail to build nor fail to link; it
// would corrupt a write at the moment the ring first wraps.
const _: () = assert!(
    CONFIG_RECORD_LEN.is_multiple_of(<FlashStorage as NorFlash>::WRITE_SIZE),
    "a config record must be a whole number of flash write words"
);
const _: () = assert!(
    SECTOR.is_multiple_of(CONFIG_RECORD_LEN),
    "config records must tile the flash erase sector exactly"
);
const _: () = assert!(
    CONFIG_RECORD_LEN.is_multiple_of(<FlashStorage as ReadNorFlash>::READ_SIZE),
    "a config record must be a whole number of flash read units"
);

/// Why the config store could not do what was asked.
///
/// Each payload exists to be printed, and rustc's dead-code analysis
/// deliberately does not count a derived `Debug` as a read — so without the
/// allow it reports every payload as unused. `NotDurable` additionally has no
/// constructor in the controller image at all: only `config-check` writes.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// The partition table could not be read or parsed.
    PartitionTable(partitions::Error),
    /// No partition labelled [`PARTITION_LABEL`]. The device was flashed
    /// without this crate's `partitions.csv`, or with an older one.
    PartitionMissing,
    /// The partition exists but is the wrong shape for the ring.
    PartitionGeometry { offset: u32, len: u32 },
    /// The flash refused a read, write or erase.
    Flash(FlashStorageError),
    /// The record read back after a write is not the record written.
    NotDurable,
    /// A slot decoded during the scan and did not decode when it was read
    /// again — two reads of the same cells disagreeing, which is failing
    /// flash. [`ConfigStore::load`] answers that with "no configuration", but
    /// [`ConfigStore::store`] must not: see the refusal there.
    Unstable { valid: usize },
    /// A slot index outside the ring. Unreachable — every index comes from the
    /// ring itself — but an error rather than a panic, because a panic here
    /// would take the radio off the air over a configuration problem.
    SlotOutOfRange { slot: usize },
}

impl From<FlashStorageError> for ConfigError {
    fn from(error: FlashStorageError) -> Self {
        Self::Flash(error)
    }
}

/// What a scan of the config ring found. A diagnostic, not a contract — except
/// for `superseded`, which the MQTT session acts on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigSurvey {
    /// Slots in the ring.
    pub slots: usize,
    /// Slots holding a record that passed its checksum.
    pub valid: usize,
    /// Slots never written since the last erase.
    pub blank: usize,
    /// Slots holding something that is neither — a torn write, or damage.
    pub damaged: usize,
    /// Sequence number of the newest valid record, if there is one.
    pub newest_seq: Option<u32>,
    /// Namespace pairs seen in the ring that the newest record does not use,
    /// and which the ring therefore may still be advertising on a broker.
    ///
    /// Not `Copy`, which is why [`ConfigSurvey`] is not either. It earns the
    /// cost: without it a re-provisioned device leaves its old discovery
    /// configs on the broker forever, which is exactly the orphan R5 forbids.
    pub superseded: Vec<Namespaces, MAX_SUPERSEDED>,
    /// True if the ring held more distinct namespace pairs than
    /// [`MAX_SUPERSEDED`], so some old configs will not be cleared. Reported
    /// rather than hidden: the alternative is a silently incomplete cleanup.
    pub superseded_truncated: bool,
}

/// One slot's bytes, aligned so `esp-storage` reads and writes them directly
/// rather than detouring through a 4 KB temporary on this stack.
#[repr(C, align(4))]
struct Slot([u8; CONFIG_RECORD_LEN]);

/// The flash-backed configuration store.
pub struct ConfigStore<'d> {
    flash: FlashStorage<'d>,
    /// Absolute flash offset of the partition.
    base: u32,
    ring: SectorRing,
}

impl<'d> ConfigStore<'d> {
    /// Find the config partition and take ownership of the flash.
    ///
    /// **Call this from `main`, not from a task**, for the same reason
    /// [`crate::store::FlashStore::mount`] says so: the partition table costs
    /// about 1 KB of stack here plus `esp-storage`'s 4 KB sector buffer on the
    /// unaligned read path.
    pub fn mount(mut flash: FlashStorage<'d>) -> Result<Self, ConfigError> {
        let capacity = flash.capacity() as u64;
        let (base, len) = {
            let mut buffer = [0u8; PARTITION_TABLE_BYTES];
            let table = partitions::read_partition_table(&mut flash, &mut buffer)
                .map_err(ConfigError::PartitionTable)?;
            let entry = table
                .iter()
                .find(|entry| {
                    // Label *and* type, so a label match alone cannot mount an
                    // app partition somebody named `wificfg` and erase the
                    // firmware with the first write.
                    entry.label_as_str() == PARTITION_LABEL
                        && matches!(entry.partition_type(), PartitionType::Data(_))
                })
                .ok_or(ConfigError::PartitionMissing)?;
            (entry.offset(), entry.len())
        };

        let geometry = || ConfigError::PartitionGeometry { offset: base, len };
        // Checked in 64-bit arithmetic, because the point is to catch a table
        // written for a larger flash than the one it is being read on.
        if base as u64 + len as u64 > capacity {
            return Err(geometry());
        }
        if !(base as usize).is_multiple_of(SECTOR) {
            return Err(geometry());
        }
        let ring = SectorRing::new(len as usize, CONFIG_RECORD_LEN, SECTOR).ok_or_else(geometry)?;
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

    /// The newest readable configuration, and what the scan saw getting there.
    ///
    /// `Ok((None, survey))` means no slot holds a readable record. Unlike the
    /// rolling-code store that is **not** refused even when `survey.damaged`
    /// is non-zero: see this module's docs for why the two stores answer
    /// damage differently. The count is returned so the caller can say so.
    pub fn load(&mut self) -> Result<(Option<ConfigRecord>, ConfigSurvey), ConfigError> {
        let mut scan = self.scan()?;
        let current = scan
            .newest
            .as_ref()
            .and_then(|(_, record)| record.mqtt.as_ref())
            .map(MqttSettings::namespaces);
        // The pair in use is not superseded by itself. Everything left is a
        // namespace this device has published under and is not publishing
        // under now — see [`MAX_SUPERSEDED`].
        scan.namespaces
            .retain(|found| Some(found) != current.as_ref());
        let survey = ConfigSurvey {
            slots: scan.valid + scan.blank + scan.damaged,
            valid: scan.valid,
            blank: scan.blank,
            damaged: scan.damaged,
            newest_seq: scan.newest.as_ref().map(|(_, record)| record.seq),
            superseded: scan.namespaces,
            superseded_truncated: scan.namespaces_truncated,
        };
        Ok((scan.newest.map(|(_, record)| record), survey))
    }

    /// Append a record carrying `wifi` and `mqtt`, and prove the bytes landed.
    ///
    /// `None` in either position clears that half, which is a value the region
    /// can hold and not the same fact as a blank region — see
    /// [`somfy_config::ConfigRecord`].
    #[allow(
        dead_code,
        reason = "the controller only reads this region; `config-check` includes \
                  this file by path and is the binary that writes it"
    )]
    pub fn store(
        &mut self,
        wifi: Option<WifiCredentials>,
        mqtt: Option<MqttSettings>,
    ) -> Result<(), ConfigError> {
        let scan = self.scan()?;

        // A write may not proceed on a ring that holds readable records but
        // could not name a newest one. `scan` reaches that state when the
        // winner decoded on the first pass and not on the second — two reads
        // of the same cells disagreeing — and answering it the way `load`
        // does would be silently destructive here.
        //
        // The damage is worth spelling out, because none of it is visible from
        // the call site. `SlotLayout::next_write(None)` aims at **slot 0 with
        // sequence 0**. Slot 0 starts an erase unit, so `append` erases the
        // first sector — up to fifteen other records, possibly including the
        // one that is still readable. Then it writes sequence 0 while records
        // numbered far higher survive in the other sector, and `newest_slot`'s
        // wrapping comparison ranks 0 as ancient: the credential just written
        // would never be returned by `load` again. The read-back check in
        // `append` would pass and this function would return `Ok`.
        //
        // `somfy_store::SlotLayout`'s docs state the precondition this
        // violates outright — never restart the counter from a low value while
        // high-numbered records may still be readable — and
        // `crate::store::Scan::newest_or_refuse` is where the rolling-code
        // store refuses the same thing. This is that refusal.
        if scan.newest.is_none() && scan.valid > 0 {
            return Err(ConfigError::Unstable { valid: scan.valid });
        }

        let newest = scan.newest.as_ref().map(|(slot, record)| SlotWrite {
            slot: *slot,
            seq: record.seq,
        });
        let aim = self.ring.layout().next_write(newest);

        // Where the ring points is where the write goes *unless* that slot
        // still holds something. `write_slot` steps over the wreckage a torn
        // write leaves; writing into it would only clear more bits.
        let slot_count = self.ring.layout().slot_count();
        let slot = self
            .ring
            .write_slot(aim.slot, &scan.free[..slot_count])
            .ok_or(ConfigError::SlotOutOfRange { slot: aim.slot })?;

        let record = ConfigRecord {
            seq: aim.seq,
            wifi,
            mqtt,
        };
        self.append(slot, &record)
    }

    /// Read every slot: the newest valid record, which slots are erased, and a
    /// tally of what each one held.
    fn scan(&mut self) -> Result<Scan, ConfigError> {
        let slot_count = self.ring.layout().slot_count();
        let mut sequences = [None; MAX_SLOTS];
        let mut free = [false; MAX_SLOTS];
        let (mut valid, mut blank, mut damaged) = (0, 0, 0);
        let mut namespaces: Vec<Namespaces, MAX_SUPERSEDED> = Vec::new();
        let mut namespaces_truncated = false;

        for slot in 0..slot_count {
            match self.read_slot(slot)? {
                Ok(record) => {
                    sequences[slot] = Some(record.seq);
                    valid += 1;
                    // Collected here, while the record is decoded and in hand,
                    // rather than by a second pass. Which of these is *stale*
                    // is not knowable until every slot has been read, so the
                    // filtering happens in `load`.
                    if let Some(found) = record.mqtt.as_ref().map(MqttSettings::namespaces) {
                        if !namespaces.contains(&found) && namespaces.push(found).is_err() {
                            namespaces_truncated = true;
                        }
                    }
                }
                // Only an erased slot can take a write without an erase first.
                // A damaged one is emphatically not free.
                Err(RecordError::Blank) => {
                    free[slot] = true;
                    blank += 1;
                }
                Err(_) => damaged += 1,
            }
        }

        // `newest_slot` owns the wrap-around comparison, so the ordering rule
        // stays in one host-tested place rather than being re-derived here.
        //
        // The winner is read a second time rather than kept from the first
        // pass, for the same reason `crate::store` does it: which slot wins is
        // only known once every sequence number is in hand. Where the two
        // differ is what a disagreement means. There, two reads of the same
        // cells answering differently is `Unstable` and the store refuses;
        // here it drops back to "no configuration", which is the degraded
        // answer this region is allowed to give. It is not silent — the survey
        // still reports `valid` above zero next to a `None` result, and that
        // contradiction is exactly the thing to look at.
        let newest = match newest_slot(&sequences[..slot_count]) {
            None => None,
            Some(slot) => self.read_slot(slot)?.ok().map(|record| (slot, record)),
        };
        Ok(Scan {
            newest,
            free,
            valid,
            blank,
            damaged,
            namespaces,
            namespaces_truncated,
        })
    }

    /// Read one slot and try to decode it.
    ///
    /// Read one slot at a time rather than in batches: this ring is scanned
    /// once at boot and once per provisioning write, so the handful of extra
    /// flash transactions costs nothing that matters, and a single 256-byte
    /// buffer is a smaller thing to hold on the stack of a `main` that has
    /// already paid for the partition table.
    fn read_slot(&mut self, slot: usize) -> Result<Result<ConfigRecord, RecordError>, ConfigError> {
        let offset = self.offset(slot)?;
        let mut buffer = Slot([0u8; CONFIG_RECORD_LEN]);
        self.flash.read(offset, &mut buffer.0)?;
        Ok(ConfigRecord::decode(&buffer.0))
    }

    /// Append `record` at `slot`, erasing that slot's sector first if it starts
    /// one, then prove the bytes landed.
    #[allow(
        dead_code,
        reason = "reachable only through `store`; see the allow there"
    )]
    fn append(&mut self, slot: usize, record: &ConfigRecord) -> Result<(), ConfigError> {
        let offset = self.offset(slot)?;

        if let Some(sector) = self.ring.erase_before(slot) {
            let from = self.base + sector as u32;
            self.flash.erase(from, from + SECTOR as u32)?;
        }

        // Through `Slot` rather than straight from `encode`: a bare `[u8; N]`
        // is byte-aligned, and `esp-storage` answers an unaligned buffer by
        // copying it through a 4 KB sector buffer on this stack.
        let bytes = Slot(record.encode());
        self.flash.write(offset, &bytes.0)?;

        match self.read_slot(slot)? {
            Ok(written) if written == *record => Ok(()),
            _ => Err(ConfigError::NotDurable),
        }
    }

    /// Absolute flash offset of `slot`.
    fn offset(&self, slot: usize) -> Result<u32, ConfigError> {
        self.ring
            .layout()
            .offset(slot)
            .map(|offset| self.base + offset as u32)
            .ok_or(ConfigError::SlotOutOfRange { slot })
    }
}

/// What one pass over the ring found.
struct Scan {
    /// The newest valid record and the slot holding it.
    newest: Option<(usize, ConfigRecord)>,
    /// `free[i]` — slot `i` is erased, so a record can be written into it
    /// without erasing anything first. Indices past the ring stay false.
    #[allow(dead_code, reason = "read only by `store`; see the allow there")]
    free: [bool; MAX_SLOTS],
    /// Slots holding a record that passed its checksum.
    valid: usize,
    /// Slots never written since the last erase.
    blank: usize,
    /// Slots holding something that is neither.
    damaged: usize,
    /// Every distinct namespace pair any readable record names, the newest
    /// included. `load` removes the one in use; what is left is stale.
    namespaces: Vec<Namespaces, MAX_SUPERSEDED>,
    /// True if a distinct pair had to be dropped for lack of room above.
    namespaces_truncated: bool,
}
