//! What a running image should do about the `otadata` state it booted with.
//!
//! # The one ambiguity this exists to sidestep
//!
//! ESP-IDF gives an image slot six states, and two of them mean "not confirmed
//! yet": [`ImageState::New`], which an update writes, and
//! [`ImageState::PendingVerify`], which **the bootloader** writes over `New` on
//! first boot — but only if it was built with `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`.
//! That option defaults to off, and the bootloader `espflash` ships is a stock
//! prebuilt whose configuration nobody here can read off the device.
//!
//! So a running image that finds `PendingVerify` cannot tell whether:
//!
//! - the bootloader promoted `New` a moment ago and this is a healthy first
//!   boot, or
//! - *it* wrote `PendingVerify` on a previous boot and never reached a verdict,
//!   because it panicked or hung.
//!
//! Both readings have been shipped by real projects and both are wrong half the
//! time. Reading it as "first boot" means a crash loop never rolls back — a
//! brick. Reading it as "previous attempt failed" means **every** update rolls
//! back immediately on a bootloader that has rollback enabled.
//!
//! # What replaces the guess
//!
//! An attempt count that lives outside `otadata` entirely, in the chip's
//! RTC-fast memory, which esp-hal zeroes on a power-on reset and preserves
//! across a software reset. That is exactly the discriminator the state field
//! cannot be:
//!
//! | What happened | Reset | Count | Verdict |
//! |---|---|---|---|
//! | An update was just activated | software, from the upload handler | 0 | [`BootVerdict::Verify`] |
//! | The new image panicked during its soak | software, from the panic handler | 1 | [`BootVerdict::RollBack`] |
//! | The power blipped during the soak | power-on | 0 | [`BootVerdict::Verify`] again |
//! | The image confirmed itself | — | — | [`BootVerdict::Settled`] |
//! | A roll-back was decided and did not take effect | software | 1 | [`BootVerdict::Settled`] — one switch per power-on |
//!
//! The third row is a deliberate improvement on ESP-IDF's own behaviour, not an
//! accident of the mechanism: a power cut is not evidence against a release,
//! and rolling one back over a blip would be a failure mode the operator cannot
//! distinguish from the update simply not having worked.
//!
//! **A brownout counts as a crash**, because esp-hal only zeroes the region on
//! `ChipPowerOn`. That is the right way round — a board that browns out under
//! its own load has a fault the previous image may not have had — and it is
//! recorded here so it is not discovered as a surprise.
//!
//! # What is still not covered
//!
//! A **hang**. An image that stops making progress without panicking never
//! resets, so nothing counts an attempt and nothing rolls back. Closing that
//! needs a hardware watchdog armed across the soak, which this firmware does
//! not have; it is named in `docs/hardware-checklist.md` rather than implied
//! away.

/// What `otadata` says about the slot that is running.
///
/// The six ESP-IDF states plus one this crate needs and the format does not
/// have: [`ImageState::Absent`], for a region that has never been written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    /// `otadata` is blank — both sequence numbers are `0xFFFFFFFF`. The board
    /// was flashed over serial and has never taken an update, which is the
    /// state every board starts in.
    Absent,
    /// An update wrote this slot and marked it bootable. Nothing has confirmed
    /// it yet.
    New,
    /// Not confirmed. See the module docs for why this does not, on its own,
    /// mean what it looks like it means.
    PendingVerify,
    /// Confirmed by a self-test.
    Valid,
    /// A self-test refused it.
    Invalid,
    /// The bootloader gave up on it.
    Aborted,
    /// No claim either way. What a slot seeded by this firmware carries before
    /// it has ever been the target of an update.
    Undefined,
}

/// Why an image is being rolled back.
///
/// Carried into the console line, because "rolled back" without a cause is the
/// hardest kind of message to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollBackReason {
    /// This image has already had its attempt and did not reach a verdict —
    /// it panicked, or something reset it, during its self-test.
    AttemptExhausted,
    /// A self-test on some earlier boot marked it bad and the switch back did
    /// not take effect. Reaching this means the roll-back below did not
    /// complete, so it is retried rather than assumed done.
    MarkedInvalid,
    /// The bootloader marked it aborted, which it does when rollback is enabled
    /// and a `PendingVerify` image boots a second time.
    MarkedAborted,
}

/// What to do about the state this image booted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootVerdict {
    /// Nothing to do. Either no update ever happened, or this image has already
    /// been confirmed.
    Settled,
    /// Run the self-test and confirm or roll back on its answer.
    Verify,
    /// Go back to the other slot now, without waiting for a self-test that a
    /// previous boot of this image has already failed to finish.
    RollBack(RollBackReason),
}

/// Decide, from the two things a booting image can know about itself.
///
/// `attempts_before` is how many times this image has already started since the
/// last power-on — zero on the first boot after an update, and on the first
/// boot after a power cut. See the module docs for why it, and not the state
/// field, is what distinguishes a first attempt from a repeat.
///
/// **It is cleared when an upload begins and when an image confirms itself, and
/// at no other time.** Clearing it on a roll-back instead was the first design
/// and it was wrong twice over: it left the count unable to bound the
/// `Invalid`/`Aborted` retry below, and it would have let a *later* update in
/// the same power cycle inherit a stale figure. Clearing it where a new
/// attempt genuinely begins fixes both.
pub const fn boot_verdict(state: ImageState, attempts_before: u32) -> BootVerdict {
    match state {
        // A board that has never taken an update, and one that has already
        // confirmed. `Undefined` is what this firmware's own seeding writes,
        // and it means exactly what it says: nobody has claimed anything about
        // this slot, so there is nothing to verify.
        ImageState::Absent | ImageState::Valid | ImageState::Undefined => BootVerdict::Settled,
        // **Bounded by the same counter, and that is not symmetry for its own
        // sake.** Reaching either of these means a roll-back was decided and
        // did not take effect — the flash refused the switch, or it landed on a
        // record that was itself condemned. Retrying is right; retrying
        // *forever* is a board that resets every few seconds, writes `otadata`
        // three times per iteration and never reaches its executor, which is
        // the one failure this module exists to make impossible.
        //
        // So: one switch per power-on. After that the board comes to rest
        // running an image whose own record calls it bad, and says so on the
        // console. That is a poor state and it is strictly better than a reboot
        // loop, because it can be reached over the network and fixed; a power
        // cycle re-arms one more attempt.
        ImageState::Invalid if attempts_before == 0 => {
            BootVerdict::RollBack(RollBackReason::MarkedInvalid)
        }
        ImageState::Aborted if attempts_before == 0 => {
            BootVerdict::RollBack(RollBackReason::MarkedAborted)
        }
        ImageState::Invalid | ImageState::Aborted => BootVerdict::Settled,
        ImageState::New | ImageState::PendingVerify => {
            if attempts_before == 0 {
                BootVerdict::Verify
            } else {
                BootVerdict::RollBack(RollBackReason::AttemptExhausted)
            }
        }
    }
}
