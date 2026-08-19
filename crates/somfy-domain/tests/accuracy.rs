//! The position-accuracy requirements, as tests.
//!
//! Every case here traces to a numbered requirement in
//! `docs/specs/2026-08-15-position-accuracy-requirements.md`, and most of them
//! trace to the same afternoon: on 2026-08-17 a command for 25% open moved a
//! shade about 1%, on three shades all carrying travel times nobody had ever
//! chosen. What the file is really pinning is the set of things that were wrong
//! at once, so that fixing one of them cannot look like fixing them all.
//!
//! The control case runs through all of it: **with nothing measured, every
//! refinement below reduces to the flat linear model it refines.** That is what
//! makes this a strict improvement rather than a different set of errors, and it
//! is asserted directly rather than left as an argument.

use heapless::Vec;
use somfy_domain::{
    round_dead_band_ms, round_start_lag_ms, CalibrationLeg, CalibrationMark, CalibrationOutcome,
    CalibrationSource, Controller, DomainError, Motion, PlannedTx, Pos, Repeats, Shade,
    ShadeCommand, ShadeConfig, ShadeId, StateDelta, TravelProfile, DELTA_CAPACITY, MAX_ACTIVITIES,
    MAX_DEAD_BAND_MS, MAX_START_LAG_MS, MAX_TRAVEL_TIME_MS, ROUTE_VIA_LIMIT_RAW, STOP_REPEATS,
    TX_CAPACITY,
};
use somfy_rts::{Command, Frame};

const ADDRESS: u32 = 0x12_3456;

/// The estate's own numbers: 30 s up, 27 s down — measured by hand on
/// 2026-08-17 — with the 4 s slat-separation band the owner described and a
/// start lag of about one 56-bit burst's air time.
const UP_MS: u32 = 30_000;
const DOWN_MS: u32 = 27_000;
const VENT_BAND_MS: u16 = 4_000;
const START_LAG_MS: u16 = 100;

fn config() -> ShadeConfig {
    ShadeConfig::new("Bedroom window", ADDRESS).unwrap()
}

/// The estate's measured numbers, as a configuration.
fn measured_config() -> ShadeConfig {
    let mut config = config();
    config.up_time_ms = UP_MS;
    config.down_time_ms = DOWN_MS;
    config.up_time_source = CalibrationSource::Measured;
    config.down_time_source = CalibrationSource::Measured;
    config.start_lag_ms = START_LAG_MS;
    config.vent_band_ms = VENT_BAND_MS;
    config
}

/// A shade with the estate's measured numbers.
fn measured() -> Shade {
    Shade::new(measured_config())
}

/// A shade exactly as it comes out of the constructor: factory travel times and
/// no compensation at all. The control case.
fn uncalibrated() -> Shade {
    Shade::new(config())
}

fn tx(out: &Vec<PlannedTx, 4>) -> std::vec::Vec<Command> {
    out.iter().map(|frame| frame.command).collect()
}

fn repeats(out: &Vec<PlannedTx, 4>) -> std::vec::Vec<Repeats> {
    out.iter().map(|frame| frame.repeats).collect()
}

/// Run `shade` forward to `until_ms`, ticking every 100 ms and collecting every
/// frame planned along the way.
///
/// A tick rate rather than one long jump, because the arrival stop now fires on
/// a *lead* — it goes out a start lag before the estimate reaches the target —
/// and a single tick from zero to the end would skip straight past the window
/// that is being tested.
fn run(shade: &mut Shade, from_ms: u64, until_ms: u64) -> std::vec::Vec<(u64, Command)> {
    let mut seen = std::vec::Vec::new();
    let mut now = from_ms;
    while now <= until_ms {
        let mut out: Vec<PlannedTx, 4> = Vec::new();
        shade.tick(now, &mut out);
        for frame in &out {
            seen.push((now, frame.command));
        }
        now += 100;
    }
    seen
}

/// One controller holding one shade, for the cases that need the *whole* path.
///
/// The multi-step movements — a vent, a seek routed via a limit, a calibration
/// run — are stored on the controller rather than on each shade, because a byte
/// per shade is about a hundred and seventy bytes of boot stack on the device
/// this runs on (`somfy_domain::MAX_ACTIVITIES`). So they can only be exercised
/// through the controller, and this is that.
struct Rig {
    controller: Controller,
    id: ShadeId,
    tx: Vec<PlannedTx, TX_CAPACITY>,
    deltas: Vec<StateDelta, DELTA_CAPACITY>,
}

impl Rig {
    fn new(config: ShadeConfig) -> Rig {
        let mut controller = Controller::new();
        let id = controller.registry.add_shade(config).unwrap();
        Rig {
            controller,
            id,
            tx: Vec::new(),
            deltas: Vec::new(),
        }
    }

    /// A shade with the estate's measured numbers.
    fn measured() -> Rig {
        Rig::new(measured_config())
    }

    fn shade(&self) -> &Shade {
        self.controller.registry.shade(self.id).expect("the shade")
    }

    /// Apply one command and return the frames it planned.
    fn command(&mut self, cmd: ShadeCommand, now_ms: u64) -> std::vec::Vec<Command> {
        self.tx.clear();
        self.deltas.clear();
        self.controller
            .command_shade(self.id, cmd, now_ms, &mut self.tx, &mut self.deltas)
            .expect("accepted");
        self.tx.iter().map(|frame| frame.command).collect()
    }

