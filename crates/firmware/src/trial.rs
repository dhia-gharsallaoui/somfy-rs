//! The seam between the settings screen and the radio, for a credential nobody
//! has proved yet.
//!
//! # What is here and what is not
//!
//! The *decision* — whether a trial continues, reverts or commits — is
//! [`somfy_config::WifiTrial`], on the host side, driven by
//! `crates/somfy-config/tests/trial.rs` through every ending it has. What is
//! here is the plumbing that decision needs: somewhere to keep a live trial,
//! the two things that read it ([`crate::api`] and [`watch`]), and the one thing
//! that applies it ([`crate::net`], because it owns the Wi-Fi controller and
//! nothing else may).
//!
//! That split is deliberate. `crates/firmware` is excluded from the root
//! workspace and builds only for Xtensa, so nothing in this file can be tested
//! on a host — which is exactly why nothing in this file decides anything.
//!
//! # Why a revert is a reboot
//!
//! Putting the previous credential back means applying it to the radio, and
//! applying it means holding it — so the passphrase would have to live in a
//! static for as long as any trial might run. `crate::net`'s own docs say the
//! opposite is a property worth keeping: "the task below never sees the
//! credentials at all — it drives the connection, not the configuration — which
//! keeps the passphrase in one place instead of inside a task's statically
//! allocated future."
//!
//! A reboot preserves that. The previous credential is still the newest record
//! in the configuration ring — a trial writes nothing — so the ordinary boot
//! path reads it and joins the network the device was reachable on. There is no
//! restore path to get wrong, no second copy of the passphrase, and no state
//! that a power cut mid-trial could leave behind: a board that loses power
//! during a trial comes back exactly as a board that reverted does.
//!
//! What it costs is a few seconds of radio downtime on top of however long the
//! failed trial already took, on a path that only runs after a settings change
//! that did not work. The shade table, the rolling codes and the announced set
//! are all in flash and survive it; what does not survive is the dead-reckoned
//! position estimate, which the controller re-establishes the way it does after
//! any power cut.
//!
//! # Why this module is compiled into a build that cannot use it
//!
//! The settings screen is the only thing that starts a trial and `http` gates
//! it, but the seam is `crate::net`'s to offer, not the web server's to own —
//! `crate::rpc` is unconditional for exactly the same reason. Keeping it here
//! means `wifi_link`'s loop reads the same in every configuration instead of
//! carrying `#[cfg]`s through the one task that must not be got wrong. What is
//! gated is [`watch`], because a task future is real DRAM and a timer polling a
//! slot nothing can fill is real work.
//!
//! # One trial at a time
//!
//! [`request`] refuses while one is live. Two candidates in flight would mean
//! two deadlines, two revert paths and a confirmation that could not say which
//! credential it was confirming — and the operator who started the second one
//! has, by definition, not yet found out whether the first worked.

use core::cell::RefCell;

#[cfg(feature = "http")]
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::Duration;
#[cfg(feature = "http")]
use embassy_time::{Instant, Timer};
#[cfg(feature = "http")]
use somfy_api::{ApiErrorCode, WifiTrialDto};
#[cfg(feature = "http")]
use somfy_config::{RevertReason, TrialOutcome};
use somfy_config::{WifiCredentials, WifiTrial};

/// How often the trial is polled.
///
/// The deadlines it enforces are 45 seconds and three minutes, so the sampling
/// interval is not a precision question — it is a latency one, and the latency
/// that matters is between the operator pressing *confirm* and the screen
/// answering. That path does not go through this task at all
/// ([`commit`] runs the machine itself), so what is left is how long a revert
/// takes to notice its deadline. Half a second is imperceptible against
/// forty-five, and cheap: one lock, one comparison.
#[cfg(feature = "http")]
const POLL_MS: u64 = 500;

/// How long the Wi-Fi task waits before applying a candidate.
///
/// **Not a settling time for the radio — a settling time for the socket.**
/// Applying the candidate drops the association the request arrived over, so
/// without this the `202` would be written into a TCP connection that dies
/// before it is flushed, and the screen would show a network error for a trial
/// that had in fact started. Half a second is far longer than a 202 with no
/// body takes to leave a LAN socket, and it is time the operator spends reading
/// the instruction to switch networks.
const APPLY_SETTLE_MS: u64 = 500;

