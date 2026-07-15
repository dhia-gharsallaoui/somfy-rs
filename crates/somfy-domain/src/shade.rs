//! The [`Shade`] aggregate: turns high-level [`ShadeCommand`]s into motion-model
//! updates plus the [`PlannedTx`] radio work the firmware must transmit.
//!
//! Port of the command side of `SomfyShade::sendCommand` (Somfy.cpp:2889-2960)
//! and the position-model side of `SomfyShade::processInternalCommand`
//! (Somfy.cpp:2210+ switch, notably the StepUp/StepDown target math at
//! :2481/:2522) plus the mid-range stop in `checkMovement` (Somfy.cpp:1166-1170
//! down / :1221-1227 up). Sun/wind/dry-contact/tilt-motor branches are out of
//! scope for v1.0.
//!
//! Two policy contracts from Plan 1 live here (README "Contracts for later
//! plans"):
//! 1. `Command::Stop` is NEVER planned by the domain. Stopping a 56-bit RTS
//!    motor is the `My` button (there is no physical Stop; the C++ downgrades
//!    non-basic commands to `My` on 56-bit motors, e.g. Somfy.cpp:2944). See
//!    the `stop_is_never_emitted_only_my` test.
//! 2. Rolling codes stay OUT of the domain. [`PlannedTx`] carries only address
//!    + command; the radio/persistence layer owns `somfy_rts::RollingCode`.

use crate::motion::{Motion, MotionSnapshot};
use crate::{Direction, Pos, ShadeConfig};
use heapless::Vec;
use somfy_rts::Command;

/// Position raw span (0..=10_000). Mirrors [`Pos::FULL`] as a `u32` for the
/// integer step-size arithmetic below.
const FULL_RAW: u32 = 10_000;

/// C++ default per-motor step size in milliseconds of travel per Step press
/// (`SomfyShade::stepSize = 100`, Somfy.cpp:701 / Somfy.h:317). The C++ step
/// target is `currentPos +/- 100 / (travelTime / (stepSize * frameStepSize))`
/// (Somfy.cpp:2481 up / :2522 down), i.e. the motor runs for
/// `stepSize * frameStepSize` ms per press. With the shipped defaults
/// (stepSize=100, frameStepSize=1 clamped at :2452/:2493) a press moves
/// `100 / (travel_ms / 100)` percent = a **1%** nudge on the default 10 s
/// travel. A configurable per-motor step size is deferred to a later plan; the
/// domain ports the C++ default here.
const STEP_TRAVEL_MS: u32 = 100;

/// One radio transmission the firmware's radio task must perform.
///
/// Rolling-code state is owned by the radio/persistence layer
/// (`somfy_rts::RollingCode`) — never by the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedTx {
    pub address: u32,
    pub command: Command,
}

/// A high-level shade command from the API / UI / automation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeCommand {
    Up,
    Down,
    My,
    StepUp,
    StepDown,
    GoTo(Pos),
    SetMy(Option<Pos>),
}

/// A single shade: its config, a dead-reckoned lift/tilt [`Motion`] pair, and
/// the stored favorite ("My") position.
pub struct Shade {
    pub config: ShadeConfig,
    lift: Motion,
    tilt: Motion,
    my_pos: Option<Pos>,
}

impl Shade {
    /// Position starts fully open ([`Pos::ZERO`]); no favorite is set.
    pub fn new(config: ShadeConfig) -> Shade {
        Shade {
            config,
            lift: Motion::new(Pos::ZERO),
            tilt: Motion::new(Pos::ZERO),
            my_pos: None,
        }
    }

    pub fn pos(&self) -> Pos {
        self.lift.pos()
    }

    pub fn tilt_pos(&self) -> Pos {
        self.tilt.pos()
    }

    pub fn my_pos(&self) -> Option<Pos> {
        self.my_pos
    }

    pub fn direction(&self) -> Direction {
        self.lift.direction()
    }

