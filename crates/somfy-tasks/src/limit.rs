//! How often one shade may be commanded, and why there is a bound at all.
//!
//! # The resource being protected is flash, not bandwidth
//!
//! Every command this controller accepts commits a rolling code to flash
//! **before** it transmits, and that ordering is not a preference — a code that
//! went out on the air without being persisted is a code this device will send
//! again after a power cut, and a motor rejects a repeated code as a replay.
//! [`somfy_store::transmit`] is the only route to the radio queue precisely so
//! that the ordering cannot be skipped.
//!
//! The consequence is that a request loop is a **write** loop. Two things
//! follow, and only the first is obvious:
//!
//! - **The region wears out.** `rollcode` is 8 KB: 32 slots of 256 bytes across
//!   two 4 KB erase sectors, so one commit is one slot and a full lap of 32
//!   commits erases each sector once. At the 100,000-cycle endurance NOR flash
//!   is quoted at, that is **3,200,000 commits** for the life of the device
//!   (`docs/hardware-checklist.md`, "Region layout"). It is a physical,
//!   unrecoverable limit: past it the codes of every paired motor stop being
//!   storable, and recovering means re-pairing at each window by hand.
//! - **The receiver goes deaf while it writes.** `esp-storage` runs an erase
//!   with interrupts disabled, tens of milliseconds typical. [`StateMachine`](crate::StateMachine)'s
//!   module documentation argues that this is tolerable *because* an erase only
//!   ever happens next to a transmit burst that was about to deafen the
//!   receiver anyway. A loop makes that argument false by making the bursts
//!   continuous.
//!
//! **Authentication would not have closed either.** An authenticated client can
//! loop exactly as fast, which is why this ships whether or not a password ever
//! does.
//!
//! # Where this sits, and why the vent cannot starve itself
//!
//! [`CommandLimiter`] is consulted from
//! [`StateMachine::apply`](crate::StateMachine::apply) — **the one function
//! both transports arrive at**. An HTTP `POST …/command` reaches it through the
//! request seam and an MQTT command reaches it through the command channel, and
//! they meet here rather than in either transport's own code, because the
//! standing rule is one behaviour behind every door.
//!
//! What it is *not* consulted from matters just as much.
//! [`StateMachine::tick`](crate::StateMachine::tick) is the controller finishing
//! something it already started: arrival stops, a seek's second leg, and — the
//! case worth naming — the second and third frames of a
//! [`ShadeCommand::Vent`](somfy_domain::ShadeCommand::Vent), which is a full
//! close, a whole travel time of waiting, an Up, and a stop. Those legs are due
//! at a time the *clock* picked, not at a time a client picked, and a limiter
//! that could refuse them would leave a shade closed with no vent coming — the
//! command half-done, in the one direction the operator did not ask for. They
//! are exempt structurally rather than by a flag: they never pass through
//! `apply` at all.
//!
//! The same is true of
//! [`begin_calibration`](crate::StateMachine::begin_calibration), which is a
//! different request with a guard of its own, and of the bring-up entry points
//! [`command_shade`](crate::StateMachine::command_shade) and
//! [`command_group`](crate::StateMachine::command_group), which exist so a
//! harness can drive a shade directly.
//!
//! # Per shade, and what that deliberately does not bound
//!
//! The bucket is per shade, so a client hammering one shade cannot stop the
//! operator moving any other. That is the same anti-lockout standard the web
//! server holds with its reserved connection tasks, and it is the reason this
//! is not a single device-wide bucket even though the flash region *is*
//! device-wide. A device-wide cap would bound the wear exactly, and would also
//! hand any one shade the power to starve the whole house; the trade is set out
//! in full on [`REFILL_INTERVAL_MS`], along with what the residual costs.

use heapless::Vec;
use somfy_domain::{Registry, ShadeId, MAX_SHADES};

use crate::ControlCommand;

/// Commands one shade may be given back to back before the rate below applies.
///
/// **A policy figure, and it is a claim about people rather than a
/// measurement.** It is what somebody standing at a window does in one go: Down,
/// stop, Up, stop is four; a position slider dragged and corrected a few times
/// is a handful more; a pairing attempt that did not take the first time is a
/// `Prog` and two test presses, twice over. Twelve is chosen *above* the widest
/// of those rather than at it, because being wrong in one direction costs a
/// person a wait and being wrong in the other costs a few hundred flash writes.
///
/// It is the burst, not the rate, that has to cover real use — people operate
/// shades in bursts and then walk away, which is exactly the shape a token
/// bucket is for.
pub const BURST: u32 = 12;

