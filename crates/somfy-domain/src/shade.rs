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

/// A single shade: its config, a dead-reckoned lift [`Motion`], and the stored
/// favorite ("My") position.
///
/// `tilt` is a NON-FUNCTIONAL placeholder for the tilt plan: no command drives
/// it yet, so [`Shade::tilt_pos`] always reports [`Pos::ZERO`]. It reserves the
/// state slot for tilt-capable shades (`tilt_first` sequencing, EuroMode)
/// without committing to semantics this task does not port.
pub struct Shade {
    pub config: ShadeConfig,
    lift: Motion,
    tilt: Motion,
    my_pos: Option<Pos>,
    /// Port of the C++ `settingPos` flag: true only while seeking an
    /// explicitly-set position target (`moveToTarget`, Somfy.cpp:3068). The
    /// mid-range arrival stop (Somfy.cpp:1166/1218) fires only when this is
    /// set — Step targets and native motor moves never schedule a stop.
    stop_on_arrival: bool,
    /// Remotes (besides this shade's own address) whose RX frames drive the
    /// estimate via [`Shade::apply_overheard`]. Bounded at
    /// `SOMFY_MAX_LINKED_REMOTES` = 7 (Somfy.h:8, `linkedRemotes[]`).
    linked: Vec<u32, 7>,
}