    fn begin(&mut self, leg: CalibrationLeg, now_ms: u64) {
        self.tx.clear();
        self.deltas.clear();
        self.controller
            .begin_calibration(self.id, leg, now_ms, &mut self.tx, &mut self.deltas)
            .expect("accepted");
    }

    fn mark(&mut self, mark: CalibrationMark, now_ms: u64) -> Result<(), DomainError> {
        self.controller.mark_calibration(self.id, mark, now_ms)
    }

    fn finish(&mut self, now_ms: u64) -> Result<CalibrationOutcome, DomainError> {
        self.deltas.clear();
        self.controller
            .finish_calibration(self.id, now_ms, &mut self.deltas)
    }

    /// Tick every 100 ms and collect every frame planned along the way.
    fn run(&mut self, from_ms: u64, until_ms: u64) -> std::vec::Vec<(u64, PlannedTx)> {
        let mut seen = std::vec::Vec::new();
        let mut now = from_ms;
        while now <= until_ms {
            self.tx.clear();
            // Drained every call, as the firmware's state task drains it: the
            // buffer is sized to one call's worth, not to a whole run.
            self.deltas.clear();
            self.controller.tick(now, &mut self.tx, &mut self.deltas);
            for frame in &self.tx {
                seen.push((now, *frame));
            }
            now += 100;
        }
        seen
    }
}

/// The commands out of a [`Rig::run`], without their timestamps.
fn commands(seen: &[(u64, PlannedTx)]) -> std::vec::Vec<Command> {
    seen.iter().map(|(_, frame)| frame.command).collect()
}

// ---------------------------------------------------------------------------
// R1 — the arrival stop is transmitted harder than an ordinary command
// ---------------------------------------------------------------------------

/// Acceptance criterion 1.
///
/// The `My` that ends a mid-range seek is the single point of failure in the
/// whole position system: a motor self-stops only at its own end stops, so if
/// that one frame is lost nothing else will ever tell it to stop.
#[test]
fn a_midrange_arrival_stop_is_planned_harder_than_the_command_that_started_it() {
    let mut shade = measured();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(60)), 0, &mut out);
    assert_eq!(tx(&out), [Command::Down]);
    assert_eq!(
        repeats(&out),
        [Repeats::Profile],
        "an ordinary command takes whatever the controller is configured for",
    );

    let seen = run(&mut shade, 0, 30_000);
    assert!(
        seen.iter().any(|(_, command)| *command == Command::My),
        "a mid-range seek must end with a stop frame",
    );

    let mut out: Vec<PlannedTx, 4> = Vec::new();
    let mut shade = measured();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(60)), 0, &mut out);
    out.clear();
    // Far enough in that the stop is due.
    for now in (0..30_000).step_by(100) {
        shade.tick(now, &mut out);
        if !out.is_empty() {
            break;
        }
    }
    assert_eq!(repeats(&out), [Repeats::AtLeast(STOP_REPEATS)]);
}

/// The floor is strictly above what an ordinary command resolves to, and
/// strictly below the burst lengths that mean something else on a real motor:
/// a held `My` stores a favourite, and a long press is a tilt on a tilt-capable
/// one.
#[test]
fn the_stop_floor_sits_between_an_ordinary_press_and_the_bursts_that_mean_something_else() {
    // `somfy_tasks::DEFAULT_REPEATS`, restated: this crate owns no repeat count.
    const ORDINARY: u8 = 2;
    // What deployed controllers read as a tilt press, and as "store the
    // favourite here". Both are documented in `docs/provenance.md`.
    const TILT_PRESS: u8 = 15;
    const SET_FAVOURITE: u8 = 35;

    // Const blocks: these are relationships between compile-time constants, so
    // a violation should stop the build rather than wait for a test run.
    const { assert!(STOP_REPEATS > ORDINARY) };
    const { assert!(STOP_REPEATS < TILT_PRESS) };
    const { assert!(STOP_REPEATS < SET_FAVOURITE) };
    assert_eq!(
        Repeats::AtLeast(STOP_REPEATS).resolve(ORDINARY),
        STOP_REPEATS
    );
    assert_eq!(
        Repeats::AtLeast(STOP_REPEATS).resolve(9),
        9,
        "a generously configured controller sends the most important frame at \
         least as hard as an ordinary one",
    );
}

/// A seek to a hard limit still plans no stop: the motor stops itself there, and
/// a `My` at a limit is a favourite recall rather than a stop.
#[test]
fn a_seek_to_a_limit_plans_no_stop_however_hard_the_floor_is() {
    let mut shade = measured();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::FULL), 0, &mut out);
    let seen = run(&mut shade, 0, 40_000);
    assert!(seen.iter().all(|(_, command)| *command != Command::My));
}

// ---------------------------------------------------------------------------
// R2 — the two directions are measured, and measured independently
// ---------------------------------------------------------------------------

/// Acceptance criterion 2: with `up_time_ms != down_time_ms`, the time to
/// traverse the same span differs by the same ratio.
///
/// The estate's 30/27 asymmetry is real — closing is gravity-assisted — which is
/// why a calibration that measured one direction and mirrored it would be wrong
/// by 10%.
#[test]
fn asymmetric_travel_times_produce_asymmetric_estimates() {
    let profile = TravelProfile::linear(UP_MS, DOWN_MS);

    let mut down = Motion::new(Pos::ZERO);
    down.set_target(Pos::FULL, 0);
    let mut up = Motion::new(Pos::FULL);
    up.set_target(Pos::ZERO, 0);

    // Half the span each way.
    let half_down = (DOWN_MS / 2) as u64;
    let half_up = (UP_MS / 2) as u64;
    assert_eq!(down.tick(half_down, profile).pos, Pos::from_percent(50));
    assert_eq!(up.tick(half_up, profile).pos, Pos::from_percent(50));

    // The ratio of the two half-times is the ratio of the two travel times.
    assert_eq!(half_up * DOWN_MS as u64, half_down * UP_MS as u64);
}

