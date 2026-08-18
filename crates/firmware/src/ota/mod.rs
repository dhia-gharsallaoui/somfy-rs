//! Over-the-air updates: the flash, the `otadata` record, and the boot-side
//! self-test that decides whether the image running is allowed to stay.
//!
//! Everything *decided* here lives in [`somfy_ota`], host-tested. What is left
//! in this file is the three things only a device can do: address flash,
//! rewrite one thirty-two byte record in `otadata`, and reset.
//!
//! # The shape, because it is not obvious
//!
//! The flash peripheral has exactly one owner in this firmware and it is the
//! **state task** (`crate::store::FlashStore` holds the only `FlashStorage`
//! there is; `esp_storage::FlashStorage::new` takes the `FLASH` peripheral and
//! panics on a second call). The upload arrives on an **HTTP connection task**.
//! So the megabyte has to cross a task boundary, and it has to cross it without
//! being copied — a per-connection buffer costs its size times
//! [`crate::api::HTTP_TASKS`] out of the DRAM the Wi-Fi driver's heap is carved
//! from, which `crate::heap` prices in Wi-Fi headroom rather than in bytes.
//!
//! [`embassy_sync::zerocopy_channel`] is what makes that possible in safe Rust:
//! the connection task is *lent* a `&mut Page` that lives in a `static`, reads
//! the socket straight into it, and hands it back. Only a pointer is ever live
//! across an await, so the buffer is paid for once instead of four times.
//!
//! The **handshake is the existing [`crate::rpc`] seam** rather than a second
//! one. That is not thrift: `Rpc` already serialises callers with a gate and
//! already lands in the state task's `select`, so a page write is one more arm
//! of a `match` rather than a fifth thing for that task to wait on — and the
//! state task's future is materialised twice on the deepest stack chain this
//! firmware has, so a fifth arm would be paid for there.
//!
//! # What one page costs the rest of the device
//!
//! A page write is a flash **sector erase** every [`SECTOR_BYTES`] bytes, and a
//! sector erase runs with interrupts disabled on this core — tens of
//! milliseconds typically, with a datasheet worst case in the hundreds. That is
//! not new (`crate::store` says the same about a rolling-code commit and
//! `crate::rpc` budgets for it), but an update does it a few hundred times in a
//! row rather than once. The consequences, stated rather than discovered:
//!
//! - **Nothing is corrupted.** Edges are timestamped by the RMT peripheral into
//!   its own RAM without software, and a burst already queued is clocked out of
//!   that RAM the same way, so neither path depends on a poll arriving on time.
//! - **A reception that overlaps an erase can be lost, and this is the honest
//!   part.** The RMT receive RAM is finite and the interrupt that drains it
//!   cannot run while the erase has interrupts masked, so a wall-remote press
//!   during an update may be truncated and never decoded. What that costs is a
//!   position estimate that misses one movement, which the next travel to a
//!   hard limit corrects — and the board is about to reboot in any case.
//! - **A position estimate can tick late**, by up to one erase. The arrival
//!   stop is already planned a start-lag early (`somfy_domain::Motion`), which
//!   is hundreds of milliseconds, so this is inside a margin that already
//!   exists.
//!
//! Shutting the radio down for the duration would remove the second bullet
//! entirely, and it is what a deployed controller of this kind does. It is not
//! done here because the cost it removes is one missed overhearing on a device
//! that is seconds from restarting, and the cost it adds is a controller that
//! cannot be commanded while it updates. `docs/provenance.md` records the
//! comparison rather than leaving it to be rediscovered.
//!
//! # What only real hardware can confirm
//!
//! That an image written this way boots. Everything below can be reasoned
//! about, and the one thing that decides it — the ESP-IDF bootloader reading
//! `otadata` and mapping a sequence number to a slot — runs before any of this
//! code exists. `docs/hardware-checklist.md` carries the procedure.

use core::cell::Cell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use esp_bootloader_esp_idf::ota::{Ota, OtaImageState};
use esp_bootloader_esp_idf::partitions::{
    self, AppPartitionSubType, DataPartitionSubType, PartitionType,
};
use esp_storage::FlashStorage;
use somfy_ota::selftest::{LegState, SelfTest, SelfTestOutcome, WINDOW_MS};
use somfy_ota::verdict::{boot_verdict, BootVerdict, ImageState, RollBackReason};