/// How long one command's worth of allowance takes to come back, in **seconds**.
/// 20 — **three commands per minute, per shade, sustained.**
///
/// Seconds rather than milliseconds because the whole schedule is stored at this
/// resolution — see [`CommandLimiter`], where it is worth 256 bytes of DRAM on
/// the chip that has the least of it. Truncating a millisecond clock to whole
/// seconds means the interval a client actually waits is between 19 and 20
/// seconds rather than exactly 20; against a policy figure chosen an order of
/// magnitude above real use, that is not a distinction with a consequence.
///
/// **A policy figure.** Nothing measured says three; what measurement there is
/// says only that three is far above anything real and far below anything
/// damaging, and here are both halves of that.
///
/// **Above real use.** Three a minute is 4,320 commands per day at one shade,
/// for ever. A household of twenty shades moved ten times each is 200 commands
/// a day *in total* — so this is more than two orders of magnitude above what
/// the device is actually asked to do, and [`BURST`] absorbs the peaks that an
/// average hides.
///
/// **Below damaging.** One admitted command costs between one and four flash
/// commits: [`somfy_domain::Shade::handle`] plans at most two frames — the
/// command's own, plus an arrival stop flushed on the way in — and a vent adds
/// two more later from the tick path. So one shade driven flat out costs at
/// most 12 commits a minute, and the 3,200,000-commit region survives **185
/// days** of that continuously; at the ordinary one commit per command, two
/// years. Unlimited, the same loop is bounded only by how fast HTTP answers —
/// a few hundred requests a second on a LAN, which uses the region up in about
/// **four hours**.
///
/// **What it does not bound, stated rather than left to be discovered.** The
/// flash region is shared by every shade, so thirty-two shades driven at once
/// cost thirty-two times this: 96 commands a minute, up to 384 commits, and the
/// region lasts **5.8 days** rather than four hours. That is a factor of about
/// thirty on the worst case and about a thousand on the realistic one — a page
/// in a browser tab hammers an endpoint, not thirty-two of them — and it is the
/// price of choosing per-shade isolation over a device-wide cap. **If that
/// trade is ever revisited, this is the paragraph to revisit it against.**
pub const REFILL_INTERVAL_S: u32 = 20;

/// [`REFILL_INTERVAL_S`] in milliseconds, which is the unit callers and tests
/// speak. Derived rather than written twice.
pub const REFILL_INTERVAL_MS: u64 = REFILL_INTERVAL_S as u64 * 1_000;

/// How far ahead of now a full bucket's deadline may sit, in seconds.
///
/// The bucket is kept as a deadline rather than as a count — see
/// [`CommandLimiter`] — and this is [`BURST`] expressed in that currency.
const TOLERANCE_S: u32 = (BURST - 1) * REFILL_INTERVAL_S;

// A bucket that could not hold one command would refuse everything, which is a
// dead controller rather than a rate limit. Asserted rather than commented,
// because the arithmetic above underflows at zero.
const _: () = assert!(
    BURST >= 1,
    "a bucket that holds no commands refuses every command",
);

/// Why a command was refused, and when to try again.
///
/// The delay is carried rather than left to the caller to guess: it is exactly
/// how long until the bucket holds one command, so a client that honours it
/// succeeds on the next attempt instead of retrying into another refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooSoon {
    /// Milliseconds until this shade will accept a command again.
    pub retry_after_ms: u64,
}

/// One token bucket per shade, kept as a deadline.
///
/// # It stores a time, not a count
///
/// A written-out token bucket needs two numbers per shade — how many tokens
/// remain and when they were last topped up — and then has to decide what to do
/// with the fractional token between refills. The equivalent formulation used
/// here keeps **one** `u64` instead: the time at which this shade's allowance
/// would run out if every command in it were spent now. A command is admitted
/// while that time is no further ahead than `TOLERANCE_MS` — [`BURST`] minus
/// one intervals — and admitting one pushes it out by [`REFILL_INTERVAL_MS`].
///
/// The behaviour is exactly a bucket of [`BURST`] refilling one per
/// [`REFILL_INTERVAL_S`], with no periodic work: it is arithmetic on the
/// timestamp the caller already holds, so nothing runs while the device is idle.
///
/// # Why it is seconds, and what that was worth
///
/// [`MAX_SHADES`] × 4 = **128 bytes**, and the same table at millisecond
/// resolution would be 256. That difference is not academic: this array lives
/// in the state task's statically-allocated future, `esp-hal`'s linker gives
/// the main stack whatever DRAM is left after the statics, and
/// `firmware::heap` hands the Wi-Fi driver's heap the remainder **rounded down
/// to a whole KiB**. Measured on an ESP32 the wider table left 66,140 bytes of
/// `.stack` against a 66,280-byte budget and cost the driver a whole kilobyte
/// of heap; the narrower one leaves 66,396 and costs nothing. On the chip whose
/// entire heap slack is under 4,000 bytes, 128 bytes of table bought 1,024
/// bytes of Wi-Fi buffer.
///
/// `u32` seconds also removes a question rather than raising one: it counts
/// 136 years from boot, so unlike a 32-bit millisecond counter there is no
/// wrap to reason about and no stale-value case to clamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLimiter {
    /// Per shade, the second at which its allowance runs out. Zero is a shade
    /// that has never been commanded, which the `max` in
    /// [`CommandLimiter::wait_for`] reads as "full".
    exhausted_at_s: [u32; MAX_SHADES],
}

