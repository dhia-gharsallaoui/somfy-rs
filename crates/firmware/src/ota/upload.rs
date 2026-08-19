//! Receiving an update: the page channel, the session, and the writes.
//!
//! The megabyte crosses from an HTTP connection task to the state task through
//! [`embassy_sync::zerocopy_channel`], which lends a `&mut Page` out of a
//! `static` rather than copying — see [`super`] for why that mattered enough to
//! shape the module.
//!
//! **Nothing here decides anything about the image.** Whether the bytes are a
//! firmware image, for this chip, complete and internally consistent is
//! `somfy_ota::image`, on the host side of the fence with tests. What is left
//! here is erase, write, read back, and one thirty-two byte record.

use core::cell::{Cell, RefCell};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::zerocopy_channel::{Channel, Receiver, Sender};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::ota::OtaImageState;
use esp_bootloader_esp_idf::partitions::{AppPartitionSubType, Error as PartitionError};
use esp_storage::FlashStorage;
use somfy_api::ApiErrorCode;
use somfy_ota::image::{Chip, ImageError, Verifier};
use somfy_ota::selftest::WINDOW_MS;
use static_cell::StaticCell;

use super::{with_ota, OtaError, Slots, SLOTS};
use crate::store::FlashStore;
use crate::tasks::Mutex;

/// The flash erase unit.
pub const SECTOR_BYTES: usize = <FlashStorage as NorFlash>::ERASE_SIZE;

/// The flash write granularity.
const WORD_BYTES: usize = <FlashStorage as NorFlash>::WRITE_SIZE;

/// How many bytes cross from the web server to the state task at a time.
///
/// **256, which is the SPI NOR page-program unit.** It is the largest write
/// that is a single program operation on the part — a longer one is split at
/// the page boundary anyway — and the smallest that wastes none of one. It
/// divides [`SECTOR_BYTES`] exactly, so a page never straddles an erase unit
/// and the "erase when this page starts a sector" rule below is complete. And
/// it is a multiple of [`WORD_BYTES`], which is what `NorFlash::write` requires
/// of every length it is given.
///
/// **The reason it is not larger is DRAM, and the figure is small.** The buffer
/// is a `static`, so it comes out of the DRAM `crate::heap::heap_region` divides and
/// therefore out of the Wi-Fi driver's heap on the chip with the least of it.
/// The next size up that divides a sector is 512, which used to cost a whole
/// KiB on the ESP32-C3 — the heap was rounded down to a whole kilobyte then, and
/// that chip's margin was a few hundred bytes. Neither is true now (the chip
/// went on 2026-08-19 and the rounding with the constant), so 512 would cost
/// exactly 256 more bytes on an ESP32-S3 with room. It is left at 256 because
/// raising it buys nothing measurable and would cost a re-measurement over the worst announcement
/// peak ever measured.
///
/// What it costs in time is nothing that matters: a 1.1 MB image is about 4,300
/// pages, each one an executor round trip through [`crate::rpc`], against a
/// flash side that spends tens of milliseconds per sector erase. The erases are
/// three orders of magnitude more expensive than the round trips.
pub const PAGE_BYTES: usize = 256;

const _: () = assert!(
    SECTOR_BYTES.is_multiple_of(PAGE_BYTES),
    "a page must not straddle an erase unit, or the erase-on-sector-start rule \
     below would leave part of a sector unerased under a write",
);
const _: () = assert!(
    PAGE_BYTES.is_multiple_of(WORD_BYTES),
    "NorFlash::write takes only word-aligned lengths",
);

/// One page in flight between the web server and the state task.
///
/// Lives in a `static` and is lent out by [`zerocopy_channel`]; see the module
/// docs for why it is not a local.
///
/// [`zerocopy_channel`]: embassy_sync::zerocopy_channel
pub struct Page {
    /// The bytes. **How many of them are the image's is carried by the request
    /// that asks for them to be written**, not by a field here — one authority
    /// rather than two that could disagree about the last page.
    pub bytes: [u8; PAGE_BYTES],
}