/// Everything one trial needs, behind one lock.
///
/// **A `blocking_mutex` around a `RefCell` rather than atomics**, for the reason
/// `crate::net::SIGNAL_DBM` gives — and here the second half of that reason is
/// the load-bearing one on its own: a `WifiTrial` is not `Copy`, so atomics were
/// never the shape for it regardless of the chip. No borrow is held across an
/// await, because none of the functions below is `async`, so there is no path on
/// which the `RefCell` can panic.
struct Slot {
    /// A candidate the web server has asked to try and [`crate::net`] has not
    /// applied yet. Held here rather than sent on a channel because the answer
    /// to "is a trial in progress?" must be `true` from the moment the request
    /// is accepted — otherwise a second request could be accepted in the gap.
    requested: Option<WifiCredentials>,
    /// The live trial, once the candidate is on the radio.
    live: Option<WifiTrial>,
    /// The link state as of [`watch`]'s last tick.
    ///
    /// **`is_config_up`, not "associated"**, and the difference is the whole
    /// point of the guard: a station that has joined an access point and not
    /// been given an address is not reachable, so counting it as a success
    /// would commit a credential nobody can use. The stricter reading is also
    /// the one the operator is about to test.
    link_up: bool,
}

impl Slot {
    const fn empty() -> Slot {
        Slot {
            requested: None,
            live: None,
            link_up: false,
        }
    }

    /// True while anything is in flight, requested or applied.
    #[cfg(feature = "http")]
    fn busy(&self) -> bool {
        self.requested.is_some() || self.live.is_some()
    }
}

static SLOT: Mutex<CriticalSectionRawMutex, RefCell<Slot>> =
    Mutex::new(RefCell::new(Slot::empty()));

/// Raised when a candidate is requested, so the Wi-Fi task leaves whichever
/// wait it is in rather than finishing a sixty-second backoff first.
static REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Ask for `candidate` to be tried.
///
/// Refuses while a trial is already in flight — see this module's docs.
/// Returns as soon as the request is recorded; the radio is not touched here,
/// because the response has to leave over the connection the candidate is about
/// to take down.
#[cfg(feature = "http")]
pub fn request(candidate: WifiCredentials) -> Result<(), ApiErrorCode> {
    SLOT.lock(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.busy() {
            return Err(ApiErrorCode::TrialInProgress);
        }
        slot.requested = Some(candidate);
        Ok(())
    })?;
    REQUESTED.signal(());
    Ok(())
}

/// Wait until a candidate is waiting to be applied.
///
/// Used by [`crate::net`] to cut short a wait it would otherwise sit out. It is
/// level-triggered by construction: [`request`] signals after recording, and
/// [`take_requested`] is what clears the record, so a signal that arrives while
/// the Wi-Fi task is elsewhere is still there when it looks.
pub async fn requested() {
    REQUESTED.wait().await;
}

/// Take the candidate waiting to be applied, if there is one.
pub fn take_requested() -> Option<WifiCredentials> {
    SLOT.lock(|cell| cell.borrow_mut().requested.take())
}

/// Record that `candidate` is now on the radio, and start its clock.
pub fn applied(candidate: WifiCredentials, now_ms: u64) {
    SLOT.lock(|cell| {
        let mut slot = cell.borrow_mut();
        // Cleared as the trial starts rather than left over from the previous
        // association: the candidate is a different network and nothing is
        // known about the link on it yet.
        slot.link_up = false;
        slot.live = Some(WifiTrial::start(candidate, now_ms));
    });
}

/// Report that a candidate could not be put on the radio at all.
///
/// The driver refused the configuration, which is a different thing from the
/// access point refusing the credential — nothing was ever transmitted. The
/// trial is dropped rather than started, so the deadline does not run against a
/// radio that is still on the previous network and working.
pub fn not_applied() {
    SLOT.lock(|cell| {
        let mut slot = cell.borrow_mut();
        slot.requested = None;
        slot.live = None;
    });
}

/// What the settings screen shows about the trial, or `None` when none is live.
#[cfg(feature = "http")]
pub fn status(now_ms: u64) -> Option<WifiTrialDto> {
    SLOT.lock(|cell| {
        let slot = cell.borrow();
        // A requested-but-not-yet-applied candidate reports as a trial in its
        // first phase. It *is* one from the operator's side — they pressed the
        // button and the device is about to leave — and reporting `None` for
        // the half second in between would make the screen flicker back to the
        // form it just submitted.
        match (&slot.live, &slot.requested) {
            (Some(trial), _) => Some(WifiTrialDto::of(trial, now_ms)),
            (None, Some(candidate)) => Some(WifiTrialDto::of(
                &WifiTrial::start(candidate.clone(), now_ms),
                now_ms,
            )),
            (None, None) => None,
        }
    })
}