impl Default for CommandLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandLimiter {
    /// A limiter with every shade's allowance full.
    pub const fn new() -> Self {
        Self {
            exhausted_at_s: [0; MAX_SHADES],
        }
    }

    /// Admit one command, charging every shade it would move.
    ///
    /// A group is checked across **all** its members before any of them is
    /// charged, and refused whole. That is the standard
    /// [`somfy_domain::Controller::command_group`] already holds for a member
    /// whose frame width cannot carry the command: half a group moved and the
    /// rest refused is a group somebody has to inspect shade by shade to find
    /// out what it did.
    ///
    /// A command naming a shade or group that does not exist is admitted here
    /// and refused by the domain a moment later with its own error. That is
    /// deliberate: it costs no flash, so there is nothing to protect, and
    /// reporting it as a rate limit would send the operator looking for traffic
    /// that does not exist.
    pub fn admit(
        &mut self,
        registry: &Registry,
        command: ControlCommand,
        now_ms: u64,
    ) -> Result<(), TooSoon> {
        // Collected rather than walked twice so that "check every member, then
        // charge every member" is one decision over one list. `MAX_SHADES` is
        // the registry's own bound on a group's size, so this cannot truncate.
        let targets: Vec<ShadeId, MAX_SHADES> = match command {
            ControlCommand::Shade { id, .. } => {
                let mut one = Vec::new();
                // Cannot fail: one element into a vector of MAX_SHADES.
                let _ = one.push(id);
                one
            }
            ControlCommand::Group { id, .. } => registry.group_shades(id).collect(),
        };

        // The longest of the members' waits, so a client that honours it finds
        // the whole group ready rather than the next member refusing in turn.
        let mut wait_ms = 0;
        for id in &targets {
            if let Some(wait) = self.wait_for(*id, now_ms) {
                wait_ms = wait_ms.max(wait);
            }
        }
        if wait_ms > 0 {
            return Err(TooSoon {
                retry_after_ms: wait_ms,
            });
        }

        for id in &targets {
            self.charge(*id, now_ms);
        }
        Ok(())
    }

    /// How long until this shade would accept a command, in milliseconds, or
    /// `None` if it would accept one now.
    fn wait_for(&self, id: ShadeId, now_ms: u64) -> Option<u64> {
        // A shade outside the registry's range has no bucket and costs no
        // flash; see `admit`.
        let exhausted_at = *self.exhausted_at_s.get(id.0 as usize)?;
        let now_s = seconds(now_ms);
        // `max(now_s)` is what stops allowance accumulating past full: a shade
        // nobody has touched for a week has `BURST` commands available, not a
        // week's worth.
        let ahead = exhausted_at.max(now_s) - now_s;
        ahead
            .checked_sub(TOLERANCE_S)
            .filter(|wait| *wait > 0)
            .map(|wait| u64::from(wait) * 1_000)
    }

    /// Spend one command's allowance.
    fn charge(&mut self, id: ShadeId, now_ms: u64) {
        let now_s = seconds(now_ms);
        if let Some(exhausted_at) = self.exhausted_at_s.get_mut(id.0 as usize) {
            *exhausted_at = (*exhausted_at).max(now_s).saturating_add(REFILL_INTERVAL_S);
        }
    }
}

/// Whole seconds since boot, saturating rather than wrapping.
///
/// `u32` seconds reaches 136 years, so the saturation is unreachable and is
/// written only because a silent truncation on a clock is the kind of thing
/// that is discovered in the field rather than in a test.
fn seconds(now_ms: u64) -> u32 {
    (now_ms / 1_000).min(u32::MAX as u64) as u32
}