impl Page {
    const fn new() -> Page {
        Page {
            bytes: [0; PAGE_BYTES],
        }
    }
}

/// Which chip this image runs on, and therefore which `chip_id` an uploaded
/// image has to carry.
///
/// Both values were read off real images built from this repository rather
/// than out of a header file; `somfy_ota::image::Chip` holds them and
/// `docs/provenance.md` records the images. The `compile_error!` below is the
/// part that matters: a second chip added to the build matrix without a
/// `chip_id` here would otherwise be a board that accepts any image at all.
#[cfg(feature = "chip-s3")]
const THIS_CHIP: Chip = Chip::Esp32S3;
#[cfg(not(feature = "chip-s3"))]
compile_error!(
    "an over-the-air update refuses an image built for a different chip, and that check needs \
     this chip's `chip_id`. Add one to `somfy_ota::image::Chip` and name it here."
);

/// One page buffer, once.
///
/// A channel of exactly one buffer is a rendezvous: the web server fills it and
/// waits, the state task drains it and hands it back. A second buffer would let
/// the socket read overlap the flash write and roughly halve an update's
/// wall-clock time — and would cost another [`PAGE_BYTES`] of the DRAM the
/// Wi-Fi driver's heap is carved from, for a transfer nobody has called slow. See [`PAGE_BYTES`].
static PAGES: StaticCell<[Page; 1]> = StaticCell::new();

/// The channel over that buffer.
static CHANNEL: StaticCell<Channel<'static, Mutex, Page>> = StaticCell::new();

/// The sending end, and **the session lock**.
///
/// Taking it is what makes a second concurrent upload impossible: there is one,
/// it is moved out for the life of a session, and a request that finds nothing
/// here is answered [`ApiErrorCode::UpdateInProgress`]. That is a stronger
/// guarantee than a flag beside a buffer, because the thing that is exclusive
/// and the thing that grants access are the same object — a handler cannot
/// forget to check.
static SENDER: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<Option<Sender<'static, Mutex, Page>>>,
> = BlockingMutex::new(RefCell::new(None));

/// Whether [`init`] has run.
static STARTED: BlockingMutex<CriticalSectionRawMutex, Cell<bool>> =
    BlockingMutex::new(Cell::new(false));

/// Build the page channel and hand the state task its end.
///
/// Called once from `crate::start`. A second call returns `None` rather than
/// panicking: a panic here is a boot loop over a programming mistake, and the
/// caller has somewhere sensible to put the answer.
pub fn init() -> Option<Pages> {
    if STARTED.lock(Cell::get) {
        return None;
    }
    STARTED.lock(|cell| cell.set(true));
    let buffers = PAGES.init([Page::new()]);
    let channel = CHANNEL.init(Channel::new(buffers));
    let (sender, receiver) = channel.split();
    SENDER.lock(|cell| *cell.borrow_mut() = Some(sender));
    Some(Pages {
        rx: receiver,
        session: None,
    })
}

/// The state task's end of an upload: the page channel and the session.
///
/// Carried in `crate::tasks::Table` rather than in a `static`, on purpose. A
/// `static` would be another [`PAGE_BYTES`]-scale object in the DRAM the Wi-Fi
/// heap comes out of; here it is stack inside the state task's future, which is
/// the resource with room. `crate::heap` prices both.
pub struct Pages {
    rx: Receiver<'static, Mutex, Page>,
    session: Option<Session>,
}

/// An upload in progress, as the state task sees it.
struct Session {
    /// Checks the bytes as they go past.
    verifier: Verifier,
    /// Where the slots are, captured at [`begin`] so the walk does not re-read
    /// the partition table once per page.
    slots: Slots,
    /// How many bytes of the image have been written to the target slot.
    written: u32,
}

/// The web server's end: the right to send pages, held for one upload.
///
/// Dropping it releases the lock — including on the path where a connection
/// task's future is dropped because its socket died. The *state task's* half of
/// an abandoned session is cleared by the next [`begin`], which is the only
/// thing that can see it.
pub struct Upload {
    sender: Option<Sender<'static, Mutex, Page>>,
}

