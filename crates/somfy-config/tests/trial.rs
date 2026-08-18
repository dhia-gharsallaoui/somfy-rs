//! The Wi-Fi credential trial, driven through every ending it has.
//!
//! # Why these tests are the deliverable and not a formality
//!
//! The guard they cover cannot be observed on the bench without a second
//! network, a wrong passphrase and somebody willing to lose the connection they
//! are watching it over. So the *decision* was made a pure function of elapsed
//! time and link state precisely so that it could be driven here — and every
//! test below is the guard firing, not a check that it exists.
//!
//! Each ending is exercised twice where the distinction matters: once at the
//! deadline, where the trial must survive, and once past it, where it must not.
//! An off-by-one in the comparison is the single most likely way to write a
//! guard that never fires, and it is invisible in every other kind of test.

use somfy_config::{
    CredentialError, Field, RevertReason, TrialOutcome, TrialPhase, WifiCredentials, WifiTrial,
    ASSOCIATE_DEADLINE_MS, CONFIRM_DEADLINE_MS,
};

/// The credential a trial is started with. Synthetic throughout: no real SSID
/// or passphrase appears in this repository.
fn candidate() -> WifiCredentials {
    WifiCredentials::new("example-network", "PLACEHOLDER_PASSPHRASE").expect("a valid credential")
}

/// A trial that has reached [`TrialPhase::AwaitingConfirmation`] at `t = 1_000`.
fn associated_at_1000() -> WifiTrial {
    let mut trial = WifiTrial::start(candidate(), 0);
    assert_eq!(
        trial.poll(1_000, true),
        TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation),
    );
    trial
}

// ---------------------------------------------------------------------------
// The credential is wrong: the station never associates
// ---------------------------------------------------------------------------

#[test]
fn a_credential_that_never_associates_survives_up_to_the_deadline() {
    let mut trial = WifiTrial::start(candidate(), 0);
    for now in [
        0,
        1,
        1_000,
        ASSOCIATE_DEADLINE_MS - 1,
        ASSOCIATE_DEADLINE_MS,
    ] {
        assert_eq!(
            trial.poll(now, false),
            TrialOutcome::Waiting(TrialPhase::Associating),
            "reverted at {now} ms, which is inside the association deadline",
        );
    }
}

#[test]
fn a_credential_that_never_associates_is_reverted_one_millisecond_later() {
    let mut trial = WifiTrial::start(candidate(), 0);
    assert_eq!(
        trial.poll(ASSOCIATE_DEADLINE_MS + 1, false),
        TrialOutcome::Revert(RevertReason::NotAssociated),
    );
}

#[test]
fn the_association_deadline_runs_from_the_start_not_from_zero() {
    // A trial started at a late `now_ms` — an ordinary device that has been up
    // for a while — must get its whole window, not have it measured from boot.
    let start = 9_000_000;
    let mut trial = WifiTrial::start(candidate(), start);
    assert_eq!(
        trial.poll(start + ASSOCIATE_DEADLINE_MS, false),
        TrialOutcome::Waiting(TrialPhase::Associating),
    );
    assert_eq!(
        trial.poll(start + ASSOCIATE_DEADLINE_MS + 1, false),
        TrialOutcome::Revert(RevertReason::NotAssociated),
    );
}

// ---------------------------------------------------------------------------
// The credential works and the operator does not arrive
//
// This is the case an "associated within N seconds" guard cannot see, and the
// reason this module is a dead man's handle rather than a link check.
// ---------------------------------------------------------------------------

#[test]
fn associating_moves_to_awaiting_confirmation_and_does_not_commit() {
    let mut trial = WifiTrial::start(candidate(), 0);
    assert_eq!(
        trial.poll(500, true),
        TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation),
        "association alone must never be enough to keep a credential",
    );
    assert_eq!(trial.phase(), TrialPhase::AwaitingConfirmation);
}

#[test]
fn an_unconfirmed_credential_survives_up_to_the_confirmation_deadline() {
    let mut trial = associated_at_1000();
    for offset in [0, 1, 60_000, CONFIRM_DEADLINE_MS - 1, CONFIRM_DEADLINE_MS] {
        assert_eq!(
            trial.poll(1_000 + offset, true),
            TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation),
            "reverted {offset} ms into the confirmation window",
        );
    }
}

#[test]
fn an_unconfirmed_credential_is_reverted_one_millisecond_later() {
    let mut trial = associated_at_1000();
    assert_eq!(
        trial.poll(1_000 + CONFIRM_DEADLINE_MS + 1, true),
        TrialOutcome::Revert(RevertReason::NotConfirmed),
    );
}

#[test]
fn the_confirmation_window_restarts_when_the_link_comes_up() {
    // The operator's window is theirs alone. A station that took 40 seconds to
    // associate must not leave them 40 seconds short.
    let mut trial = WifiTrial::start(candidate(), 0);
    let joined = ASSOCIATE_DEADLINE_MS - 5_000;
    assert_eq!(
        trial.poll(joined, true),
        TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation),
    );
    assert_eq!(
        trial.poll(joined + CONFIRM_DEADLINE_MS, true),
        TrialOutcome::Waiting(TrialPhase::AwaitingConfirmation),
    );
    assert_eq!(
        trial.poll(joined + CONFIRM_DEADLINE_MS + 1, true),
        TrialOutcome::Revert(RevertReason::NotConfirmed),
    );
}

// ---------------------------------------------------------------------------
// The operator arrives
// ---------------------------------------------------------------------------