/// A guided run measures one direction, stores it as **measured**, and leaves
/// the other exactly as it was.
#[test]
fn a_calibration_run_measures_one_direction_and_leaves_the_other_alone() {
    let mut rig = Rig::new(config());
    rig.begin(CalibrationLeg::Up, 0);
    assert_eq!(
        rig.tx
            .iter()
            .map(|f| f.command)
            .collect::<std::vec::Vec<_>>(),
        [Command::Up]
    );
    assert!(rig.controller.is_calibrating(rig.id));

    let outcome = rig.finish(UP_MS as u64).unwrap();
    assert_eq!(outcome.leg, CalibrationLeg::Up);
    assert_eq!(outcome.travel_ms, UP_MS);
    assert_eq!(rig.shade().config.up_time_ms, UP_MS);
    assert_eq!(
        rig.shade().config.up_time_source,
        CalibrationSource::Measured
    );
    assert_eq!(
        rig.shade().config.down_time_source,
        CalibrationSource::FactoryDefault,
        "the down direction was never measured and must not claim to have been",
    );
    assert!(!rig.controller.is_calibrating(rig.id));
}

/// The two runs together give the estate's numbers, and neither is derived from
/// the other.
#[test]
fn both_directions_can_be_measured_and_neither_is_mirrored() {
    let mut rig = Rig::new(config());

    rig.begin(CalibrationLeg::Up, 0);
    rig.finish(UP_MS as u64).unwrap();
    rig.begin(CalibrationLeg::Down, 100_000);
    rig.finish(100_000 + DOWN_MS as u64).unwrap();

    assert_eq!(rig.shade().config.up_time_ms, UP_MS);
    assert_eq!(rig.shade().config.down_time_ms, DOWN_MS);
    assert_ne!(
        rig.shade().config.up_time_ms,
        rig.shade().config.down_time_ms
    );
}

/// A run whose numbers the model cannot hold stores **nothing**: a half-applied
/// calibration — a new up time against an old band — is worse than none.
#[test]
fn an_implausible_run_leaves_the_shade_exactly_as_it_was() {
    let mut rig = Rig::measured();
    let before = rig.shade().config.clone();

    rig.begin(CalibrationLeg::Up, 0);
    assert_eq!(
        rig.finish(0),
        Err(DomainError::CalibrationImplausible),
        "a traverse of zero has no scale",
    );

    rig.begin(CalibrationLeg::Up, 0);
    assert_eq!(
        rig.finish(MAX_TRAVEL_TIME_MS as u64 + 1),
        Err(DomainError::CalibrationImplausible),
        "a run still going after three minutes is one somebody walked away from",
    );

    rig.begin(CalibrationLeg::Up, 0);
    // A band longer than the traverse it is part of leaves nothing that lifts.
    rig.mark(CalibrationMark::CurtainMoved, 9_000).unwrap();
    assert_eq!(
        rig.finish(8_000),
        Err(DomainError::DeadBandTooLong),
        "reported as the specific rule it broke rather than as a generic refusal",
    );

    // A refused run stays open, so the operator can tap again rather than start
    // over — and cancelling is what actually ends it.
    assert!(rig.controller.is_calibrating(rig.id));
    rig.controller.cancel_calibration(rig.id).unwrap();
    assert_eq!(rig.shade().config, before);
}

#[test]
fn marking_or_finishing_without_a_run_is_refused() {
    let mut rig = Rig::measured();
    assert_eq!(
        rig.mark(CalibrationMark::MotionBegan, 10),
        Err(DomainError::NotCalibrating),
    );
    assert_eq!(rig.finish(10).unwrap_err(), DomainError::NotCalibrating);
    assert_eq!(
        rig.controller.cancel_calibration(rig.id),
        Err(DomainError::NotCalibrating),
    );
}

/// **Anything that commands the shade ends the run**, and the operator is not
/// told until the next tap.
///
/// Not a defect: a run is a stopwatch against one uninterrupted traverse, and a
/// traverse somebody else interrupted has nothing left to time. But it is a fact
/// the *screen* has to carry, because the operator is holding a device whose
/// other controls are one panel away and whose shade may also have a wall
/// remote — and the failure is silent until they report the stop.
///
/// Pinned here because the web UI now says this in words, and a claim in a
/// screen with no test behind it is the kind that quietly stops being true.
#[test]
fn a_command_from_anywhere_else_ends_the_run_silently() {
    let mut rig = Rig::measured();
    let before = rig.shade().config.clone();

    rig.begin(CalibrationLeg::Up, 0);
    // The operator taps Open on the tile above the calibration panel, or Home
    // Assistant does, or an automation does.
    rig.command(ShadeCommand::Down, 5_000);
    assert!(
        !rig.controller.is_calibrating(rig.id),
        "the command took the shade's only activity slot",
    );

    // Nothing said so at the time. It is said now, at the first tap after.
    assert_eq!(
        rig.mark(CalibrationMark::MotionBegan, 6_000),
        Err(DomainError::NotCalibrating),
    );
    assert_eq!(rig.finish(30_000).unwrap_err(), DomainError::NotCalibrating);
    assert_eq!(
        rig.shade().config,
        before,
        "and nothing was stored from the half-run",
    );
}