use crate::store::FlashStore;

/// How much of the start of flash a partition table can occupy.
///
/// The same figure `crate::store` uses, restated rather than shared for the
/// same reason it is restated in `config`, `shades` and `estate`: each module
/// reads the table for itself, and a shared constant would be one import that
/// says nothing.
const PARTITION_TABLE_BYTES: usize = 1024;

/// Where the two app slots are, resolved once at boot.
///
/// Held as plain numbers rather than as `esp_bootloader_esp_idf` partition
/// entries because those borrow the buffer the table was read into, and that
/// buffer is a kilobyte on the boot stack that must not outlive the read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slots {
    /// The slot this image is executing from.
    pub booted: AppPartitionSubType,
    /// The other one, which an update writes.
    pub target: AppPartitionSubType,
    /// Where the target starts in flash.
    pub target_at: u32,
    /// How long the target is. Equal to the booted slot's length by
    /// construction — `crates/firmware/build.rs` fails the build otherwise —
    /// because an image that fits one slot and not the other is a brick on the
    /// following update.
    pub target_len: u32,
}

/// Why this module could not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaError {
    /// The partition table could not be read.
    PartitionTable(partitions::Error),
    /// There is no `otadata` region. A board flashed with espflash's built-in
    /// table rather than this crate's reaches this, and it means no update can
    /// be activated — which is loud rather than silent, and recoverable by
    /// reflashing from `crates/firmware`.
    NoOtaData,
    /// This image is not running from an OTA slot at all. Unreachable with the
    /// table in `partitions.csv`, which has no `factory` partition; it is here
    /// because an image that cannot identify its own slot must not guess at
    /// which one to overwrite.
    NotInASlot,
    /// The other slot is not in the table.
    NoTargetSlot,
    /// A flash read, write or erase failed.
    Flash,
    /// A write did not read back as what was written. The same check, and the
    /// same name, as `somfy_store::StoreError::NotDurable`.
    ///
    /// Only an upload writes an app slot, so only a build that can receive
    /// one can produce this.
    #[cfg(feature = "http")]
    NotDurable {
        /// Where in the target slot.
        at: u32,
    },
}

// ---------------------------------------------------------------------------
// Boot side
// ---------------------------------------------------------------------------

/// How many times this image has started since the last power-on.
///
/// **In RTC-fast memory, which esp-hal zeroes on a power-on reset and preserves
/// across a software reset** (`esp_hal::soc::__init_persistent` compares
/// `reset_reason()` against `ChipPowerOn`). That is precisely the discriminator
/// [`somfy_ota::verdict`] needs and the `otadata` state field cannot be: it
/// separates "an update was just activated" from "this image already had its
/// go and did not finish", without depending on whether the bootloader on this
/// board was built with rollback enabled.
///
/// A power cut therefore *resets the attempt*, which is deliberate and better
/// than ESP-IDF's own behaviour — a blip is not evidence against a release.
///
/// **It can hold garbage, and that is survivable rather than guarded against.**
/// esp-hal's own documentation for `#[ram(persistent)]` says a system-level or
/// lesser reset landing before the zeroing "could skip initialization and start
/// the application with the static filled with random bytes". A garbage count is
/// almost certainly non-zero, which reads as "not the first attempt" and rolls
/// back — the safe direction, and the reason a checksum is not carried here.
/// Every read of it is `saturating`, so it cannot also be an arithmetic panic.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut ATTEMPTS: u32 = 0;

/// Read the attempt count and record that this boot is one.
///
/// `unsafe` because it is a `static mut`, and sound because of *when* it runs:
/// [`survey`] is called from `crate::start`, on the only core this firmware
/// brings up, before any task exists. There is no second reader and no
/// interrupt that touches it.
fn take_attempt() -> u32 {
    unsafe {
        let so_far = core::ptr::read_volatile(&raw const ATTEMPTS);
        core::ptr::write_volatile(&raw mut ATTEMPTS, so_far.saturating_add(1));
        so_far
    }
}