#[test]
fn a_confirmed_credential_commits() {
    let mut trial = associated_at_1000();
    trial.confirm();
    assert_eq!(trial.poll(2_000, true), TrialOutcome::Commit);
}

#[test]
fn confirming_twice_is_the_same_as_confirming_once() {
    let mut trial = associated_at_1000();
    trial.confirm();
    trial.confirm();
    assert_eq!(trial.poll(2_000, true), TrialOutcome::Commit);
}

#[test]
fn the_committed_credential_is_the_one_that_was_tried() {
    let mut trial = WifiTrial::start(candidate(), 0);
    trial.poll(1_000, true);
    trial.confirm();
    assert_eq!(trial.poll(1_100, true), TrialOutcome::Commit);
    assert_eq!(trial.candidate().ssid(), "example-network");
    assert_eq!(trial.candidate().psk(), "PLACEHOLDER_PASSPHRASE");
}

// ---------------------------------------------------------------------------
// Endings that beat each other
//
// Each of these is a way the guard could be defeated by two things happening at
// once, so each is an ordering rule rather than a case.
// ---------------------------------------------------------------------------

#[test]
fn a_confirmation_cannot_commit_a_credential_the_station_has_fallen_off() {
    let mut trial = associated_at_1000();
    trial.confirm();
    assert_eq!(
        trial.poll(2_000, false),
        TrialOutcome::Revert(RevertReason::LinkLost),
        "a click that lands as the link drops must not keep the credential",
    );
}

#[test]
fn a_dropped_link_is_reported_as_lost_rather_than_as_never_associated() {
    let mut trial = associated_at_1000();
    assert_eq!(
        trial.poll(2_000, false),
        TrialOutcome::Revert(RevertReason::LinkLost),
    );
}

#[test]
fn cancelling_reverts_at_once_and_beats_a_deadline_in_the_same_millisecond() {
    let mut trial = WifiTrial::start(candidate(), 0);
    trial.cancel();
    assert_eq!(
        trial.poll(ASSOCIATE_DEADLINE_MS + 1, false),
        TrialOutcome::Revert(RevertReason::Cancelled),
    );
}

#[test]
fn cancelling_beats_an_unpolled_confirmation() {
    // Only one of the two is reversible afterwards, so the reversible one wins.
    let mut trial = associated_at_1000();
    trial.confirm();
    trial.cancel();
    assert_eq!(
        trial.poll(2_000, true),
        TrialOutcome::Revert(RevertReason::Cancelled),
    );
}

#[test]
fn cancelling_while_associating_reverts_before_the_station_ever_joins() {
    let mut trial = WifiTrial::start(candidate(), 0);
    trial.cancel();
    assert_eq!(
        trial.poll(10, false),
        TrialOutcome::Revert(RevertReason::Cancelled),
    );
}

// ---------------------------------------------------------------------------
// Countdown and clock behaviour
// ---------------------------------------------------------------------------

#[test]
fn remaining_counts_down_the_phase_in_force() {
    let mut trial = WifiTrial::start(candidate(), 1_000);
    assert_eq!(trial.remaining_ms(1_000), ASSOCIATE_DEADLINE_MS);
    assert_eq!(trial.remaining_ms(6_000), ASSOCIATE_DEADLINE_MS - 5_000);

    trial.poll(6_000, true);
    assert_eq!(trial.remaining_ms(6_000), CONFIRM_DEADLINE_MS);
    assert_eq!(trial.remaining_ms(7_000), CONFIRM_DEADLINE_MS - 1_000);
}

#[test]
fn remaining_saturates_at_zero_rather_than_wrapping() {
    let trial = WifiTrial::start(candidate(), 0);
    assert_eq!(trial.remaining_ms(ASSOCIATE_DEADLINE_MS * 10), 0);
}

#[test]
fn a_clock_that_goes_backwards_does_not_wrap_the_deadline() {
    // `saturating_sub`, not `-`. A poll with a `now_ms` behind the phase start
    // should read as "no time has passed", not as "four billion milliseconds
    // have passed", which would revert instantly.
    let mut trial = WifiTrial::start(candidate(), 10_000);
    assert_eq!(
        trial.poll(0, false),
        TrialOutcome::Waiting(TrialPhase::Associating),
    );
    assert_eq!(trial.remaining_ms(0), ASSOCIATE_DEADLINE_MS);
}

// ---------------------------------------------------------------------------
// The candidate went through the ordinary validation before it got here
// ---------------------------------------------------------------------------

#[test]
fn a_trial_cannot_be_started_with_a_credential_the_store_would_refuse() {
    // Not a property of `WifiTrial` — it is a property of its argument type,
    // and that is the point: there is no way to trial a value flash could not
    // then hold, because the only way to build one is through the constructor
    // that refuses.
    assert_eq!(
        WifiCredentials::new("", "PLACEHOLDER_PASSPHRASE"),
        Err(CredentialError::Empty(Field::Ssid)),
    );
    assert_eq!(
        WifiCredentials::new("example-network", "short"),
        Err(CredentialError::TooShort {
            field: Field::Psk,
            len: 5,
            limit: 8,
        }),
    );
}

#[test]
fn an_open_network_is_a_credential_a_trial_can_carry() {
    let open = WifiCredentials::new("example-open", "").expect("an open network is legal");
    let mut trial = WifiTrial::start(open, 0);
    trial.poll(100, true);
    trial.confirm();
    assert_eq!(trial.poll(200, true), TrialOutcome::Commit);
    assert!(trial.candidate().is_open());
}