/// The operator reached the device on the candidate network and said so.
///
/// Runs the machine with the confirmation set and the live link state, and
/// hands back the credential **only** on [`TrialOutcome::Commit`]. Everything
/// else is a refusal that names why, and leaves the trial running: a
/// confirmation that arrives a moment too early should be retriable, not fatal.
///
/// The write is the caller's to do. This function cannot reach flash — the
/// configuration region belongs to the state task — and that is the right
/// division: what is decided here is *whether* the credential has been proved,
/// and what is done there is storing it.
#[cfg(feature = "http")]
pub fn commit(now_ms: u64) -> Result<WifiCredentials, ApiErrorCode> {
    SLOT.lock(|cell| {
        let mut slot = cell.borrow_mut();
        let link_up = slot.link_up;
        let Some(trial) = slot.live.as_mut() else {
            return Err(ApiErrorCode::NoTrialInProgress);
        };
        trial.confirm();
        match trial.poll(now_ms, link_up) {
            TrialOutcome::Commit => Ok(trial.candidate().clone()),
            // Including the reverting outcomes: a trial whose deadline has just
            // passed has not been proved either, and the watcher is about to
            // put the device back. Answering "not associated" is true of every
            // one of them — the station is not on the candidate network with an
            // address — and it is what the screen tells the operator to fix.
            _ => Err(ApiErrorCode::TrialNotAssociated),
        }
    })
}

/// The operator asked for the previous credential back.
///
/// Recorded rather than acted on: the revert is [`watch`]'s, so there is one
/// place a trial ends badly and one reboot to reason about.
#[cfg(feature = "http")]
pub fn cancel() -> Result<(), ApiErrorCode> {
    SLOT.lock(|cell| {
        let mut slot = cell.borrow_mut();
        // A candidate that has been requested and not yet applied is cancelled
        // by dropping it, with no reboot: the radio was never touched.
        if slot.requested.take().is_some() && slot.live.is_none() {
            return Ok(());
        }
        let Some(trial) = slot.live.as_mut() else {
            return Err(ApiErrorCode::NoTrialInProgress);
        };
        trial.cancel();
        Ok(())
    })
}

/// Forget the trial, once its credential is in flash.
///
/// Called only after the write has been acknowledged. A trial cleared before
/// the write landed would leave the device running on a credential it would not
/// come back to.
#[cfg(feature = "http")]
pub fn end() {
    SLOT.lock(|cell| {
        let mut slot = cell.borrow_mut();
        slot.requested = None;
        slot.live = None;
    });
}

/// Watch the live trial and put the device back if it fails.
///
/// The only task that ends a trial badly, and the only caller of
/// `esp_hal::system::software_reset` on this path. Everything it does when
/// nothing is happening is one lock and one comparison every [`POLL_MS`].
#[cfg(feature = "http")]
#[embassy_executor::task]
pub async fn watch(stack: Stack<'static>) -> ! {
    loop {
        Timer::after(Duration::from_millis(POLL_MS)).await;

        // Sampled outside the lock: `is_config_up` walks the stack's own state
        // and there is no reason to hold a critical section across it.
        //
        // `is_config_up`, not `is_link_up` — see `Slot::link_up`.
        let link_up = stack.is_config_up();
        let now_ms = Instant::now().as_millis();

        let outcome = SLOT.lock(|cell| {
            let mut slot = cell.borrow_mut();
            slot.link_up = link_up;
            slot.live.as_mut().map(|trial| trial.poll(now_ms, link_up))
        });

        let Some(outcome) = outcome else {
            continue;
        };

        match outcome {
            TrialOutcome::Waiting(_) => {}
            // Reached only through `commit`, which polls the machine itself and
            // hands the credential to the writer. If the machine says `Commit`
            // here, the write has already been acknowledged and `end` has not
            // been called yet — a moment, not a state — so there is nothing to
            // do but wait for it.
            TrialOutcome::Commit => {}
            TrialOutcome::Revert(reason) => {
                // Printed before the reset, because after it there is nothing
                // to read: this line and the boot banner that follows it are the
                // whole record of what happened.
                crate::logln!(
                    "wifi: the credential trial was not proved ({}) — restarting onto \
                     the stored credential, which was never overwritten",
                    describe(reason),
                );
                esp_hal::system::software_reset();
            }
        }
    }
}

/// What to say about a reverting trial.
///
/// Exhaustive, so a reason added in `somfy-config` stops this compiling rather
/// than reaching a serial console as a number.
#[cfg(feature = "http")]
fn describe(reason: RevertReason) -> &'static str {
    match reason {
        RevertReason::NotAssociated => {
            "the network never came up — the passphrase is wrong, or the SSID is not on the air"
        }
        RevertReason::NotConfirmed => "nobody confirmed from the new network in time",
        RevertReason::Cancelled => "cancelled",
        RevertReason::LinkLost => {
            "the network came up and dropped again, which is what a MAC policy or an \
             exhausted DHCP pool looks like"
        }
    }
}

/// How long the Wi-Fi task waits before applying a candidate. See
/// [`APPLY_SETTLE_MS`].
pub fn settle() -> Duration {
    Duration::from_millis(APPLY_SETTLE_MS)
}