/// Forget that this image was ever unconfirmed.
///
/// Called once the slot has been marked valid, so that a later reset — months
/// later, for an unrelated reason — does not read a stale count as a failed
/// verification. Sound for a different reason from [`take_attempt`]: the value
/// is a single aligned word, this is the only writer after boot, and the only
/// reader runs before any task exists.
fn clear_attempts() {
    unsafe {
        core::ptr::write_volatile(&raw mut ATTEMPTS, 0);
    }
}

/// Where the slots are, once somebody has looked.
static SLOTS: BlockingMutex<CriticalSectionRawMutex, Cell<Option<Slots>>> =
    BlockingMutex::new(Cell::new(None));

/// Whether this boot owes a verdict, and what the self-test has found so far.
static PENDING: BlockingMutex<CriticalSectionRawMutex, Cell<bool>> =
    BlockingMutex::new(Cell::new(false));

/// The self-test's legs.
static LEGS: BlockingMutex<CriticalSectionRawMutex, Cell<SelfTest>> =
    BlockingMutex::new(Cell::new(SelfTest::new()));

/// What a boot found out about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boot {
    /// Where the two slots are.
    pub slots: Slots,
    /// What to do about the state this image booted with.
    pub verdict: BootVerdict,
    /// How many times this image had already started since the last power-on.
    pub attempts_before: u32,
    /// The state `otadata` carried.
    pub state: ImageState,
}

/// Read the partition table and `otadata`, and decide.
///
/// Called from `crate::start` immediately after the rolling-code store mounts,
/// which is the earliest moment a `FlashStorage` exists — and the right one,
/// because a roll-back should happen before the radio is brought up rather
/// than after.
///
/// Returns `Err` for every reason a board might have no update machinery at
/// all. **None of them is fatal**: a board with espflash's built-in table has
/// no `otadata`, and it still receives, decodes and tracks.
pub fn survey(flash: &mut FlashStorage<'_>) -> Result<Boot, OtaError> {
    let slots = read_slots(flash)?;
    SLOTS.lock(|cell| cell.set(Some(slots)));
    let state = read_state(flash, slots)?;
    let attempts_before = if matches!(
        state,
        ImageState::New | ImageState::PendingVerify | ImageState::Invalid | ImageState::Aborted
    ) {
        take_attempt()
    } else {
        // A settled image is not an attempt at anything and must not consume
        // one: a board that panics for an unrelated reason would otherwise walk
        // a counter a later update would read.
        //
        // **`Invalid` and `Aborted` are on the other side of that line**, and
        // they have to be. Each of them means a roll-back was decided and did
        // not take effect, so each of them is a *retry* — and a retry nothing
        // counts is a reset loop. See `somfy_ota::verdict`.
        0
    };
    Ok(Boot {
        slots,
        verdict: boot_verdict(state, attempts_before),
        attempts_before,
        state,
    })
}

/// Which slot is running and which one an update writes.
fn read_slots(flash: &mut FlashStorage<'_>) -> Result<Slots, OtaError> {
    let mut buffer = [0u8; PARTITION_TABLE_BYTES];
    let table =
        partitions::read_partition_table(flash, &mut buffer).map_err(OtaError::PartitionTable)?;

    if table
        .find_partition(PartitionType::Data(DataPartitionSubType::Ota))
        .map_err(OtaError::PartitionTable)?
        .is_none()
    {
        return Err(OtaError::NoOtaData);
    }

    let booted = table
        .booted_partition()
        .map_err(OtaError::PartitionTable)?
        .ok_or(OtaError::NotInASlot)?;
    let booted = match booted.partition_type() {
        PartitionType::App(AppPartitionSubType::Ota0) => AppPartitionSubType::Ota0,
        PartitionType::App(AppPartitionSubType::Ota1) => AppPartitionSubType::Ota1,
        _ => return Err(OtaError::NotInASlot),
    };
    let target = match booted {
        AppPartitionSubType::Ota0 => AppPartitionSubType::Ota1,
        _ => AppPartitionSubType::Ota0,
    };
    let entry = table
        .find_partition(PartitionType::App(target))
        .map_err(OtaError::PartitionTable)?
        .ok_or(OtaError::NoTargetSlot)?;

    Ok(Slots {
        booted,
        target,
        target_at: entry.offset(),
        target_len: entry.len(),
    })
}

