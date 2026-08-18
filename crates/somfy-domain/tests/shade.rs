use heapless::Vec;
use somfy_domain::{Direction, FrameWidth, PlannedTx, Pos, Shade, ShadeCommand, ShadeConfig};
use somfy_rts::{encode56, Command, Frame};

fn shade() -> Shade {
    Shade::new(ShadeConfig::new("Test", 0x123456).unwrap())
}

/// A shade whose motor was paired as an 80-bit device.
///
/// The extended commands live only in the wide frame, so this is the shade any
/// test about them has to be about: `ShadeConfig::new` produces the narrow
/// width, which is what every motor this project has driven uses and which has
/// no field for `StepUp` to occupy.
fn wide_shade() -> Shade {
    let mut config = ShadeConfig::new("Test", 0x123456).unwrap();
    config.frame_width = FrameWidth::Bits80;
    Shade::new(config)
}

fn tx(out: &Vec<PlannedTx, 4>) -> std::vec::Vec<Command> {
    out.iter().map(|t| t.command).collect()
}

#[test]
fn up_command_targets_zero_and_transmits_up() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    s.tick(5_000, &mut out); // mid-travel at 50%
    out.clear();
    s.handle(ShadeCommand::Up, 5_000, &mut out);
    assert_eq!(tx(&out), [Command::Up]);
    assert_eq!(s.direction(), Direction::Up);
}

#[test]
fn my_while_moving_stops_and_transmits_my() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    out.clear();
    s.handle(ShadeCommand::My, 4_000, &mut out);
    assert_eq!(tx(&out), [Command::My]);
    assert_eq!(s.pos(), Pos::from_percent(40));
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn my_while_idle_goes_to_favorite() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(30))),
        0,
        &mut out,
    );
    assert!(out.is_empty()); // SetMy transmits nothing
    s.handle(ShadeCommand::My, 0, &mut out);
    assert_eq!(tx(&out), [Command::Down]); // from 0% toward 30%
    let snap = s.tick(3_000, &mut out);
    assert_eq!(snap.pos, Pos::from_percent(30));
}

#[test]
fn my_while_idle_without_favorite_is_noop() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::My, 0, &mut out);
    assert!(out.is_empty());
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn goto_midrange_emits_stop_on_arrival() {
    // A real motor only self-stops at its hard limits (fully open/closed); a
    // mid-range target needs an explicit My at arrival.
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    assert_eq!(tx(&out), [Command::Down]);
    out.clear();
    let snap = s.tick(5_000, &mut out);
    assert!(snap.arrived);
    assert_eq!(tx(&out), [Command::My]); // the scheduled stop
}

#[test]
fn goto_full_limit_does_not_emit_stop() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::FULL), 0, &mut out);
    out.clear();
    let snap = s.tick(20_000, &mut out);
    assert!(snap.arrived);
    assert!(out.is_empty()); // hard limit: motor stops itself
}

#[test]
fn goto_current_position_is_noop() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::ZERO), 0, &mut out);
    assert!(out.is_empty());
}

#[test]
fn step_commands_nudge_target_and_emit_extended_commands() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    s.tick(5_000, &mut out);
    out.clear();
    s.handle(ShadeCommand::StepDown, 5_000, &mut out);
    assert_eq!(tx(&out), [Command::StepDown]);
    out.clear();
    let snap = s.tick(20_000, &mut out);
    // Step target formula: pos +/- 100/(travel/(stepSize*frameStep)). With the
    // shipped per-motor defaults (a 100 ms step size, a frame-step of 1, and a
    // 10 s travel time) this resolves to a 1% (100-raw) nudge -- not the
    // brief's 5% guess. See docs/provenance.md for the cross-check.
    assert_eq!(snap.pos, Pos::from_raw(5_100)); // 50% + 1% step

    // Step targets are not tracked as an in-progress position seek: deployed
    // firmware's Step handling never marks one, so the mid-range arrival
    // stop is skipped -- the motor self-stops after its increment. No stop
    // frame may follow a step arrival.
    assert!(out.is_empty());
}

#[test]
fn step_up_nudges_toward_open_and_clamps_at_zero() {
    let mut s = wide_shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    s.tick(5_000, &mut out);
    out.clear();
    s.handle(ShadeCommand::StepUp, 5_000, &mut out);
    assert_eq!(tx(&out), [Command::StepUp]);
    let snap = s.tick(20_000, &mut out);
    assert_eq!(snap.pos, Pos::from_raw(4_900)); // 50% - 1% step

    // At the hard limit deployed firmware still transmits the step frame
    // unconditionally, but the position cannot move past the limit.
    let mut s = wide_shade(); // fresh shade at ZERO
    out.clear();
    s.handle(ShadeCommand::StepUp, 0, &mut out);
    assert_eq!(tx(&out), [Command::StepUp]);
    assert_eq!(s.pos(), Pos::ZERO);
    assert_eq!(s.direction(), Direction::Idle);
}

