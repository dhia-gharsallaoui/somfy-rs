//! Changing Wi-Fi credentials without being able to lock the operator out.
//!
//! # The failure this exists to prevent
//!
//! The settings screen is served **over the network it edits**. Save a
//! passphrase with one character wrong and the device leaves the network it was
//! reachable on, fails to join the one it was told about, and the only way back
//! in is the USB cable the screen exists to eliminate. The credential is already
//! in flash by then, so a reboot does not help either: the device comes back and
//! makes the same wrong choice, forever.
//!
//! # Why "did it associate?" is not the question
//!
//! The obvious guard is to commit, wait, and roll back if the station has not
//! associated within some window. It catches a wrong passphrase, and it catches
//! nothing else.
//!
//! Consider the ordinary move: the operator is on network A, the device is on
//! network A, and they point the device at network B. Everything works — the
//! device associates with B on the first attempt, the guard is satisfied, the
//! credential is kept — and the operator, still on A, cannot reach the device
//! again. Association was never the property that mattered. **Reachability by
//! the person making the change** is, and the only evidence of it is that
//! person arriving.
//!
//! So the guard here is a dead man's handle: the candidate credential is put on
//! the radio but **not into flash**, and it is written only once somebody has
//! come back through it and said so. [`WifiTrial::confirm`] is that arrival.
//! Nothing else ends a trial in the candidate's favour — not a timer, not an
//! association, not a successful DHCP lease.
//!
//! # What that buys, beyond covering the second case
//!
//! - **A failed attempt leaves no trace.** Flash is not written, so there is no
//!   wear per attempt, no torn record to recover from, and no window in which
//!   the newest record in the ring is one nobody has proved.
//! - **A power cut during a trial reverts by construction.** The stored record
//!   is still the previous credential, so the board comes back on the network it
//!   was reachable on. There is no trial marker to interpret at boot, because
//!   there is nothing in flash to mark.
//! - **The revert needs no new machinery.** The previous credential never left
//!   the caller's hand; putting it back is applying a value it already holds.
//!
//! # The two deadlines, and why there are two
//!
//! A single confirmation deadline would work, and it would make the commonest
//! failure — a mistyped passphrase — cost the operator the *whole* window with
//! the device off the air, when it was knowable within seconds that no
//! confirmation could ever arrive. So association is watched as well, and a
//! trial that has not associated by [`ASSOCIATE_DEADLINE_MS`] is reverted at
//! once rather than waited out. Association is not evidence *for* the candidate
//! here; its absence is evidence against it.
//!
//! # Example
//!
//! ```
//! use somfy_config::{RevertReason, TrialOutcome, TrialPhase, WifiCredentials, WifiTrial};
//!
//! let candidate = WifiCredentials::new("example-network", "PLACEHOLDER_PASSPHRASE")?;
//! let mut trial = WifiTrial::start(candidate, 0);
//!
//! // Associated, so the association deadline no longer applies.
//! assert_eq!(trial.poll(3_000, true), TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation));
//!
//! // Nobody arrives. The credential is put back and never reaches flash.
//! let late = 3_000 + somfy_config::CONFIRM_DEADLINE_MS + 1;
//! assert_eq!(trial.poll(late, true), TrialOutcome::Revert(RevertReason::NotConfirmed));
//! # Ok::<(), somfy_config::CredentialError>(())
//! ```

use crate::credentials::WifiCredentials;

/// How long a candidate credential has to associate before it is judged wrong.
///
/// **A policy figure, not a measurement**, in the sense
/// `crate::catalog::DEBOUNCE_MS` is not: nothing about the radio makes 45
/// seconds a boundary. It is chosen against two costs that pull in opposite
/// directions.
///
/// Too short and a network that is merely slow — a busy access point, a DHCP
/// server that takes its time, a station that loses the first attempt and
/// succeeds on the retry after its backoff — is reverted while it was about to
/// work, and the operator is told their correct passphrase is wrong. The
/// caller's reconnect backoff starts at one second and doubles, so 45 seconds
/// admits the first five attempts (at 0, 1, 3, 7 and 15 seconds) with room for
/// the association and lease of the fifth.
///
/// Too long and a mistyped passphrase costs the operator that whole window with
/// the device off the air before anything tells them. Forty-five seconds is a
/// wait somebody watching a page will sit through; three minutes is not.
///
/// It is deliberately **shorter** than [`CONFIRM_DEADLINE_MS`], because the two
/// measure different things: this one is the device's own progress, which it can
/// judge, and that one is a human walking to a phone.
pub const ASSOCIATE_DEADLINE_MS: u64 = 45_000;