/// What `otadata` says about the slot that is running.
///
/// **A blank region is [`ImageState::Absent`] rather than an error**, which is
/// what every freshly flashed board has: `esp_bootloader_esp_idf` reports
/// `InvalidState` when both sequence numbers are `0xFFFFFFFF`, and that is a
/// fact about the region rather than a failure to read it.
///
/// **A selection that is not the booted slot is repaired here.** It means the
/// bootloader refused the image `otadata` chose — a corrupt slot, which it
/// verifies before jumping — and fell back to this one. Leaving the record
/// pointing at the bad slot would make the next boot try it again; pointing it
/// at the slot that actually runs makes the fallback stick.
fn read_state(flash: &mut FlashStorage<'_>, slots: Slots) -> Result<ImageState, OtaError> {
    with_ota(flash, |ota| {
        let state = match ota.current_ota_state() {
            Ok(state) => state,
            // The one error that is a state rather than a fault.
            Err(partitions::Error::InvalidState) => return Ok(ImageState::Absent),
            Err(_) => return Err(OtaError::Flash),
        };
        let selected = ota.current_app_partition().map_err(|_| OtaError::Flash)?;
        if selected != slots.booted {
            crate::logln!(
                "ota: otadata selects {:?} and this image is running from {:?} — the bootloader \
                 fell back, so the record is being repaired to match what actually boots",
                selected,
                slots.booted,
            );
            ota.set_current_app_partition(slots.booted)
                .map_err(|_| OtaError::Flash)?;
            // The repaired entry carries whatever state the slot it now selects
            // had; re-read rather than assume.
            return match ota.current_ota_state() {
                Ok(state) => Ok(state_of(state)),
                Err(partitions::Error::InvalidState) => Ok(ImageState::Absent),
                Err(_) => Err(OtaError::Flash),
            };
        }
        Ok(state_of(state))
    })
}

/// `esp-bootloader-esp-idf`'s state, as the one [`somfy_ota`] reasons about.
///
/// A translation rather than a re-export, because the two are not the same
/// list: [`ImageState::Absent`] has no counterpart there, and keeping the
/// decision table free of a device crate is what lets it be tested on a host.
const fn state_of(state: OtaImageState) -> ImageState {
    match state {
        OtaImageState::New => ImageState::New,
        OtaImageState::PendingVerify => ImageState::PendingVerify,
        OtaImageState::Valid => ImageState::Valid,
        OtaImageState::Invalid => ImageState::Invalid,
        OtaImageState::Aborted => ImageState::Aborted,
        OtaImageState::Undefined => ImageState::Undefined,
    }
}

/// Run something against the `otadata` region.
///
/// The region is found afresh each time. That looks wasteful and is not: it
/// happens at most three times in a board's boot and once per update, and the
/// alternative is holding a `PartitionEntry` that borrows a kilobyte of stack
/// for the life of the program.
fn with_ota<T>(
    flash: &mut FlashStorage<'_>,
    f: impl FnOnce(&mut Ota<'_, FlashStorage<'_>>) -> Result<T, OtaError>,
) -> Result<T, OtaError> {
    let mut buffer = [0u8; PARTITION_TABLE_BYTES];
    let table =
        partitions::read_partition_table(flash, &mut buffer).map_err(OtaError::PartitionTable)?;
    let entry = table
        .find_partition(PartitionType::Data(DataPartitionSubType::Ota))
        .map_err(OtaError::PartitionTable)?
        .ok_or(OtaError::NoOtaData)?;
    let region = entry.as_embedded_storage(flash);
    // Two slots, and it is a constant rather than a count because
    // `crates/firmware/build.rs` already refuses a `partitions.csv` that does
    // not define exactly `ota_0` and `ota_1` — counting here would be a second
    // reading of a fact the build has already settled, and one that could
    // disagree with it.
    let mut ota = Ota::new(region, 2).map_err(|_| OtaError::NoOtaData)?;
    f(&mut ota)
}