/// Starting a second run replaces the first rather than being refused.
///
/// The operator who mis-tapped Start, or started the wrong direction, has no
/// other way forward — a half-finished run has stored nothing, so there is no
/// state worth protecting. The screen relies on this to offer "measure the other
/// direction" without a cancel in between.
#[test]
fn beginning_a_second_run_replaces_the_first() {
    let mut rig = Rig::measured();

    rig.begin(CalibrationLeg::Up, 0);
    rig.mark(CalibrationMark::MotionBegan, 500).unwrap();
    rig.begin(CalibrationLeg::Down, 1_000);
    assert!(rig.controller.is_calibrating(rig.id));

    // The Down leg's traverse is timed from the *second* begin, and the first
    // run's mark is gone with it — otherwise a 500 ms lag measured against a
    // clock that no longer exists would be folded into this direction.
    let outcome = rig.finish(1_000 + DOWN_MS as u64).unwrap();
    assert_eq!(outcome.leg, CalibrationLeg::Down);
    assert_eq!(outcome.travel_ms, DOWN_MS);
    assert_eq!(outcome.start_lag_ms, None, "no mark carried over");
    assert_eq!(
        rig.shade().config.up_time_source,
        CalibrationSource::Measured,
        "the abandoned Up run stored nothing, so this is still the fixture's own",
    );
    assert_eq!(rig.shade().config.up_time_ms, UP_MS);
}

/// Skipping the *first* tap does not skip a number — it moves it.
///
/// With no `MotionBegan`, the band is measured against the **stored** start lag
/// rather than a fresh one, so on a shade whose lag is still zero the whole
/// command-to-motion delay lands inside the slat-separation figure. That is the
/// right arithmetic (the band is what is left of the interval after the lag) and
/// it is a surprising consequence, so the screen says it and this pins it.
#[test]
fn skipping_the_motion_tap_folds_the_start_delay_into_the_band() {
    // Two shades, identical but for the stored lag.
    let mut without = Rig::new(measured_config());
    let mut with_lag = Rig::new(measured_config());

    // Zero the lag on one of them; the fixture carries START_LAG_MS on both.
    {
        let shade = without.controller.registry.shade_mut(without.id).unwrap();
        shade.config.start_lag_ms = 0;
    }

    const CURTAIN_AT_MS: u64 = 4_200;
    for rig in [&mut without, &mut with_lag] {
        rig.begin(CalibrationLeg::Up, 0);
        rig.mark(CalibrationMark::CurtainMoved, CURTAIN_AT_MS)
            .unwrap();
        rig.finish(UP_MS as u64).unwrap();
    }

    assert_eq!(
        without.shade().config.vent_band_ms,
        round_dead_band_ms(CURTAIN_AT_MS as u32).unwrap(),
        "with no lag stored, the band is the whole interval from the command",
    );
    assert_eq!(
        with_lag.shade().config.vent_band_ms,
        round_dead_band_ms(CURTAIN_AT_MS as u32 - START_LAG_MS as u32).unwrap(),
        "with a lag stored, the band is what is left after it",
    );
    assert_eq!(
        without.shade().config.start_lag_ms,
        0,
        "an untapped moment stores nothing rather than a worse value",
    );
}

// ---------------------------------------------------------------------------
// R3 — endpoint resynchronisation
// ---------------------------------------------------------------------------

/// Acceptance criterion 3: drive the estimator to a limit with a deliberately
/// wrong travel time, then assert the reported position is exactly `ZERO`/`FULL`
/// and that the accumulated error is back to zero.
#[test]
fn reaching_a_limit_snaps_the_estimate_and_clears_the_accumulated_error() {
    let mut shade = uncalibrated();
    let mut out: Vec<PlannedTx, 4> = Vec::new();

    // A partial move on factory travel times: the estimate is now worthless, and
    // says so.
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    run(&mut shade, 0, 6_000);
    assert!(
        shade.confidence() > 0,
        "a partial move must cost confidence"
    );

    // Now close it. The travel time is wrong by a factor of three against the
    // real shade, and it does not matter: the motor stops itself at the sill.
    out.clear();
    shade.handle(ShadeCommand::Down, 6_000, &mut out);
    run(&mut shade, 6_000, 20_000);

    assert_eq!(shade.pos(), Pos::FULL);
    assert_eq!(
        shade.confidence(),
        0,
        "the motor's own end stop is the answer, whatever was calculated",
    );
}

/// The same at the other end, and through the estimator rather than through a
/// command: `Motion::resync` is what a limit event does to it.
#[test]
fn a_resync_re_anchors_the_move_that_follows_it() {
    let mut motion = Motion::new(Pos::from_percent(50));
    motion.resync(Pos::ZERO, 1_000);
    assert_eq!(motion.pos(), Pos::ZERO);
    assert_eq!(motion.target(), Pos::ZERO);

    motion.set_target(Pos::FULL, 1_000);
    let profile = TravelProfile::linear(UP_MS, DOWN_MS);
    // Half a down traverse from the *limit*, not from the stale 50%.
    let half = 1_000 + (DOWN_MS / 2) as u64;
    assert_eq!(motion.tick(half, profile).pos, Pos::from_percent(50));
}

