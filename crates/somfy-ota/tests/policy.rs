//! The two decisions: what to do about the state an image booted with, and
//! what the self-test is allowed to fail an update over.

use somfy_ota::selftest::{Leg, LegState, SelfTest, SelfTestOutcome, WINDOW_MS};
use somfy_ota::verdict::{boot_verdict, BootVerdict, ImageState, RollBackReason};

// ---------------------------------------------------------------------------
// The boot verdict
// ---------------------------------------------------------------------------

#[test]
fn a_board_that_has_never_taken_an_update_has_nothing_to_verify() {
    // Every board starts here: `otadata` blank, flashed over serial. It must
    // not run a self-test, because there is no previous image to roll back to.
    assert_eq!(
        boot_verdict(ImageState::Absent, 0),
        BootVerdict::Settled,
        "a blank otadata is not an unconfirmed update",
    );
    // And the attempt count is irrelevant to it, which matters because the
    // count is zeroed only by a power cycle: a board that panics for an
    // unrelated reason must not start rolling back images it never took.
    assert_eq!(boot_verdict(ImageState::Absent, 7), BootVerdict::Settled);
}

#[test]
fn an_already_confirmed_image_is_left_alone_however_often_it_restarts() {
    for attempts in [0u32, 1, 2, 100] {
        assert_eq!(
            boot_verdict(ImageState::Valid, attempts),
            BootVerdict::Settled
        );
        assert_eq!(
            boot_verdict(ImageState::Undefined, attempts),
            BootVerdict::Settled,
            "Undefined is what this firmware's own seeding writes and claims nothing",
        );
    }
}

#[test]
fn the_first_boot_of_a_new_image_runs_the_self_test() {
    assert_eq!(boot_verdict(ImageState::New, 0), BootVerdict::Verify);
}

#[test]
fn pending_verify_on_a_first_attempt_is_a_first_boot_and_not_a_failure() {
    // **This is the case the whole design turns on.** A bootloader built with
    // rollback enabled promotes `New` to `PendingVerify` before the image runs,
    // so a healthy first boot can find `PendingVerify` there. Reading that as
    // "a previous attempt failed" would roll back every single update on such a
    // bootloader, which is why the attempt count and not the state field is
    // what distinguishes them.
    assert_eq!(
        boot_verdict(ImageState::PendingVerify, 0),
        BootVerdict::Verify
    );
}

#[test]
fn a_second_attempt_at_an_unconfirmed_image_rolls_back() {
    // The crash case: the image reached its soak, wrote its mark, panicked, and
    // the panic handler reset. The count survives a software reset, so this
    // boot knows it is the second.
    for state in [ImageState::New, ImageState::PendingVerify] {
        assert_eq!(
            boot_verdict(state, 1),
            BootVerdict::RollBack(RollBackReason::AttemptExhausted),
            "{state:?} on a repeat attempt should roll back",
        );
        assert_eq!(
            boot_verdict(state, 9),
            BootVerdict::RollBack(RollBackReason::AttemptExhausted),
        );
    }
}

#[test]
fn a_state_the_bootloader_or_an_earlier_self_test_condemned_rolls_back_at_once() {
    // Neither of these should be reachable — a roll-back switches the active
    // slot in the same breath as writing `Invalid`, and a bootloader that
    // writes `Aborted` also moves on — so reaching them means the switch did
    // not take. Retrying is the only safe reading, and it costs one boot.
    assert_eq!(
        boot_verdict(ImageState::Invalid, 0),
        BootVerdict::RollBack(RollBackReason::MarkedInvalid),
    );
    assert_eq!(
        boot_verdict(ImageState::Aborted, 0),
        BootVerdict::RollBack(RollBackReason::MarkedAborted),
    );
}

#[test]
fn a_condemned_image_is_switched_away_from_once_per_power_on_and_not_forever() {
    // **The reset loop this bound exists to prevent.** If the switch itself
    // cannot land — the flash refuses it, or it lands on a record that is also
    // condemned — then "always roll back on `Invalid`" is a board that resets
    // every few seconds, rewrites `otadata` three times per iteration, and
    // never reaches its executor. That is a brick, and a brick is worse than
    // running an image whose own record calls it bad, because the second one
    // can be reached over the network and replaced.
    for state in [ImageState::Invalid, ImageState::Aborted] {
        assert!(
            matches!(boot_verdict(state, 0), BootVerdict::RollBack(_)),
            "{state:?} should be switched away from once",
        );
        for attempts in [1u32, 2, 40] {
            assert_eq!(
                boot_verdict(state, attempts),
                BootVerdict::Settled,
                "{state:?} on attempt {attempts} must stop resetting the board",
            );
        }
    }
}