/// How long the operator has to reach the device on the new network and confirm.
///
/// Also a policy figure. It has to cover a person noticing the instruction,
/// switching a phone or laptop to the new network — which on a phone is a
/// handful of taps and on a laptop can involve a captive-portal check that takes
/// its own time — and loading a page. Three minutes is generous for that and
/// costs nothing when it is not needed, because the deadline is only reached by
/// somebody who was never coming.
///
/// The cost of it expiring is one revert: the device returns to the network it
/// was already reachable on, and the operator tries again. The cost of it being
/// too short is that a change which was going to work is undone under somebody
/// halfway through making it, which is the more annoying of the two and the
/// harder to understand from the outside.
pub const CONFIRM_DEADLINE_MS: u64 = 180_000;

/// What a live trial is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialPhase {
    /// The candidate is on the radio and the station has not associated yet.
    /// [`ASSOCIATE_DEADLINE_MS`] applies.
    Associating,
    /// The station associated. [`CONFIRM_DEADLINE_MS`] applies, and the clock
    /// restarted when this phase began — the operator's window is theirs alone
    /// and is not eaten by however long the association took.
    AwaitingConfirmation,
}

/// Why a trial ended with the candidate discarded.
///
/// Carried rather than collapsed into one "it failed", because the three mean
/// different things to whoever is looking at the screen: one says the
/// passphrase is wrong, one says nobody arrived, and one says they changed
/// their mind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertReason {
    /// Never associated inside [`ASSOCIATE_DEADLINE_MS`]. The usual cause is a
    /// wrong passphrase or an SSID that is not on the air.
    NotAssociated,
    /// Associated, and nobody confirmed inside [`CONFIRM_DEADLINE_MS`]. The
    /// case the association deadline cannot see: the device joined the network
    /// and the operator is not on it.
    NotConfirmed,
    /// The operator asked for the previous credential back.
    Cancelled,
    /// The station associated and then dropped again before anybody confirmed.
    ///
    /// Distinct from [`RevertReason::NotAssociated`] because it is a *worse*
    /// sign, not an earlier one: a credential the access point accepts and then
    /// disowns is what a MAC-address policy, a full DHCP pool or a failing
    /// radio looks like, and none of those is fixed by retyping the passphrase.
    LinkLost,
}

/// What the caller must do now.
///
/// There is no variant meaning "keep waiting, and also do this" — every poll
/// answers with exactly one instruction, so a caller cannot half-apply an
/// outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialOutcome {
    /// The trial continues. Nothing to do.
    Waiting(TrialPhase),
    /// Put the previous credential back on the radio. **Nothing was written to
    /// flash**, so there is nothing to undo there.
    Revert(RevertReason),
    /// Somebody came back through the new network and said so. Write the
    /// candidate to flash; it is the credential now.
    Commit,
}

/// A candidate Wi-Fi credential that is on the radio and not in flash.
///
/// Deliberately not `Copy`: a trial is a single live thing, and a caller
/// holding two copies of one would poll them separately and reach two
/// conclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiTrial {
    candidate: WifiCredentials,
    /// When the current phase began. Reset on the move to
    /// [`TrialPhase::AwaitingConfirmation`], so each deadline measures its own
    /// phase rather than the whole trial.
    phase_started_ms: u64,
    phase: TrialPhase,
    /// Set by [`WifiTrial::confirm`] and [`WifiTrial::cancel`]. Held rather
    /// than acted on immediately so that **every** ending goes through
    /// [`WifiTrial::poll`] and there is one place where an outcome is decided.
    decision: Option<Decision>,
}