/// Say what a boot found, and act on it if the answer is "go back".
///
/// Returns `true` when a self-test is owed, which is what `crate::start` uses
/// to decide whether to arm one. Diverges — resets the board — when the verdict
/// is a roll-back, because there is nothing this image should go on to do.
pub fn report(flash: &mut FlashStorage<'_>, boot: Boot) -> bool {
    crate::logln!(
        "ota: running from {:?} (updates would write {:?} at {:#010X}, {} bytes), \
         otadata says {:?}",
        boot.slots.booted,
        boot.slots.target,
        boot.slots.target_at,
        boot.slots.target_len,
        boot.state,
    );
    match boot.verdict {
        BootVerdict::Settled => {
            if matches!(boot.state, ImageState::Invalid | ImageState::Aborted) {
                crate::logln!(
                    "ota: this image's own record says {:?} and the switch away from it has \
                     already been tried once since the last power-on — running it anyway \
                     rather than resetting again. Reach it over the network and upload a \
                     known-good image; a power cycle re-arms one more roll-back attempt.",
                    boot.state,
                );
            }
            false
        }
        BootVerdict::Verify => {
            crate::logln!(
                "ota: this image has not been confirmed — running the self-test, and marking \
                 the slot valid in {} s if the radio, the stores and the network bring-up all \
                 answer. A crash before then rolls back on the next boot.",
                WINDOW_MS / 1_000,
            );
            PENDING.lock(|cell| cell.set(true));
            true
        }
        BootVerdict::RollBack(reason) => {
            crate::logln!(
                "ota: rolling back to {:?} — {}. This image started {} time(s) since the last \
                 power-on without confirming itself.",
                boot.slots.target,
                describe(reason),
                // `saturating_add`, because this value comes out of a region
                // esp-hal's own documentation says a reset landing before the
                // zeroing can leave "filled with random bytes". A garbage count
                // reads as "not the first attempt" and rolls back, which is the
                // safe direction; it must not also be an arithmetic panic in the
                // line that explains why.
                boot.attempts_before.saturating_add(1),
            );
            match switch_back(flash, boot.slots) {
                Ok(()) => crate::logln!("ota: otadata now selects {:?}", boot.slots.target),
                Err(error) => crate::logln!(
                    "ota: could not write otadata ({:?}) — resetting anyway, which will land \
                     back here and try once more before settling",
                    error,
                ),
            }
            // **Deliberately not cleared here.** The count is what bounds this
            // path to one switch per power-on; clearing it would make a
            // roll-back that cannot take effect reset the board forever. It is
            // cleared where a genuinely new attempt begins — an upload, or a
            // confirmation. See `somfy_ota::verdict::boot_verdict`.
            crate::drain_serial();
            esp_hal::system::software_reset()
        }
    }
}

/// One sentence per reason, for the console.
const fn describe(reason: RollBackReason) -> &'static str {
    match reason {
        RollBackReason::AttemptExhausted => {
            "it did not finish its self-test on a previous boot, which means it crashed or was \
             reset part-way through"
        }
        RollBackReason::MarkedInvalid => {
            "a self-test already refused it and the switch back did not take effect"
        }
        RollBackReason::MarkedAborted => "the bootloader gave up on it",
    }
}

/// Mark the running slot bad and point `otadata` at the other one.
///
/// **Both halves, and the order matters.** `Invalid` alone does nothing on a
/// bootloader built without rollback — which is the default and which is
/// probably what `espflash` ships — so the switch is what actually changes
/// which image runs. `Invalid` is written first so that a power cut between the
/// two leaves a record that says the slot is bad, which the next boot reads as
/// [`RollBackReason::MarkedInvalid`] and retries.
fn switch_back(flash: &mut FlashStorage<'_>, slots: Slots) -> Result<(), OtaError> {
    with_ota(flash, |ota| {
        ota.set_current_ota_state(OtaImageState::Invalid)
            .map_err(|_| OtaError::Flash)?;
        ota.set_current_app_partition(slots.target)
            .map_err(|_| OtaError::Flash)
    })
}

// ---------------------------------------------------------------------------
// The self-test
// ---------------------------------------------------------------------------