#[test]
fn a_power_cycle_re_arms_one_more_roll_back_attempt() {
    // The counter lives in memory a power-on reset clears, so the operator's
    // one available remedy — pull the power — buys another try at the switch.
    // Expressed here as the property rather than the mechanism: attempts zero
    // is the state a power cycle produces, and it rolls back.
    assert!(matches!(
        boot_verdict(ImageState::Invalid, 0),
        BootVerdict::RollBack(_)
    ));
}

// ---------------------------------------------------------------------------
// The self-test
// ---------------------------------------------------------------------------

/// Every local leg answered.
fn healthy() -> SelfTest {
    SelfTest {
        radio: LegState::Passed,
        stores: LegState::Passed,
        network: LegState::Passed,
        associated: true,
    }
}

#[test]
fn a_healthy_image_is_confirmed_only_once_the_whole_window_has_run() {
    let test = healthy();
    assert_eq!(test.poll(0), SelfTestOutcome::Waiting);
    assert_eq!(test.poll(WINDOW_MS - 1), SelfTestOutcome::Waiting);
    assert_eq!(
        test.poll(WINDOW_MS),
        SelfTestOutcome::Pass { associated: true }
    );
}

#[test]
fn a_network_that_came_up_does_not_shorten_the_soak() {
    // **The point of the window is not to wait for the network.** It is to give
    // the image time to fall over, and the most dangerous thing this firmware
    // does — the retained discovery burst that is the heap's high-water mark —
    // happens *after* the link comes up. Confirming on association would
    // confirm about ten seconds before it.
    let associated = healthy();
    let mut alone = healthy();
    alone.associated = false;
    for at in [0u64, 1_000, 30_000, WINDOW_MS - 1] {
        assert_eq!(associated.poll(at), SelfTestOutcome::Waiting);
        assert_eq!(alone.poll(at), SelfTestOutcome::Waiting);
    }
}

#[test]
fn a_board_whose_access_point_is_down_is_still_confirmed() {
    // The regression this rule exists to prevent, stated as a test: this
    // estate's access point vanished for a stretch on 2026-08-17 and the
    // firmware retried with backoff, which is correct behaviour. An update
    // taken in that window must not be discarded over it.
    let mut test = healthy();
    test.associated = false;
    assert_eq!(
        test.poll(WINDOW_MS),
        SelfTestOutcome::Pass { associated: false },
        "a router reboot is not evidence against a release",
    );
}

#[test]
fn a_board_with_no_credentials_is_still_confirmed() {
    // A radio-only controller has no network to bring up. Refusing it an update
    // would make the one configuration that needs no network the one that
    // cannot be updated.
    let mut test = healthy();
    test.network = LegState::Skipped;
    test.associated = false;
    assert_eq!(
        test.poll(WINDOW_MS),
        SelfTestOutcome::Pass { associated: false }
    );
}

#[test]
fn a_failed_leg_does_not_wait_out_the_window() {
    // There is nothing to be gained by soaking an image whose radio did not
    // answer, and ninety seconds of it is ninety seconds the operator spends
    // wondering.
    let mut test = healthy();
    test.radio = LegState::Failed;
    assert_eq!(test.poll(0), SelfTestOutcome::Fail(Leg::Radio));
    assert_eq!(test.poll(WINDOW_MS * 10), SelfTestOutcome::Fail(Leg::Radio));
}

#[test]
fn each_leg_can_fail_the_update_on_its_own() {
    for (leg, set) in [(Leg::Radio, 0usize), (Leg::Stores, 1), (Leg::Network, 2)] {
        let mut test = healthy();
        match set {
            0 => test.radio = LegState::Failed,
            1 => test.stores = LegState::Failed,
            _ => test.network = LegState::Failed,
        }
        assert_eq!(test.poll(WINDOW_MS), SelfTestOutcome::Fail(leg));
    }
}

#[test]
fn a_leg_that_never_reported_neither_passes_nor_fails() {
    // Pending is not the same as failed, and confirming over it would mean
    // marking an image valid on evidence that was never gathered. The image
    // then stays unconfirmed, which the next reset turns into a roll-back —
    // slow, and the right direction.
    let mut test = healthy();
    test.stores = LegState::Pending;
    assert_eq!(test.poll(WINDOW_MS), SelfTestOutcome::Waiting);
    assert_eq!(test.poll(WINDOW_MS * 100), SelfTestOutcome::Waiting);
}

#[test]
fn the_window_outlasts_the_association_deadline_this_project_already_uses() {
    // 45,000 ms is `somfy_config::trial::ASSOCIATE_DEADLINE_MS` — the figure
    // this repository already treats as "long enough to associate on a network
    // that is working". The soak has to be longer than that *and* leave room
    // for the broker session behind it, or it would end before the part of the
    // boot most likely to kill a bad release.
    const {
        assert!(
            WINDOW_MS >= 2 * 45_000,
            "the soak must outlast association plus the broker's announcement burst",
        )
    };
}