/// A shade paired at the narrow width has no frame that means "step up": the
/// command field is four bits, `StepUp`'s identity lives in the extended
/// frame's tail, and the nibble a narrow frame would carry for it is
/// `StepDown`'s — the opposite direction.
///
/// So the estimate must not move either. That is the whole of the trap: a
/// controller that nudged its estimate up and put a down-step on the air would
/// be wrong twice over, and nothing in a one-way protocol would report it.
#[test]
fn a_narrow_shade_neither_sends_nor_believes_a_step_up() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(50)), 0, &mut out);
    s.tick(5_000, &mut out);
    out.clear();

    s.handle(ShadeCommand::StepUp, 5_000, &mut out);

    assert!(out.is_empty(), "no frame a narrow shade cannot carry");
    let snap = s.tick(20_000, &mut out);
    assert_eq!(
        snap.pos,
        Pos::from_percent(50),
        "the estimate must not move"
    );
}

/// Its mirror does go out, because `StepDown` is a base command with a nibble
/// of its own — the asymmetry is in the protocol, not in this crate.
#[test]
fn a_narrow_shade_still_steps_down() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::StepDown, 0, &mut out);
    assert_eq!(tx(&out), [Command::StepDown]);
}

/// The guard above names one command, and this is what says the naming is
/// complete: every command, at both widths, and nothing planned may be
/// something its own width cannot carry. A fourth extended command, or an
/// existing command rerouted onto one, fails here rather than on a motor.
///
/// **It asks the encoder, not the rule.** `FrameWidth::carries` is the rule
/// this crate applies, so a sweep that checked against it would agree with
/// itself no matter what the rule said — verified by breaking `carries` and
/// watching this pass. `somfy_rts::encode56` is the ground truth: it is the
/// function that will actually be asked to build the frame, and it refuses an
/// extended command whatever anything here believes.
#[test]
fn nothing_planned_is_ever_wider_than_the_shade_that_planned_it() {
    let commands = [
        ShadeCommand::Up,
        ShadeCommand::Down,
        ShadeCommand::My,
        ShadeCommand::StepUp,
        ShadeCommand::StepDown,
        ShadeCommand::GoTo(Pos::from_percent(37)),
        ShadeCommand::SetMy(Some(Pos::from_percent(20))),
        ShadeCommand::Vent,
        ShadeCommand::Pair,
    ];
    for width in [FrameWidth::Bits56, FrameWidth::Bits80] {
        for command in commands {
            let mut config = ShadeConfig::new("Test", 0x123456).unwrap();
            config.frame_width = width;
            // A measured band, so `Vent` plans its legs instead of being a
            // shade the caller would have refused before it got here.
            config.vent_band_ms = 4_000;
            let mut s = Shade::new(config);
            let mut out = Vec::new();
            // Driven past a command and a whole traverse, so the frames a tick
            // plans on its own — the mid-range arrival stop, a vent's later
            // legs — are covered as well as the command's own.
            s.handle(command, 0, &mut out);
            for now in [1_000, 5_000, 11_000, 21_000, 31_000] {
                s.tick(now, &mut out);
            }
            for planned in &out {
                assert_eq!(planned.width, width, "a frame must carry its own width");
                let wire = Frame {
                    key: 0xA0,
                    command: planned.command,
                    rolling_code: 1,
                    address: planned.address,
                };
                let encodable = match planned.width {
                    // `encode80` is infallible, so the wide width has nothing
                    // to prove here beyond carrying its own name.
                    FrameWidth::Bits80 => Ok(()),
                    FrameWidth::Bits56 => encode56(&wire).map(|_| ()),
                };
                assert!(
                    encodable.is_ok(),
                    "{command:?} at {width:?} planned {:?}, which has no frame at that width",
                    planned.command
                );
            }
        }
    }
}

#[test]
fn set_my_none_clears_favorite() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(
        ShadeCommand::SetMy(Some(Pos::from_percent(30))),
        0,
        &mut out,
    );
    assert_eq!(s.my_pos(), Some(Pos::from_percent(30)));
    s.handle(ShadeCommand::SetMy(None), 0, &mut out);
    assert_eq!(s.my_pos(), None);
    assert!(out.is_empty());
    s.handle(ShadeCommand::My, 0, &mut out); // cleared favorite: My is a no-op
    assert!(out.is_empty());
    assert_eq!(s.direction(), Direction::Idle);
}

#[test]
fn stop_is_never_emitted_only_my() {
    // Plan 1 contract: Stop downgrades to My, matching deployed firmware's
    // command dispatch for 56-bit motors; this crate must never plan a
    // Command::Stop TX.
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    s.handle(ShadeCommand::My, 3_000, &mut out);
    s.handle(ShadeCommand::GoTo(Pos::from_percent(70)), 3_000, &mut out);
    s.tick(30_000, &mut out);
    assert!(out.iter().all(|t| t.command != Command::Stop));
}

#[test]
fn planned_tx_carries_shade_address() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::Down, 0, &mut out);
    assert_eq!(out[0].address, 0x123456);
}

#[test]
fn target_accessor_reports_seek_destination() {
    let mut s = shade();
    let mut out = Vec::new();
    s.handle(ShadeCommand::GoTo(Pos::from_percent(70)), 0, &mut out);
    assert_eq!(s.target(), Pos::from_percent(70));
}