/// With enough doubt, a go-to-position runs to a limit first and times from
/// there — and it picks the limit by worst-case cost rather than by the estimate
/// it is refusing to trust.
#[test]
fn a_doubtful_seek_routes_via_a_limit_and_ends_at_the_target() {
    let mut rig = Rig::measured();

    // A partial move on measured times accumulates some doubt; push it past the
    // threshold by abandoning one, which is what an overheard wall remote does.
    rig.command(ShadeCommand::GoTo(Pos::from_percent(50)), 0);
    rig.run(0, 5_000);
    // A wall remote's PROG press: it drops our pending stop and tells the motor
    // nothing, so after it nobody knows where the shade stopped.
    let frame = Frame {
        key: 0xA0,
        command: Command::Prog,
        rolling_code: 1,
        address: ADDRESS,
    };
    rig.controller.on_rx_frame(&frame, 5_000, &mut rig.deltas);
    assert!(rig.shade().confidence() >= ROUTE_VIA_LIMIT_RAW);

    // Target 70% closed is nearer the closed limit, and the cheaper route to it
    // is a full close followed by a short open.
    assert_eq!(
        rig.command(ShadeCommand::GoTo(Pos::from_percent(70)), 6_000),
        [Command::Down],
    );

    let seen = rig.run(6_000, 6_000 + 90_000);
    let seen = commands(&seen);
    assert!(
        seen.contains(&Command::Up),
        "the second leg times back from the limit: {seen:?}",
    );
    assert!(
        seen.contains(&Command::My),
        "and still ends with an arrival stop: {seen:?}",
    );
    assert_eq!(rig.shade().pos(), Pos::from_percent(70));
}

/// **The condition that stops this being a pessimisation.** Re-anchoring buys a
/// known starting position and nothing else, so on a shade whose travel times
/// nobody chose the leg back from the limit is as wrong as the direct seek would
/// have been — and the shade would have travelled its whole range to learn
/// nothing.
#[test]
fn an_uncalibrated_shade_never_routes_via_a_limit() {
    let mut shade = uncalibrated();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    run(&mut shade, 0, 8_000);
    assert!(
        shade.confidence() >= ROUTE_VIA_LIMIT_RAW,
        "doubtful enough that a *calibrated* shade would route via a limit",
    );

    out.clear();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(70)), 9_000, &mut out);
    assert_eq!(tx(&out), [Command::Down]);
    assert_eq!(shade.target(), Pos::from_percent(70), "a direct seek");
}

// ---------------------------------------------------------------------------
// R4 — confidence
// ---------------------------------------------------------------------------

/// Acceptance criterion 4: uncertainty is non-decreasing across partial moves
/// and returns to its floor at an endpoint.
#[test]
fn confidence_is_monotone_between_limits_and_floors_at_one() {
    let mut shade = measured();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    let mut seen = std::vec::Vec::new();
    let mut now = 0u64;

    for target in [40u8, 55, 30, 65] {
        shade.handle(ShadeCommand::GoTo(Pos::from_percent(target)), now, &mut out);
        out.clear();
        now += 40_000;
        run(&mut shade, now - 40_000, now);
        seen.push(shade.confidence());
    }

    assert!(
        seen.windows(2).all(|pair| pair[1] >= pair[0]),
        "uncertainty may not fall between limits: {seen:?}",
    );
    assert!(seen[0] > 0, "a partial move always costs something");

    shade.handle(ShadeCommand::Up, now, &mut out);
    run(&mut shade, now, now + 40_000);
    assert_eq!(shade.pos(), Pos::ZERO);
    assert_eq!(shade.confidence(), 0);
}

/// A shade whose travel times nobody chose reports the estimate as worth
/// nothing, immediately. That is the correct report rather than a defect: it is
/// the state three shades were in when a 25% command moved one of them 1%.
#[test]
fn factory_travel_times_saturate_the_doubt_on_the_first_partial_move() {
    let mut shade = uncalibrated();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(25)), 0, &mut out);
    run(&mut shade, 0, 5_000);
    // A travel time nobody chose is worth nothing, so the doubt is the whole
    // distance travelled: the shade is somewhere between fully open and half
    // closed, and the report says exactly that.
    assert_eq!(shade.confidence(), Pos::from_percent(25).raw());
    assert!(shade.confidence() >= ROUTE_VIA_LIMIT_RAW);
}

/// A measured shade doing the same move is doubtful, but only a little — which
/// is what makes the number worth surfacing rather than a constant.
#[test]
fn a_measured_shade_is_only_a_little_doubtful_after_one_partial_move() {
    let mut shade = measured();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(25)), 0, &mut out);
    run(&mut shade, 0, 12_000);
    let doubt = shade.confidence();
    assert!(doubt > 0);
    assert!(doubt < 100, "under one percent of the range, got {doubt}");
}

/// A move taken over by something this controller does not drive costs the whole
/// remaining distance, because after that nothing knows whether the motor
/// covered it.
#[test]
fn an_abandoned_move_costs_its_whole_remaining_distance() {
    let mut shade = measured();
    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(80)), 0, &mut out);
    // A wall remote's PROG press, which drops our pending stop without telling
    // the motor anything.
    shade.apply_overheard(Command::Prog, 1_000);
    assert!(shade.confidence() >= Pos::from_percent(70).raw());
}

// ---------------------------------------------------------------------------
// R5 — dead-time compensation
// ---------------------------------------------------------------------------

