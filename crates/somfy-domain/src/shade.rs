//! The [`Shade`] aggregate: turns high-level [`ShadeCommand`]s into motion-model
//! updates plus the [`PlannedTx`] radio work the firmware must transmit.
//!
//! This models the position side of a Somfy RTS shade: how a high-level
//! command turns into a lift-motion re-target, a queued command frame, and —
//! for a seek that lands mid-range — a follow-up stop frame once the motion
//! model reports arrival. Sun/wind/dry-contact sensing and a driven tilt axis
//! are out of scope for v1.0.
//!
//! Two policy contracts from Plan 1 live here (README "Contracts for later
//! plans"):
//! 1. `Command::Stop` is NEVER planned by the domain. A 56-bit RTS motor has
//!    no physical Stop on the protocol: the `My` command is what halts it
//!    mid-travel, and any non-basic command sent to a 56-bit motor gets
//!    downgraded to `My` on the wire. See the `stop_is_never_emitted_only_my`
//!    test.
//! 2. Rolling codes stay OUT of the domain. [`PlannedTx`] carries only address
//!    + command; the radio/persistence layer owns `somfy_rts::RollingCode`.

use crate::motion::{Motion, MotionSnapshot};
use crate::{Direction, Pos, ShadeConfig};
use heapless::Vec;
use somfy_rts::Command;

/// Position raw span (0..=10_000). Mirrors [`Pos::FULL`] as a `u32` for the
/// integer step-size arithmetic below.
const FULL_RAW: u32 = 10_000;