impl Shade {
    /// Position starts fully open ([`Pos::ZERO`]); no favorite is set.
    pub fn new(config: ShadeConfig) -> Shade {
        Shade {
            config,
            lift: Motion::new(Pos::ZERO),
            tilt: Motion::new(Pos::ZERO),
            my_pos: None,
            stop_on_arrival: false,
            linked: Vec::new(),
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
        self.sync(now_ms, out);
        match cmd {
            // Up/Down always seek a hard limit; the motor self-stops there so
            // no My is scheduled (Somfy.cpp:2893-2927, settingPos stays false).
            ShadeCommand::Up => {
                self.stop_on_arrival = false;
                self.lift.set_target(Pos::ZERO, now_ms);
                self.push(out, Command::Up);
            }
            ShadeCommand::Down => {
                self.stop_on_arrival = false;
                self.lift.set_target(Pos::FULL, now_ms);
                self.push(out, Command::Down);
            }
            // My while moving => stop (freeze estimate) + TX My; My while idle
            // with a favorite => go to it; My while idle without one => no-op
            // (Somfy.cpp:2929-2942 + moveToMyPosition at :2863). The favorite
            // recall uses GoTo semantics (the C++ simMy() branch,
            // moveToMyPosition -> moveToTarget at :2884).
            ShadeCommand::My => {
                if self.lift.direction() != Direction::Idle {
                    self.stop_on_arrival = false;
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

    /// Register a remote whose overheard RX frames should drive this shade's
    /// estimate. Rejects the sentinel addresses (0 / 0xFFFFFF, same guard as
    /// [`ShadeConfig::new`], Somfy.cpp:169-170), duplicates (including this
    /// shade's own address), and overflow past `SOMFY_MAX_LINKED_REMOTES` = 7
    /// (Somfy.h:8).
    pub fn link_remote(&mut self, addr: u32) -> Result<(), crate::DomainError> {
        use crate::DomainError;
        if addr == 0 || addr >= 0xFF_FFFF {
            return Err(DomainError::InvalidAddress);
        }
        if self.is_linked(addr) {
            return Err(DomainError::DuplicateAddress);
        }
        self.linked
            .push(addr)
            .map_err(|_| DomainError::RegistryFull)
    }

    /// True if `addr` is this shade's own remote address or a linked remote.
    /// Mirrors the C++ frame-ownership test (Somfy.cpp:2191-2199: own
    /// `getRemoteAddress()` first, then the `linkedRemotes[]` scan).
    pub fn is_linked(&self, addr: u32) -> bool {
        addr == self.config.address || self.linked.contains(&addr)
    }

    /// Apply a (deduped) frame overheard on RX from this shade's own or a
    /// linked remote — the wall remote already commanded the motor, so this
    /// only tracks the estimate and NEVER plans a [`PlannedTx`] (retransmitting
    /// would double-drive the motor). Port of `SomfyShade::processFrame` from a
    /// non-internal source (Somfy.cpp:2186): `Up`/`Down` retarget the hard
    /// limits (`p_target(0/100)`, :2360/:2388), `My` while moving freezes the
    /// estimate (`p_target(currentPos)`, :2437), and `My` while idle recalls
    /// the favorite (`p_target(myPos)`, :2429).
    ///
    /// The live estimate is advanced to `now_ms` first (like [`Shade::handle`]'s
    /// `sync`) so retargets anchor on the current position. A remote frame also
    /// aborts any of our own in-flight positioning — the C++ clears `settingPos`
    /// unconditionally here (Somfy.cpp:2209) — so we drop the pending mid-range
    /// stop by clearing `stop_on_arrival`; it is never transmitted.
    pub fn apply_overheard(&mut self, cmd: Command, now_ms: u64) {
        // Advance the estimate without emitting: `apply_overheard` has no TX
        // channel, and the C++ abandons our positioning on a remote frame.
        let _ = self
            .lift
            .tick(now_ms, self.config.up_time_ms, self.config.down_time_ms);
        self.stop_on_arrival = false;
        match cmd {
            Command::Up => self.lift.set_target(Pos::ZERO, now_ms),
            Command::Down => self.lift.set_target(Pos::FULL, now_ms),
            Command::My => {
                if self.lift.direction() != Direction::Idle {
                    self.lift
                        .halt(now_ms, self.config.up_time_ms, self.config.down_time_ms);
                } else if let Some(fav) = self.my_pos {
                    self.lift.set_target(fav, now_ms);
                }
            }
            // Other commands (combo buttons, Step, Sensor, ...) do not move the
            // lift estimate in v1.0 — the C++ routes them to telemetry-only
            // branches (Somfy.cpp:2289-2297) with no `p_target` change.
            _ => {}
        }
    }

    /// Advance motion. On arriving at a **mid-range** target of an explicit
    /// position seek, plan the `My` stop frame (Somfy.cpp:1166-1170 down /
    /// :1218-1227 up, guarded by `settingPos`: the motor only self-stops at
    /// the hard limits, so a seeked mid-range target needs an explicit `My`).
    /// Hard limits (0 / FULL) and Step-originated targets need no stop.
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<PlannedTx, 4>) -> MotionSnapshot {
        let snap = self
            .lift
            .tick(now_ms, self.config.up_time_ms, self.config.down_time_ms);
        if snap.arrived {
            if self.stop_on_arrival && snap.pos != Pos::ZERO && snap.pos != Pos::FULL {
                self.push(out, Command::My);
            }
            self.stop_on_arrival = false;
        }
        snap
    }

    /// Advance the live estimate to `now_ms` before applying a command, so the
    /// re-target math anchors on the current position — the C++ `checkMovement`
    /// runs continuously before any command is processed. A pending mid-range
    /// arrival crossed during the sync still emits its stop frame (in the C++
    /// that `My` would already have been sent by the movement loop).
    fn sync(&mut self, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        let _ = self.tick(now_ms, out);
    }

    /// Seek an arbitrary target from the (already-synced) live position. Emits
    /// `Up`/`Down` toward it; the mid-range stop is scheduled by [`Shade::tick`]
    /// on arrival (`settingPos = true`, Somfy.cpp:3068). Seeking the current
    /// position is a no-op with no TX (Somfy.cpp GoTo path skips motors
    /// already at target).
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
        self.stop_on_arrival = true;
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
        // Step targets are not `settingPos` targets (Somfy.cpp:2443-2525 never
        // set the flag): the motor self-stops after its increment, so tick
        // must not schedule a My at arrival.
        self.stop_on_arrival = false;
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

    /// Queue one frame. Capacity 4 is generous: a single `handle`/`tick` call
    /// plans at most 2 frames (a sync-crossed arrival stop plus the command's
    /// own frame). Overflow would mean the caller is not draining `out`
    /// between calls; the frame is dropped rather than panicking on-device,
    /// but debug builds assert.
    fn push(&self, out: &mut Vec<PlannedTx, 4>, command: Command) {
        let pushed = out.push(PlannedTx {
            address: self.config.address,
            command,
        });
        debug_assert!(pushed.is_ok(), "PlannedTx buffer overflow: out not drained");
    }
}