/// Record how the radio's control path answered.
///
/// See [`somfy_ota::selftest::Leg::Radio`] for exactly how little this
/// establishes.
pub fn radio_leg(passed: bool) {
    #[cfg(feature = "bad-image-selftest")]
    let passed = {
        let _ = passed;
        crate::logln!(
            "ota: THIS IMAGE IS DELIBERATELY BROKEN — built with `bad-image-selftest`, which \
             reports the radio leg as failed however the radio answered. If it arrived over the \
             air it should roll back within {} s. Never flash it as a keeper.",
            WINDOW_MS / 1_000,
        );
        false
    };
    set_leg(|legs| legs.radio = pass(passed));
}

/// How long a `bad-image-panic` build runs before falling over.
///
/// **Twenty seconds**, and the figure is chosen against the two things it has
/// to sit between rather than picked: long enough that the network has come up
/// and the board looks healthy — which is the whole point, since an image that
/// died instantly would be caught by the bootloader rather than by this — and
/// well short of [`WINDOW_MS`], so the soak has not concluded and the image is
/// still unconfirmed when the panic lands.
#[cfg(feature = "bad-image-panic")]
const SABOTAGE_PANIC_MS: u64 = 20_000;

/// Record whether the flash regions mounted and read back.
pub fn stores_leg(passed: bool) {
    set_leg(|legs| legs.stores = pass(passed));
}

/// Record whether the Wi-Fi driver and the network stack started.
///
/// `None` for a board with no credentials, which has no network to bring up and
/// must still be able to accept an update.
pub fn network_leg(passed: Option<bool>) {
    set_leg(|legs| {
        legs.network = match passed {
            Some(passed) => pass(passed),
            None => LegState::Skipped,
        }
    });
}

/// Record that the station reached a configured address.
///
/// Reported, never a trigger — see [`somfy_ota::selftest`]. Called from
/// `crate::net`'s address watcher, which is the one place that already knows.
pub fn associated() {
    set_leg(|legs| legs.associated = true);
}

const fn pass(passed: bool) -> LegState {
    if passed {
        LegState::Passed
    } else {
        LegState::Failed
    }
}

fn set_leg(f: impl FnOnce(&mut SelfTest)) {
    LEGS.lock(|cell| {
        let mut legs = cell.get();
        f(&mut legs);
        cell.set(legs);
    });
}

/// Whether this boot owes a verdict.
pub fn verification_pending() -> bool {
    PENDING.lock(Cell::get)
}

/// When the soak started, in milliseconds of uptime.
///
/// Set on the first tick after [`report`] said a verdict is owed. The state
/// task's own clock rather than one of ours, because that task ticks anyway and
/// a second clock would be a second thing to keep true.
static SOAK_FROM: BlockingMutex<CriticalSectionRawMutex, Cell<Option<u64>>> =
    BlockingMutex::new(Cell::new(None));

/// What one look at the self-test concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do: this boot owes no verdict, or it has already given one.
    Idle,
    /// Still soaking.
    Waiting,
    /// The image was marked valid.
    Confirmed,
    /// The image was refused and `otadata` now selects the other slot. **The
    /// caller must reset.**
    RollBack,
}