/// Motion does not begin when a command is planned.
#[test]
fn the_first_start_lag_of_a_move_produces_no_position_change() {
    let profile = TravelProfile {
        up_time_ms: UP_MS,
        down_time_ms: DOWN_MS,
        start_lag_ms: 1_000,
        vent_band_ms: 0,
        close_band_ms: 0,
    };
    let mut motion = Motion::new(Pos::ZERO);
    motion.set_target(Pos::FULL, 0);

    assert_eq!(motion.tick(999, profile).pos, Pos::ZERO);
    assert!(motion.tick(1_100, profile).pos > Pos::ZERO);
    // And the full traverse still takes exactly `down_time_ms`, because the lag
    // is a part of it rather than an addition to it.
    assert_eq!(motion.tick(DOWN_MS as u64, profile).pos, Pos::FULL);
}

/// **Where it matters, in the owner's own case.** A 25%-open command runs the
/// motor for a quarter of the traverse; on a short run a one-second lag is a
/// large fraction of it, and on a long one it is noise.
#[test]
fn compensating_the_lag_matters_on_a_short_run_and_not_on_a_long_one() {
    let lagged = TravelProfile {
        up_time_ms: 10_000,
        down_time_ms: 10_000,
        start_lag_ms: 1_000,
        vent_band_ms: 0,
        close_band_ms: 0,
    };
    let flat = TravelProfile::linear(10_000, 10_000);

    let at = |profile: TravelProfile, elapsed: u64| {
        let mut motion = Motion::new(Pos::ZERO);
        motion.set_target(Pos::FULL, 0);
        motion.tick(elapsed, profile).pos.raw()
    };

    // A short run: the flat model says a quarter, the compensated one says
    // rather less, because a tenth of the run has not moved anything and the
    // rest of it therefore covers the range faster.
    //
    // The gap is 834 raw units — **more than eight percentage points on a
    // command for twenty-five percent**, which is the order of the error the
    // owner reported. On the estate's real 30 s shade the same lag is a tenth
    // of that.
    let short_flat = at(flat, 2_500);
    let short_lagged = at(lagged, 2_500);
    assert!(
        short_flat - short_lagged > 500,
        "the error is large on a short run: {short_flat} against {short_lagged}",
    );

    // A long run: both are at the limit, and the difference has vanished.
    assert_eq!(at(flat, 10_000), at(lagged, 10_000));
}

/// The arrival stop is planned a lag **early**, because a `My` takes the same
/// time to reach the motor and the motor keeps travelling meanwhile.
#[test]
fn the_arrival_stop_is_planned_a_start_lag_before_the_estimate_arrives() {
    let mut shade = measured();
    // A lag well above the tick rate, so the lead is observable rather than
    // rounded away — the effect is real at 100 ms too, it is just smaller than
    // one tick.
    shade.config.start_lag_ms = 1_000;

    let mut out: Vec<PlannedTx, 4> = Vec::new();
    shade.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);

    let mut stop_at = None;
    let mut arrived_at = None;
    let mut now = 0u64;
    while now <= 40_000 {
        out.clear();
        let snapshot = shade.tick(now, &mut out);
        if stop_at.is_none() && out.iter().any(|frame| frame.command == Command::My) {
            stop_at = Some(now);
        }
        if arrived_at.is_none() && snapshot.arrived {
            arrived_at = Some(now);
        }
        now += 100;
    }

    let stop_at = stop_at.expect("a mid-range seek ends with a stop");
    let arrived_at = arrived_at.expect("and the estimate does reach the target");
    assert!(
        stop_at < arrived_at,
        "the stop goes out before the estimate arrives ({stop_at} against {arrived_at})",
    );
    // And it leads by about the lag, which is the whole point: the motor
    // travels for that long after hearing the frame.
    assert!((arrived_at - stop_at).abs_diff(1_000) <= 200);
}

// ---------------------------------------------------------------------------
// R8 — the closed-end dead band
// ---------------------------------------------------------------------------

/// Leaving the closed limit upward, the slats separate before the curtain
/// rises: measured on this estate at about 4 s of a 30 s traverse, so roughly
/// 13% of a commanded Up produces no elevation at all.
#[test]
fn leaving_the_closed_limit_upward_lifts_nothing_until_the_slats_are_apart() {
    let profile = TravelProfile {
        up_time_ms: UP_MS,
        down_time_ms: DOWN_MS,
        start_lag_ms: START_LAG_MS,
        vent_band_ms: VENT_BAND_MS,
        close_band_ms: 0,
    };
    let mut motion = Motion::new(Pos::FULL);
    motion.set_target(Pos::ZERO, 0);

    let dead = START_LAG_MS as u64 + VENT_BAND_MS as u64;
    assert_eq!(motion.tick(dead - 100, profile).pos, Pos::FULL);
    assert!(motion.tick(dead + 200, profile).pos < Pos::FULL);
    // The traverse still takes exactly `up_time_ms`.
    assert_eq!(motion.tick(UP_MS as u64, profile).pos, Pos::ZERO);
}

/// The band applies **only** from the closed limit. Anywhere else the slats are
/// already apart, and the test is exact equality with `Pos::FULL` rather than
/// nearness, so it fires on knowledge rather than on a number that happens to be
/// close.
#[test]
fn the_slat_band_does_not_apply_to_an_up_move_from_mid_range() {
    let profile = TravelProfile {
        up_time_ms: UP_MS,
        down_time_ms: DOWN_MS,
        start_lag_ms: 0,
        vent_band_ms: VENT_BAND_MS,
        close_band_ms: 0,
    };
    let mut motion = Motion::new(Pos::from_percent(99));
    motion.set_target(Pos::ZERO, 0);
    assert!(
        motion.tick(500, profile).pos < Pos::from_percent(99),
        "a shade one percent off the sill has its slats apart already",
    );
}