    /// Apply a command: update the motion model AND queue any radio frame(s).
    ///
    /// The live position is advanced to `now_ms` first ([`Shade::sync`]) so the
    /// re-target math anchors on the current estimate — the C++ `currentPos` is
    /// continuously updated by `checkMovement` before any command is processed.
    pub fn handle(&mut self, cmd: ShadeCommand, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        self.sync(now_ms);
        match cmd {
            // Up/Down always seek a hard limit; the motor self-stops there so
            // no My is scheduled (Somfy.cpp:2893-2927).
            ShadeCommand::Up => {
                self.lift.set_target(Pos::ZERO, now_ms);
                self.push(out, Command::Up);
            }
            ShadeCommand::Down => {
                self.lift.set_target(Pos::FULL, now_ms);
                self.push(out, Command::Down);
            }
            // My while moving => stop (freeze estimate) + TX My; My while idle
            // with a favorite => go to it; My while idle without one => no-op
            // (Somfy.cpp:2929-2942 + moveToMyPosition at :2863).
            ShadeCommand::My => {
                if self.lift.direction() != Direction::Idle {
                    self.lift
                        .halt(now_ms, self.config.up_time_ms, self.config.down_time_ms);
                    self.push(out, Command::My);
                } else if let Some(fav) = self.my_pos {
                    self.seek(fav, now_ms, out);
                }
            }
            ShadeCommand::GoTo(p) => self.seek(p, now_ms, out),
            ShadeCommand::StepUp => self.step(Direction::Up, now_ms, out),
            ShadeCommand::StepDown => self.step(Direction::Down, now_ms, out),
            // Favorite set/clear is a pure state change; the C++ prog-button
            // My-set flow is a pairing-assistant concern (Plan 5+).
            ShadeCommand::SetMy(p) => self.my_pos = p,
        }
    }

    /// Advance motion. On arriving at a **mid-range** target, plan the `My`
    /// stop frame (Somfy.cpp:1166-1170 down / :1221-1227 up: the motor only
    /// self-stops at the hard limits, so a mid-range target needs an explicit
    /// `My`). Hard limits (0 / FULL) need no stop.
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<PlannedTx, 4>) -> MotionSnapshot {
        let snap = self
            .lift
            .tick(now_ms, self.config.up_time_ms, self.config.down_time_ms);
        if snap.arrived && snap.pos != Pos::ZERO && snap.pos != Pos::FULL {
            self.push(out, Command::My);
        }
        snap
    }

    /// Advance the live estimate to `now_ms` without emitting any TX. Mid-range
    /// stop frames are the responsibility of [`Shade::tick`]; a command that
    /// immediately re-targets does not re-emit them.
    fn sync(&mut self, now_ms: u64) {
        let _ = self
            .lift
            .tick(now_ms, self.config.up_time_ms, self.config.down_time_ms);
    }

    /// Seek an arbitrary target from the (already-synced) live position. Emits
    /// `Up`/`Down` toward it; the mid-range stop is scheduled by [`Shade::tick`]
    /// on arrival. Seeking the current position is a no-op with no TX
    /// (Somfy.cpp GoTo path skips motors already at target).
    fn seek(&mut self, target: Pos, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        let current = self.lift.pos();
        if current == target {
            return;
        }
        let cmd = if target > current {
            Command::Down
        } else {
            Command::Up
        };
        self.lift.set_target(target, now_ms);
        self.push(out, cmd);
    }

    /// Nudge the target one step and emit the extended Step command. Port of
    /// the non-tilt Step branch (Somfy.cpp:2481 up / :2522 down): the target
    /// moves by `FULL_RAW * STEP_TRAVEL_MS / travel_ms` raw, clamped to the
    /// limits, and the extended command is transmitted regardless of whether
    /// the position changed (the C++ always calls `emitCommand`; only the
    /// target math is conditional). Zero travel time is a no-op, matching the
    /// C++ `return` guard (:2479/:2521).
    fn step(&mut self, dir: Direction, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        let travel_ms = match dir {
            Direction::Up => self.config.up_time_ms,
            _ => self.config.down_time_ms,
        };
        if travel_ms == 0 {
            return;
        }
        let step_raw = (FULL_RAW * STEP_TRAVEL_MS / travel_ms).min(FULL_RAW) as u16;
        let current = self.lift.pos().raw();
        let (target, command) = match dir {
            Direction::Up => (
                Pos::from_raw(current.saturating_sub(step_raw)),
                Command::StepUp,
            ),
            _ => (
                Pos::from_raw(current.saturating_add(step_raw)),
                Command::StepDown,
            ),
        };
        self.lift.set_target(target, now_ms);
        self.push(out, command);
    }

    fn push(&self, out: &mut Vec<PlannedTx, 4>, command: Command) {
        let _ = out.push(PlannedTx {
            address: self.config.address,
            command,
        });
    }
}