impl Upload {
    /// Lend the page buffer, for the caller to fill.
    ///
    /// Only a pointer is live across the awaits this returns into, which is the
    /// whole reason the channel exists rather than a per-connection buffer.
    /// `None` is unreachable — an `Upload` holds its sender for its whole life
    /// and only `Drop` takes it back — and is answered rather than `expect`ed
    /// for the reason `crate::rpc` gives about its own unreachable arm: a panic
    /// on a request path reboots the board over one request.
    pub async fn lend(&mut self) -> Option<&mut Page> {
        Some(self.sender.as_mut()?.send().await)
    }

    /// Hand the filled page to the state task.
    pub fn post(&mut self) {
        if let Some(sender) = self.sender.as_mut() {
            sender.send_done();
        }
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            SENDER.lock(|cell| *cell.borrow_mut() = Some(sender));
        }
    }
}

/// Claim the right to upload, or find that somebody already has.
pub fn take() -> Option<Upload> {
    SENDER
        .lock(|cell| cell.borrow_mut().take())
        .map(|sender| Upload {
            sender: Some(sender),
        })
}

/// Start writing the target slot. Runs on the state task.
///
/// `declared` is the request's `Content-Length`, which is the only thing that
/// can settle "does it fit" before a byte has been read — and settling it here
/// is what keeps a too-large image from erasing a sector on its way to being
/// refused.
pub fn begin(
    pages: &mut Pages,
    store: &mut FlashStore<'static>,
    declared: u32,
) -> Result<(), ApiErrorCode> {
    // An abandoned session — a connection task whose socket died mid-upload —
    // leaves a stale session and possibly a page in flight. Both are cleared
    // here, which is the only place that can see either.
    if pages.session.is_some() {
        crate::logln!(
            "ota: a previous upload was abandoned part-way; its half-written slot is being \
             overwritten, and it was never marked bootable",
        );
    }
    pages.session = None;
    pages.rx.clear();

    let Some(slots) = SLOTS.lock(Cell::get) else {
        crate::logln!(
            "ota: this board has no otadata region, so an update could never be activated. \
             Reflash from crates/firmware so espflash writes this crate's partition table."
        );
        return Err(ApiErrorCode::UpdateUnwritable);
    };
    let verifier =
        Verifier::new(THIS_CHIP, declared as usize, slots.target_len as usize).map_err(refuse)?;

    store
        .with_flash(|flash| seed_otadata(flash, slots))
        .map_err(|error| {
            crate::logln!("ota: otadata could not be prepared ({:?})", error);
            ApiErrorCode::UpdateUnwritable
        })?;

    // **The one place a new attempt begins, and so the one place the count is
    // cleared.** Whatever this board did earlier in this power cycle — a
    // roll-back that did not take, a crash, a soak that never finished — the
    // image about to be written gets its own full attempt. See
    // `somfy_ota::verdict::boot_verdict`.
    super::clear_attempts();
    crate::logln!(
        "ota: accepting {} bytes for {:?} at {:#010X} — the running slot is not touched",
        declared,
        slots.target,
        slots.target_at,
    );
    pages.session = Some(Session {
        verifier,
        slots,
        written: 0,
    });
    Ok(())
}