/// An out-of-band ending, waiting for the next poll to report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Confirmed,
    Cancelled,
}

impl WifiTrial {
    /// Begin a trial of `candidate`, starting the association deadline at `now_ms`.
    ///
    /// The caller is expected to have already pointed the radio at the
    /// candidate. Nothing here touches flash, and nothing here can.
    pub fn start(candidate: WifiCredentials, now_ms: u64) -> WifiTrial {
        WifiTrial {
            candidate,
            phase_started_ms: now_ms,
            phase: TrialPhase::Associating,
            decision: None,
        }
    }

    /// The credential under trial, for the caller that has to write it on
    /// [`TrialOutcome::Commit`].
    pub fn candidate(&self) -> &WifiCredentials {
        &self.candidate
    }

    /// What the trial is waiting for, without advancing it.
    pub fn phase(&self) -> TrialPhase {
        self.phase
    }

    /// Record that the operator reached the device and accepted the change.
    ///
    /// Takes effect at the next [`WifiTrial::poll`], and **only if the station
    /// is still associated then** — a confirmation that arrives as the link
    /// drops must not commit a credential the device is no longer on. Confirming
    /// twice is the same as confirming once.
    pub fn confirm(&mut self) {
        self.decision = Some(Decision::Confirmed);
    }

    /// Record that the operator asked for the previous credential back.
    ///
    /// A cancellation beats a confirmation that has not been polled yet: of the
    /// two, only one is reversible afterwards.
    pub fn cancel(&mut self) {
        self.decision = Some(Decision::Cancelled);
    }

    /// Advance the trial and say what to do.
    ///
    /// `associated` is the station's link state as the caller sees it now. It is
    /// passed in rather than remembered because this type has no radio and must
    /// not pretend to: the caller is the only thing that knows, and a trial
    /// deciding on a cached answer would revert a link that had come back.
    ///
    /// # The order of the checks is the guarantee
    ///
    /// A cancellation is honoured before anything else, so a cancel cannot be
    /// beaten by a deadline expiring in the same millisecond. A lost link is
    /// checked before a confirmation, so an operator's click cannot commit a
    /// credential the device has just fallen off. Everything else is a deadline,
    /// and the deadlines are strict (`>`), so `poll` at exactly the deadline is
    /// still inside it.
    pub fn poll(&mut self, now_ms: u64, associated: bool) -> TrialOutcome {
        if self.decision == Some(Decision::Cancelled) {
            return TrialOutcome::Revert(RevertReason::Cancelled);
        }

        let elapsed = now_ms.saturating_sub(self.phase_started_ms);

        match self.phase {
            TrialPhase::Associating => {
                if associated {
                    // The operator's window starts here, not at `start`.
                    self.phase = TrialPhase::AwaitingConfirmation;
                    self.phase_started_ms = now_ms;
                    return self.poll(now_ms, associated);
                }
                if elapsed > ASSOCIATE_DEADLINE_MS {
                    TrialOutcome::Revert(RevertReason::NotAssociated)
                } else {
                    TrialOutcome::Waiting(TrialPhase::Associating)
                }
            }
            TrialPhase::AwaitingConfirmation => {
                if !associated {
                    return TrialOutcome::Revert(RevertReason::LinkLost);
                }
                if self.decision == Some(Decision::Confirmed) {
                    return TrialOutcome::Commit;
                }
                if elapsed > CONFIRM_DEADLINE_MS {
                    TrialOutcome::Revert(RevertReason::NotConfirmed)
                } else {
                    TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation)
                }
            }
        }
    }

    /// Milliseconds left in the current phase, for a screen that is counting
    /// down.
    ///
    /// Saturates at zero rather than going negative: past the deadline the
    /// honest answer is "none", and the outcome is [`WifiTrial::poll`]'s to
    /// give, not this one's.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        let deadline = match self.phase {
            TrialPhase::Associating => ASSOCIATE_DEADLINE_MS,
            TrialPhase::AwaitingConfirmation => CONFIRM_DEADLINE_MS,
        };
        deadline.saturating_sub(now_ms.saturating_sub(self.phase_started_ms))
    }
}