/// Closing, the slats compress *after* the curtain reaches the sill — so the
/// position is already `FULL` throughout that phase, and the whole close still
/// takes `down_time_ms`.
#[test]
fn the_closing_band_shortens_the_lifting_phase_without_lengthening_the_close() {
    let profile = TravelProfile {
        up_time_ms: UP_MS,
        down_time_ms: DOWN_MS,
        start_lag_ms: 0,
        vent_band_ms: 0,
        close_band_ms: 2_000,
    };
    let mut motion = Motion::new(Pos::ZERO);
    motion.set_target(Pos::FULL, 0);
    // The curtain reaches the sill early, and the rest of the close is slats.
    assert_eq!(
        motion.tick((DOWN_MS - 2_000) as u64, profile).pos,
        Pos::FULL
    );
    assert_eq!(motion.tick(DOWN_MS as u64, profile).pos, Pos::FULL);
}

// ---------------------------------------------------------------------------
// The control case: nothing measured reduces to the model this refines
// ---------------------------------------------------------------------------

/// **The property that makes all of the above a strict refinement.**
///
/// With every compensation at zero — which is what a shade is created with and
/// what a migrated one carries — the estimator is exactly the flat linear model
/// it started as. Nothing moves until something is measured.
#[test]
fn with_nothing_measured_the_model_is_the_flat_one_it_refines() {
    let fresh = config();
    let flat = TravelProfile::linear(fresh.up_time_ms, fresh.down_time_ms);
    assert_eq!(fresh.travel(), flat);
    assert_eq!(fresh.travel().up_span_ms(), fresh.up_time_ms);
    assert_eq!(fresh.travel().down_span_ms(), fresh.down_time_ms);

    for elapsed in [0u64, 1, 500, 5_000, 13_500, 27_000, 60_000] {
        let mut refined = Motion::new(Pos::ZERO);
        let mut control = Motion::new(Pos::ZERO);
        refined.set_target(Pos::FULL, 0);
        control.set_target(Pos::FULL, 0);
        assert_eq!(
            refined.tick(elapsed, fresh.travel()),
            control.tick(elapsed, flat),
            "at {elapsed} ms",
        );
    }
}

/// The bands and the lag have to leave real travel behind them: they are parts
/// of a traverse, not additions to it.
#[test]
fn a_band_that_consumes_its_whole_traverse_is_refused() {
    let mut config = config();
    config.up_time_ms = 5_000;
    config.vent_band_ms = 5_000;
    assert_eq!(config.checked_bands(), Err(DomainError::DeadBandTooLong));

    config.vent_band_ms = 4_900;
    assert_eq!(config.checked_bands(), Ok(()));

    // A zero travel time is a different complaint with its own message, so this
    // check passes it over rather than claiming the band ate it.
    let mut empty = ShadeConfig::new("x", ADDRESS).unwrap();
    empty.up_time_ms = 0;
    empty.down_time_ms = 0;
    assert_eq!(empty.checked_bands(), Ok(()));
}

/// The resolutions are the measurement's, and a value past what the model can
/// express is refused rather than clamped — a silently substituted ceiling is a
/// position estimate computed from a number nobody entered.
#[test]
fn lag_and_band_values_round_to_their_resolution_and_refuse_what_is_past_it() {
    assert_eq!(round_start_lag_ms(104), Some(100));
    assert_eq!(round_start_lag_ms(105), Some(110));
    assert_eq!(round_start_lag_ms(MAX_START_LAG_MS), Some(2_550));
    assert_eq!(round_start_lag_ms(MAX_START_LAG_MS + 1), None);

    assert_eq!(round_dead_band_ms(4_049), Some(4_000));
    assert_eq!(round_dead_band_ms(4_050), Some(4_100));
    assert_eq!(round_dead_band_ms(MAX_DEAD_BAND_MS), Some(25_500));
    assert_eq!(round_dead_band_ms(MAX_DEAD_BAND_MS + 1), None);
}

// ---------------------------------------------------------------------------
// The vent command
// ---------------------------------------------------------------------------

/// The whole sequence, in order, on the clock — and the thing that makes it
/// trustworthy is that **no step consults the position estimate**.
#[test]
fn a_vent_closes_fully_then_opens_for_the_measured_band_then_stops() {
    let mut rig = Rig::measured();

    assert_eq!(
        rig.command(ShadeCommand::Vent, 0),
        [Command::Down],
        "step one is a full close",
    );

    // Nothing more until the motor has had a whole `down_time_ms`.
    let during = rig.run(100, DOWN_MS as u64 - 100);
    assert!(during.is_empty(), "the wait is the anchor: {during:?}");

    let after = rig.run(DOWN_MS as u64, DOWN_MS as u64 + 10_000);
    assert_eq!(commands(&after), [Command::Up, Command::My]);

    // The Up leg runs for exactly the lag plus the measured band.
    let ran_for = after[1].0 - after[0].0;
    let expected = START_LAG_MS as u64 + VENT_BAND_MS as u64;
    assert!(
        ran_for.abs_diff(expected) <= 100,
        "the Up leg ran {ran_for} ms, expected about {expected}",
    );

    // The curtain never rose — that is what a vent *is* — so the lift estimate
    // is the closed limit, and it is known rather than calculated.
    assert_eq!(rig.shade().pos(), Pos::FULL);
    assert_eq!(rig.shade().confidence(), 0);
}