/// Make `otadata` describe the slot that is running, if it describes nothing.
///
/// **This is the first-update trap, closed.** With both sequence numbers blank,
/// `esp_bootloader_esp_idf` reports the current partition as `Factory` whether
/// or not a factory partition exists, and asking it to select the *second* slot
/// from there computes an increment of zero and writes sequence number **0** —
/// which underflows its own reader (`0u32 - 1`) and which ESP-IDF treats as
/// nothing selected. Writing the booted slot first avoids it: from `Factory` to
/// `Ota0` the increment is one, the sequence becomes 1, and every later switch
/// is an ordinary increment of a non-zero number.
///
/// It happens **here rather than at boot** so that a board which never takes an
/// update never writes this region at all — which is what keeps a plain
/// `espflash flash` working with no `--erase-parts` ceremony until the day the
/// board has actually taken one. `docs/hardware-checklist.md` carries the other
/// half of that.
fn seed_otadata(flash: &mut FlashStorage<'_>, slots: Slots) -> Result<(), OtaError> {
    with_ota(flash, |ota| {
        let blank = matches!(ota.current_ota_state(), Err(PartitionError::InvalidState));
        if !blank {
            return Ok(());
        }
        if slots.booted != AppPartitionSubType::Ota0 {
            // Unreachable: a blank `otadata` with no factory partition is
            // exactly what the bootloader reads as "boot ota_0". Refused rather
            // than worked around, because the workaround would be the
            // sequence-zero write this function exists to avoid.
            crate::logln!(
                "ota: otadata is blank but this image is running from {:?}, which cannot happen \
                 with this partition table — refusing rather than writing a sequence number \
                 that would select nothing",
                slots.booted,
            );
            return Err(OtaError::NotInASlot);
        }
        crate::logln!(
            "ota: otadata is blank — seeding it to {:?}, the slot this image runs from. From \
             here a serial reflash needs `--erase-parts otadata`; see \
             docs/hardware-checklist.md.",
            slots.booted,
        );
        ota.set_current_app_partition(slots.booted)
            .map_err(|_| OtaError::Flash)
    })
}

/// Verify and write one page. Runs on the state task.
///
/// **Verification comes first and the write is conditional on it**, which is
/// what makes the first page's refusals — not a firmware image, built for
/// another chip, no application descriptor — cost the target slot nothing at
/// all.
/// Hand the lent page to `f`, then give the buffer back.
///
/// **The one thing in this module that is not about firmware.** A configuration
/// restore is also an upload, and it crosses from the web server to this task
/// the same way a firmware image does — so it borrows this channel rather than
/// declaring a second one, which would be a second page buffer out of the DRAM
/// `crate::heap` carves the Wi-Fi driver's heap from. See `crate::restore`.
///
/// `None` is a page that was asked for and never sent. Unreachable — the web
/// server posts a page and only then asks for it to be written, and the request
/// gate serialises the two — and answered rather than asserted, because a panic
/// on a request path reboots the board over one request.
pub fn with_page<T>(pages: &mut Pages, len: usize, f: impl FnOnce(&[u8]) -> T) -> Option<T> {
    let buffer = pages.rx.try_receive()?;
    let len = len.min(buffer.bytes.len());
    let outcome = f(&buffer.bytes[..len]);
    pages.rx.receive_done();
    Some(outcome)
}

pub fn page(
    pages: &mut Pages,
    store: &mut FlashStore<'static>,
    len: usize,
) -> Result<(), ApiErrorCode> {
    if pages.session.is_none() {
        return Err(ApiErrorCode::UpdateUnwritable);
    }
    let Some(buffer) = pages.rx.try_receive() else {
        // Unreachable: the web server posts a page and only then asks for it to
        // be written, and the request gate serialises the two.
        crate::logln!("ota: a page was asked for and none had been sent");
        pages.session = None;
        return Err(ApiErrorCode::UpdateUnwritable);
    };
    let len = len.min(buffer.bytes.len());
    // **The erase rule below depends on every page but the last being full**,
    // and that is decided in `api::routes::receive` — a different file. A short
    // page in the middle would put every later page off its sector boundary and
    // leave part of a sector unerased under a write. It fails safe today
    // (`NorFlash::write` refuses the unaligned offset, or the read-back reports
    // `NotDurable`), and it fails *legibly* here.
    let short = len != PAGE_BYTES;
    // Re-borrowed here rather than above so the session and the lent page are
    // two disjoint field borrows of `pages`, which is what lets the verifier and
    // the write share one pass over the bytes. The `None` arm is the same one
    // the `is_none` check above already excluded, and it refuses rather than
    // panicking for the reason `crate::rpc` gives about its own unreachable
    // arm — a panic on a request path reboots the board over one request.
    let outcome = match pages.session.as_mut() {
        // A short page is only legitimate as the *last* one, which is exactly
        // the page whose length is everything the verifier still expects.
        Some(session) if short && len != session.verifier.remaining() => {
            crate::logln!(
                "ota: a {}-byte page arrived with {} bytes of the image still to come — every \
                 page but the last has to be {}, or the sector-erase rule stops holding",
                len,
                session.verifier.remaining(),
                PAGE_BYTES,
            );
            Err(ApiErrorCode::UpdateUnwritable)
        }
        Some(session) => {
            let outcome = session
                .verifier
                .feed(&buffer.bytes[..len])
                .map_err(refuse)
                .and_then(|()| {
                    store
                        .with_flash(|flash| write_page(flash, session, &buffer.bytes, len))
                        .map_err(|error| {
                            crate::logln!("ota: the target slot refused a write ({:?})", error);
                            ApiErrorCode::UpdateUnwritable
                        })
                });
            if outcome.is_ok() {
                session.written += len as u32;
            }
            outcome
        }
        None => Err(ApiErrorCode::UpdateUnwritable),
    };
    pages.rx.receive_done();

    if outcome.is_err() {
        // Nothing was marked bootable and nothing will be. The half-written
        // slot is inert: the bootloader only looks at it if `otadata` names it,
        // and `otadata` still names the slot that is running.
        pages.session = None;
    }
    outcome
}