/// Look at the self-test, and act if it has concluded.
///
/// **Driven by the state task's existing ticker rather than by a task of its
/// own**, and that is a measurement rather than a preference: an
/// `#[embassy_executor::task]` allocates its future as a `static` whether or
/// not it is ever spawned, and the one this replaced measured **864 bytes** —
/// almost all of it the [`crate::rpc::Request`] its confirm call held. Those
/// bytes come out of [`crate::heap::DRAM_FOR_STACK_AND_HEAP`], and so out of
/// the Wi-Fi driver's heap, on every boot of every board including the ones
/// that will never take an update. Here the whole thing is a synchronous call
/// from an arm that already exists: it costs the executor's stack for the
/// length of the call and nothing at all when it is not running.
///
/// It also removes a round trip. The state task owns the flash, so it can write
/// `otadata` itself instead of asking itself to through [`crate::rpc`].
///
/// The tick is every `somfy_tasks::TICK_MS`, which is far finer than a
/// ninety-second window needs — that is the ticker's business rather than this
/// function's, and the arithmetic below is over the timestamp rather than over
/// the number of calls, so the two are independent.
pub fn tick_self_test(store: &mut FlashStore<'static>, now_ms: u64) -> Step {
    if !verification_pending() {
        return Step::Idle;
    }
    let from = match SOAK_FROM.lock(Cell::get) {
        Some(from) => from,
        None => {
            SOAK_FROM.lock(|cell| cell.set(Some(now_ms)));
            now_ms
        }
    };
    let elapsed = now_ms.saturating_sub(from);
    // The other half of the deliberately-broken pair, and the harder one: this
    // image never reaches a verdict at all. The panic handler resets, the
    // attempt counter in RTC memory survives that reset, and the *next* boot
    // rolls back. Nothing in the ordinary code path can produce this.
    #[cfg(feature = "bad-image-panic")]
    if elapsed >= SABOTAGE_PANIC_MS {
        panic!(
            "bad-image-panic: this image was built to fall over {} s into its self-test, so \
             that the roll-back on the next boot can be observed rather than argued about",
            SABOTAGE_PANIC_MS / 1_000,
        );
    }
    let legs = LEGS.lock(Cell::get);
    match legs.poll(elapsed) {
        SelfTestOutcome::Waiting => Step::Waiting,
        SelfTestOutcome::Pass { associated } => {
            crate::logln!(
                "ota: self-test passed after {} s — radio SPI answered, the stores read back, \
                 the network {}. Marking this image valid. It does NOT establish that anything \
                 radiates: see somfy_ota::selftest::Leg::Radio.",
                elapsed / 1_000,
                if associated {
                    "came up"
                } else {
                    "did not come up, which is not a reason to refuse a release"
                },
            );
            PENDING.lock(|cell| cell.set(false));
            match confirm(store) {
                Ok(()) => crate::logln!("ota: this image is now the one a reset boots"),
                // Left unconfirmed on purpose: the next reset reads that as a
                // verification that never finished and rolls back, which is the
                // safe direction to fail in.
                Err(error) => crate::logln!(
                    "ota: could not mark this image valid ({:?}) — it stays unconfirmed, and the \
                     next reset will roll it back",
                    error,
                ),
            }
            Step::Confirmed
        }
        SelfTestOutcome::Fail(leg) => {
            crate::logln!(
                "ota: self-test FAILED on {:?} after {} s — rolling back to the image that was \
                 running before this update",
                leg,
                elapsed / 1_000,
            );
            PENDING.lock(|cell| cell.set(false));
            if let Err(error) = roll_back(store) {
                crate::logln!(
                    "ota: could not switch back ({:?}) — resetting anyway, which lands back here \
                     and tries again",
                    error,
                );
            }
            Step::RollBack
        }
    }
}

/// Mark the running slot valid. Called by the state task.
pub fn confirm(store: &mut FlashStore<'static>) -> Result<(), OtaError> {
    let outcome = store.with_flash(|flash| {
        with_ota(flash, |ota| {
            ota.set_current_ota_state(OtaImageState::Valid)
                .map_err(|_| OtaError::Flash)
        })
    });
    if outcome.is_ok() {
        clear_attempts();
    }
    outcome
}

/// Mark the running slot bad and switch back. Called by the state task; the
/// caller resets.
pub fn roll_back(store: &mut FlashStore<'static>) -> Result<(), OtaError> {
    let Some(slots) = SLOTS.lock(Cell::get) else {
        return Err(OtaError::NotInASlot);
    };
    // Not cleared, for the same reason `report` does not — see there.
    store.with_flash(|flash| switch_back(flash, slots))
}

// ---------------------------------------------------------------------------
// The upload session
//
// `http` only, and the gate is worth stating: everything above is the *safety*
// half — it decides whether the image that is running may stay — and it costs
// no buffers, so it is built for every chip that has an `otadata` region.
// Below is the machinery for *receiving* an update, which needs a web server
// and a page buffer, and the ESP32 has neither.
// ---------------------------------------------------------------------------

#[cfg(feature = "http")]
mod upload;

#[cfg(feature = "http")]
pub use upload::{abort, begin, finish, init, page, take, with_page, Pages, Upload, PAGE_BYTES};