/// The vent's own stop is an arrival stop and is transmitted as one: losing it
/// leaves the shade opening all the way.
#[test]
fn the_vent_stop_is_transmitted_at_the_arrival_stop_floor() {
    let mut rig = Rig::measured();
    rig.command(ShadeCommand::Vent, 0);
    let seen = rig.run(0, DOWN_MS as u64 + 10_000);
    let stop = seen
        .iter()
        .find(|(_, frame)| frame.command == Command::My)
        .expect("a vent ends with a stop");
    assert_eq!(stop.1.repeats, Repeats::AtLeast(STOP_REPEATS));
}

/// **A shade the estimate already calls closed still runs the full close.**
///
/// That is the design rather than an oversight: the estimate is the one thing
/// this command exists not to depend on, so skipping the leg on its say-so would
/// give back exactly what the design bought. The cost is accepted — a shade
/// already open travels its whole range down first.
#[test]
fn a_shade_already_closed_still_waits_out_the_whole_down_leg() {
    let mut rig = Rig::measured();

    // Put it at the closed limit, for real.
    rig.command(ShadeCommand::Down, 0);
    rig.run(0, DOWN_MS as u64 + 1_000);
    assert_eq!(rig.shade().pos(), Pos::FULL);

    let started = DOWN_MS as u64 + 1_000;
    assert_eq!(
        rig.command(ShadeCommand::Vent, started),
        [Command::Down],
        "it closes again regardless",
    );

    let during = rig.run(started + 100, started + DOWN_MS as u64 - 100);
    assert!(during.is_empty(), "{during:?}");
    let after = rig.run(started + DOWN_MS as u64, started + DOWN_MS as u64 + 10_000);
    assert_eq!(commands(&after), [Command::Up, Command::My]);
}

/// A vent with no measured band is refused before anything is planned. The vent
/// position *is* that number, so with nothing measured there is nothing to aim
/// at, and a vent that ran anyway would look like a button that does nothing.
#[test]
fn a_vent_is_refused_while_the_slat_band_has_never_been_measured() {
    let mut controller = Controller::new();
    let id = controller.registry.add_shade(config()).unwrap();
    let mut planned: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
    let mut deltas: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();

    assert_eq!(
        controller.command_shade(id, ShadeCommand::Vent, 0, &mut planned, &mut deltas),
        Err(DomainError::VentBandNotMeasured),
    );
    assert!(planned.is_empty(), "nothing may be transmitted");
}

/// A stop halts a vent in progress, because a person watching the shade expects
/// it to.
#[test]
fn a_stop_abandons_a_vent_in_progress() {
    let mut rig = Rig::measured();
    rig.command(ShadeCommand::Vent, 0);
    assert_eq!(rig.command(ShadeCommand::My, 5_000), [Command::My]);

    let after = rig.run(5_000, 5_000 + DOWN_MS as u64 + 10_000);
    assert!(
        after.is_empty(),
        "no leg of the abandoned vent may fire: {after:?}"
    );
}

/// A fifth concurrent multi-step movement is refused, and **nothing is
/// transmitted for it**.
///
/// The bound is measured in boot stack rather than chosen — see
/// [`MAX_ACTIVITIES`] — and what makes it safe to have is that it fails loudly:
/// a shade that closed fully and then never vented is a shade somebody has to go
/// and open.
#[test]
fn a_fifth_concurrent_sequence_is_refused_with_nothing_on_the_air() {
    let mut controller = Controller::new();
    let mut ids = std::vec::Vec::new();
    for slot in 0..=MAX_ACTIVITIES {
        let mut config = measured_config();
        config.address = ADDRESS + slot as u32;
        ids.push(controller.registry.add_shade(config).unwrap());
    }

    let mut tx: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
    let mut deltas: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();
    for id in ids.iter().take(MAX_ACTIVITIES) {
        controller
            .command_shade(*id, ShadeCommand::Vent, 0, &mut tx, &mut deltas)
            .expect("within the bound");
    }
    assert_eq!(tx.len(), MAX_ACTIVITIES, "one Down each");

    tx.clear();
    assert_eq!(
        controller.command_shade(
            ids[MAX_ACTIVITIES],
            ShadeCommand::Vent,
            0,
            &mut tx,
            &mut deltas
        ),
        Err(DomainError::TooManySequences),
    );
    assert!(
        tx.is_empty(),
        "a refused vent must not leave a shade closing with no vent coming",
    );
}

/// A group may vent — it is a movement anybody can watch and undo, which is the
/// test `Pair` fails — but the whole group is checked first, so no member starts
/// closing for a vent that will not come.
#[test]
fn a_group_vent_is_refused_whole_rather_than_half_applied() {
    let mut controller = Controller::new();
    let mut ready = config();
    ready.vent_band_ms = VENT_BAND_MS;
    let first = controller.registry.add_shade(ready).unwrap();
    let mut other = ShadeConfig::new("Uncalibrated", ADDRESS + 1).unwrap();
    other.vent_band_ms = 0;
    let second = controller.registry.add_shade(other).unwrap();

    let group = controller.registry.add_group("Bedroom").unwrap();
    controller.registry.group_add_shade(group, first).unwrap();
    controller.registry.group_add_shade(group, second).unwrap();

    let mut planned: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
    let mut deltas: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();
    assert_eq!(
        controller.command_group(group, ShadeCommand::Vent, 0, &mut planned, &mut deltas),
        Err(DomainError::VentBandNotMeasured),
    );
    assert!(planned.is_empty(), "no member may have started closing");

    let _ = ShadeId(0);
}