/// Erase where needed, write, and read back.
///
/// The read-back is the same discipline `crate::store` applies to a rolling
/// code, for the same reason: a write that silently did not land is
/// indistinguishable from one that did until something tries to run it, and by
/// then the only recovery is a cable.
///
/// Both scratch buffers are locals of a **synchronous** function, so they live
/// on the executor's stack rather than in any task future or `static` — which
/// is why they can afford to be a whole page each while the page buffer itself
/// is argued over byte by byte.
fn write_page(
    flash: &mut FlashStorage<'_>,
    session: &Session,
    bytes: &[u8; PAGE_BYTES],
    len: usize,
) -> Result<(), OtaError> {
    let at = session.slots.target_at + session.written;
    if (session.written as usize).is_multiple_of(SECTOR_BYTES) {
        flash
            .erase(at, at + SECTOR_BYTES as u32)
            .map_err(|_| OtaError::Flash)?;
    }

    // `NorFlash::write` takes only word-aligned lengths, and the last page of
    // an image need not be one. Padding with 0xFF is padding with what an
    // erased sector already holds, so it writes nothing the flash did not
    // already contain — and it lands past the end of the image, where the
    // bootloader does not look.
    let padded = len.div_ceil(WORD_BYTES) * WORD_BYTES;
    let mut staged = Aligned([0xFF; PAGE_BYTES]);
    staged.0[..len].copy_from_slice(&bytes[..len]);
    flash
        .write(at, &staged.0[..padded])
        .map_err(|_| OtaError::Flash)?;

    let mut check = Aligned([0; PAGE_BYTES]);
    flash
        .read(at, &mut check.0[..padded])
        .map_err(|_| OtaError::Flash)?;
    if check.0[..len] != bytes[..len] {
        return Err(OtaError::NotDurable { at });
    }
    Ok(())
}

/// A page's bytes, aligned so `esp-storage` reads and writes them directly.
///
/// **The same wrapper and the same reason as `crate::store::Slot`**, and it
/// matters more here than there: without the alignment *every* transaction
/// detours through a 4 KB temporary buffer that `esp-storage` places on the
/// caller's stack, and an update makes a few thousand of them on the state
/// task's chain. Two of these are live at once in [`write_page`], so the
/// difference is 512 bytes of stack against 8 KB of it.
#[repr(C, align(4))]
struct Aligned([u8; PAGE_BYTES]);