/// Default per-motor step size in milliseconds of travel per Step press. The
/// step target moves by `STEP_TRAVEL_MS / travel_ms` of full travel per
/// press, i.e. the motor runs for `STEP_TRAVEL_MS` ms per tap of the Step
/// button. At the default 10 s travel time that works out to a **1%** nudge
/// per press. A configurable per-motor step size is deferred to a later
/// plan; this is the shipped default, unchanged.
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
    /// True only while seeking an explicitly-set position target (a `GoTo`
    /// seek). The mid-range arrival stop fires only when this flag is set —
    /// Step targets and native Up/Down moves never schedule a stop.
    stop_on_arrival: bool,
    /// Remotes (besides this shade's own address) whose RX frames drive the
    /// estimate via [`Shade::apply_overheard`]. Bounded at 7 linked remotes,
    /// matching the fixed-size limit used by real deployed firmware.
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

    /// Current lift target (dead-reckoned seek destination).
    pub fn target(&self) -> Pos {
        self.lift.target()
    }

    pub fn my_pos(&self) -> Option<Pos> {
        self.my_pos
    }

    pub fn direction(&self) -> Direction {
        self.lift.direction()
    }

    /// Apply a command: update the motion model AND queue any radio frame(s).
    ///
    /// The live position is advanced to `now_ms` first (`sync`) so the
    /// re-target math anchors on the current estimate — a real motor's
    /// position keeps advancing continuously while it travels, so any
    /// command must be evaluated against where the shade actually is right
    /// now, not where it was when the previous command was issued.
    pub fn handle(&mut self, cmd: ShadeCommand, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        self.sync(now_ms, out);
        match cmd {
            // Up/Down always seek a hard limit; the motor self-stops there so
            // no My is scheduled.
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
            // with a favorite => simulate a move to it (GoTo semantics).
            //
            // My while idle WITHOUT a favorite (`my_pos == None`) is a NO-OP.
            // DEVIATION (see crate docs): a real RTS motor, sent a raw My
            // frame with no software-tracked favorite, can still recall a
            // favorite stored in its own hardware and move to it. We always
            // simulate positions in software instead, so we cannot reproduce
            // that hardware recall without either a raw-My passthrough or a
            // config bit to opt back into it — deferred to Plan 4.
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
            // Favorite set/clear is a pure state change; the physical
            // prog-button pairing flow that sets a favorite on real hardware
            // is a pairing-assistant concern (Plan 5+).
            ShadeCommand::SetMy(p) => self.my_pos = p,
        }
    }

    /// Register a remote whose overheard RX frames should drive this shade's
    /// estimate. Rejects the sentinel addresses (0 / 0xFFFFFF — reserved
    /// values that never identify a real remote, the same guard used by
    /// [`ShadeConfig::new`]), duplicates (including this shade's own
    /// address), and overflow past the 7-remote link limit.
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
    /// Frame ownership is checked in that order: the shade's own address
    /// first, then a scan of the linked-remote list.
    pub fn is_linked(&self, addr: u32) -> bool {
        addr == self.config.address || self.linked.contains(&addr)
    }

    /// Apply a (deduped) frame overheard on RX from this shade's own or a
    /// linked remote — the wall remote already commanded the motor, so this
    /// only tracks the estimate and NEVER plans a [`PlannedTx`] (retransmitting
    /// would double-drive the motor). `Up`/`Down` retarget the hard limits,
    /// `My` while moving freezes the estimate, `My` while idle recalls the
    /// favorite, and `StepUp`/`StepDown` nudge the estimate one step — an
    /// overheard step moves the estimate exactly like one we issued
    /// ourselves, because a wall remote's Step frame carries no marker
    /// distinguishing it from an internally-generated one.
    ///
    /// The live estimate is advanced to `now_ms` first (like [`Shade::handle`]'s
    /// `sync`) so retargets anchor on the current position. A remote frame
    /// also aborts any of our own in-flight positioning — a wall remote
    /// taking over the motor invalidates whatever seek we had in progress —
    /// so we drop the pending mid-range stop by clearing `stop_on_arrival`;
    /// it is never transmitted.
    pub fn apply_overheard(&mut self, cmd: Command, now_ms: u64) {
        // Advance the estimate without emitting: `apply_overheard` has no TX
        // channel, and a remote frame abandons our own in-flight positioning.
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
            // Step frames move the estimate: an overheard step from a wall
            // remote moves the position estimate just like our own Step
            // command would. We route it through the same `step_target`
            // math (1%/press, direction-matched) but plan no TX — the
            // buffer-less `apply_overheard` signature makes that
            // structurally impossible — and leave `stop_on_arrival` false
            // (cleared above): steps never arm the mid-range My stop,
            // overheard or not.
            Command::StepUp => {
                self.step_target(Direction::Up, now_ms);
            }
            Command::StepDown => {
                self.step_target(Direction::Down, now_ms);
            }
            // Remaining commands (combo buttons `MyUp`/`MyDown`/`MyUpDown`/
            // `UpDown`/`Prog`, sun/wind sensors, ...) do not move the lift
            // estimate in v1.0 — they are telemetry-only from the position
            // model's point of view and never retarget the motor.
            _ => {}
        }
    }

    /// Advance motion. On arriving at a **mid-range** target of an explicit
    /// position seek, plan the `My` stop frame: the motor only self-stops
    /// when it reaches a hard limit (fully open or fully closed) — it has no
    /// other way to know when to stop — so a seek to any intermediate
    /// position needs an explicit `My` frame to actually halt it there. Hard
    /// limits (0 / FULL) and Step-originated targets need no stop (guarded
    /// by `stop_on_arrival`).
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

    /// Advance the live estimate to `now_ms` before applying a command, so
    /// the re-target math anchors on the current position — a real motor's
    /// position keeps advancing continuously, so it must be caught up before
    /// any new command is evaluated. A pending mid-range arrival crossed
    /// during the sync still emits its stop frame: a real motor's movement
    /// loop would already have sent that `My` on its own by the time the new
    /// command arrives.
    fn sync(&mut self, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        let _ = self.tick(now_ms, out);
    }

    /// Seek an arbitrary target from the (already-synced) live position. Emits
    /// `Up`/`Down` toward it; the mid-range stop is scheduled by
    /// [`Shade::tick`] on arrival (`stop_on_arrival` is set here). Seeking the
    /// current position is a no-op with no TX — a motor already at its
    /// target has nothing to do.
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

    /// Internal Step: nudge the target one step, arm no arrival stop, and emit
    /// the extended Step command. The estimate math lives in [`Shade::step_target`]
    /// (shared with overheard steps); this adds the TX the internal path owes.
    ///
    /// We transmit whenever `step_target` applied the nudge (i.e. travel
    /// time is non-zero) — even if clamping meant the position ends up
    /// unchanged, the button press itself is still a real physical event
    /// that must go out on the radio. Only a genuinely zero travel time
    /// (motor not configured) skips the transmission entirely.
    ///
    /// NOTE (deliberate): a Step arriving mid-GoTo clears `stop_on_arrival`,
    /// so it abandons the pending mid-range My stop of the in-flight seek.
    /// This is the safer choice even though it means discarding a stop that
    /// was already armed: a stray Step should not leave a phantom My
    /// scheduled against a target the step has just moved past.
    fn step(&mut self, dir: Direction, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        if self.step_target(dir, now_ms) {
            self.stop_on_arrival = false;
            let command = match dir {
                Direction::Up => Command::StepUp,
                _ => Command::StepDown,
            };
            self.push(out, command);
        }
    }

    /// Move the lift target one step in `dir` from the (already-synced) live
    /// position, WITHOUT any TX. Shared by the internal [`Shade::step`] and
    /// by [`Shade::apply_overheard`] (overheard steps move the estimate
    /// exactly as an internally-issued step would).
    ///
    /// The non-tilt Step target math: the target moves by
    /// `FULL_RAW * STEP_TRAVEL_MS / travel_ms` raw, clamped to the limits.
    /// `travel_ms` is the **direction-matched** travel time — the zero-travel
    /// guard and the division both use the same (up-time-for-up,
    /// down-time-for-down) value. That matters because writing the two
    /// directions as separate branches makes it easy to guard one
    /// direction's zero-travel case while accidentally dividing by the
    /// other direction's time; keeping them matched here avoids that trap.
    /// Zero travel time is a no-op returning `false`.
    ///
    /// Returns `true` iff the step was applied (travel time non-zero).
    fn step_target(&mut self, dir: Direction, now_ms: u64) -> bool {
        let travel_ms = match dir {
            Direction::Up => self.config.up_time_ms,
            Direction::Down | Direction::Idle => self.config.down_time_ms,
        };
        if travel_ms == 0 {
            return false;
        }
        let step_raw = (FULL_RAW * STEP_TRAVEL_MS / travel_ms).min(FULL_RAW) as u16;
        let current = self.lift.pos().raw();
        let target = match dir {
            Direction::Up => Pos::from_raw(current.saturating_sub(step_raw)),
            Direction::Down | Direction::Idle => Pos::from_raw(current.saturating_add(step_raw)),
        };
        self.lift.set_target(target, now_ms);
        true
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