/// Every byte has arrived. Finish the checks and mark the slot bootable.
///
/// **In that order**, which is the whole point of the task: nothing here
/// touches `otadata` until `somfy_ota` has agreed that what was written is a
/// complete, internally consistent image for this chip.
pub fn finish(pages: &mut Pages, store: &mut FlashStore<'static>) -> Result<(), ApiErrorCode> {
    let Some(session) = pages.session.take() else {
        return Err(ApiErrorCode::UpdateUnwritable);
    };
    let slots = session.slots;
    let accepted = session.verifier.finish().map_err(refuse)?;

    crate::logln!(
        "ota: {} bytes verified — {} version '{}', built for {}. Marking {:?} bootable.",
        accepted.len,
        accepted.project,
        accepted.version,
        accepted.chip.name(),
        slots.target,
    );

    store
        .with_flash(|flash| {
            with_ota(flash, |ota| {
                ota.set_current_app_partition(slots.target)
                    .map_err(|_| OtaError::Flash)?;
                // **The selection and the state are one operation, and this is
                // where that is enforced.** `set_current_app_partition` writes a
                // *new* record and copies whatever `ota_state` the entry it
                // reuses already held — `Undefined` on a first update, `Valid`
                // on a later one. Both of those read as
                // `BootVerdict::Settled`, so a switch that landed without the
                // `New` behind it would boot the new image with **no self-test
                // and no roll-back**: precisely the protection this module
                // exists to provide, silently absent, on the one path where
                // flash has already shown it is unreliable.
                //
                // So the state is written and then **read back** — the same
                // discipline `crate::store` applies to a rolling code — and a
                // failure of either half puts the selection back where it was.
                let marked = ota
                    .set_current_ota_state(OtaImageState::New)
                    .map_err(|_| OtaError::Flash)
                    .and_then(|()| match ota.current_ota_state() {
                        Ok(OtaImageState::New) => Ok(()),
                        _ => Err(OtaError::Flash),
                    });
                if marked.is_err() {
                    // Best effort, and its own failure is reported by the
                    // caller: if this does not land either, the next boot reads
                    // `Undefined` on the target and settles, which is the state
                    // the message below describes as the bad one.
                    let _ = ota.set_current_app_partition(slots.booted);
                }
                marked
            })
        })
        .map_err(|error| {
            crate::logln!(
                "ota: the image is written and verified but otadata could not be marked ({:?}) \
                 — the selection has been put back to {:?}, so this board boots what it is \
                 running now. Check the next boot's `ota: running from` line before assuming \
                 that worked.",
                error,
                slots.booted,
            );
            ApiErrorCode::UpdateUnwritable
        })?;
    crate::logln!(
        "ota: the next boot runs {:?}. It has to pass a self-test within {} s or the board \
         comes back to {:?} on its own.",
        slots.target,
        WINDOW_MS / 1_000,
        slots.booted,
    );
    Ok(())
}

/// Give up on an upload without touching what is running.
pub fn abort(pages: &mut Pages) {
    if pages.session.take().is_some() {
        crate::logln!(
            "ota: upload abandoned — the running slot was never touched and nothing was marked \
             bootable"
        );
    }
    pages.rx.clear();
}

/// One refusal per thing a person can do about it.
///
/// Deliberately coarser than [`ImageError`]: an operator facing "the upload did
/// not arrive intact" does the same thing whether it was short, long or
/// corrupt, and three codes sharing an action are three translations sharing a
/// sentence. The *precise* cause goes to the console, where a developer is.
///
/// **That last sentence was a promise this function did not keep** until
/// 2026-08-18: it mapped and said nothing, so `imageDamaged` reached the
/// operator with no way to tell a short upload from a bad digest, on either
/// side of the wire. Logging here rather than at the three call sites is what
/// makes that true for all of them at once — and it is why this is no longer a
/// `const fn`, which it had no other reason to be.
fn refuse(error: ImageError) -> ApiErrorCode {
    crate::logln!("ota: the image was refused — {:?}", error);
    match error {
        ImageError::NotAnImage { .. }
        | ImageError::NotAnApp { .. }
        | ImageError::BadSegmentCount { .. } => ApiErrorCode::ImageNotFirmware,
        ImageError::WrongChip { .. } => ApiErrorCode::ImageForAnotherChip,
        ImageError::TooLarge { .. } => ApiErrorCode::ImageTooLarge,
        ImageError::Truncated { .. }
        | ImageError::LengthMismatch { .. }
        | ImageError::BadChecksum { .. }
        | ImageError::BadDigest => ApiErrorCode::ImageDamaged,
    }
}
