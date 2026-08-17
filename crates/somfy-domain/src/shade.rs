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
//! 2. Rolling codes stay OUT of the domain. [`PlannedTx`] carries an address, a
//!    command and a repeat *policy*; the radio/persistence layer owns
//!    `somfy_rts::RollingCode` and the repeat count a policy resolves against.

use crate::motion::{Motion, MotionSnapshot};
use crate::pairing::PAIR_REPEATS;
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

/// How hard one planned frame is to be transmitted.
///
/// A bare count would be the wrong shape here, because the domain does not know
/// what an ordinary press is worth: how many repeat frames follow the first is a
/// per-controller radio setting (`somfy_tasks::TxProfile`), and a number chosen
/// here would silently override it. What the domain *does* know is the policy —
/// whether this particular frame may take whatever is configured, must not fall
/// below a floor, or must be a specific count whatever is configured.
///
/// All three exist because all three are needed, and two of them for opposite
/// reasons:
///
/// - An **arrival stop** is the single frame that ends a mid-range seek. Lose it
///   and the motor runs to its limit, because nothing else will ever tell it to
///   stop. It wants a floor — more than an ordinary command, and more still if
///   the controller is configured generously.
///   (`docs/specs/2026-08-15-position-accuracy-requirements.md` R1.)
/// - A **pairing frame**'s repeat count is part of what it means: a short burst
///   pairs a remote and a long one removes it. It wants an exact count, immune
///   to a generous configuration, or a controller tuned for a weak RF path would
///   unpair the shade it was asked to pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeats {
    /// Whatever the controller is configured to send for an ordinary command.
    Profile,
    /// At least this many, and more if the controller sends more. For a frame
    /// that must not be lost.
    AtLeast(u8),
    /// Exactly this many, whatever the controller is configured to send. For a
    /// frame whose repeat count carries meaning of its own.
    Exactly(u8),
}

impl Repeats {
    /// Resolve the policy against the controller's configured repeat count.
    pub fn resolve(self, profile: u8) -> u8 {
        match self {
            Repeats::Profile => profile,
            Repeats::AtLeast(floor) => floor.max(profile),
            Repeats::Exactly(count) => count,
        }
    }
}

/// One radio transmission the firmware's radio task must perform.
///
/// Rolling-code state is owned by the radio/persistence layer
/// (`somfy_rts::RollingCode`) — never by the domain. So is the repeat count the
/// [`Repeats`] policy resolves against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedTx {
    pub address: u32,
    pub command: Command,
    pub repeats: Repeats,
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
    /// Teach a motor this shade's remote address, so it accepts commands from
    /// this controller.
    ///
    /// The only command here that is not about position, and the only one whose
    /// effect the position model must ignore entirely — see [`Shade::handle`]'s
    /// arm for it. It is also the only one that depends on the motor already
    /// having been put into programming mode by a person standing at the shade;
    /// `docs/hardware-checklist.md` carries that sequence.
    Pair,
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
    /// estimate via [`Shade::apply_overheard`]. Bounded at
    /// [`MAX_LINKED_REMOTES`], matching the fixed-size limit used by real
    /// deployed firmware.
    linked: Vec<u32, MAX_LINKED_REMOTES>,
}

/// Remotes one shade may have linked to it.
///
/// **These are the only feedback this controller ever gets.** RTS is one-way:
/// nothing asks a motor where it is, so the position estimate is dead
/// reckoning from the moment it was last known. A wall remote moving the shade
/// is therefore not an inconvenience to be tolerated — it is the one event that
/// can put the estimate back on the truth, and it can only do that if the
/// remote's address is registered here. A shade with an empty list drifts
/// silently, and every frame that could have corrected it is decoded, matched
/// against nothing, and dropped.
pub const MAX_LINKED_REMOTES: usize = 7;

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
            // Pairing is NOT motion, and the three lines it does not have are
            // the point: no `set_target`, no `halt`, no `stop_on_arrival`.
            // A `Prog` frame tells a motor in programming mode to remember this
            // address; the motor jogs to acknowledge and goes nowhere. Treating
            // that jog as travel would leave the estimate reporting a position
            // the shade never reached, and — worse — would arm an arrival stop
            // that fires a `My` at a shade in programming mode.
            //
            // `stop_on_arrival` IS cleared, and that line is the load-bearing
            // one. A mid-range seek arms a `My` that [`Shade::tick`] transmits
            // when the estimate says the target is reached — on a clock this
            // command has no say over, so without the clear it fires seconds
            // later, inside the programming window a person has just opened at
            // the shade. **In programming mode `My` is not a stop**: it is how
            // a favourite position is stored, so that frame would silently
            // rewrite a setting inside the motor.
            //
            // Dropping the stop leaves the shade running to its physical limit,
            // which any later command undoes. The trade is one-sided, and it is
            // the same one [`Shade::step`] and [`Shade::apply_overheard`]
            // already make when something else takes over the motor.
            //
            // The repeat count is pinned rather than configured: see
            // [`PAIR_REPEATS`](crate::PAIR_REPEATS) for what a longer burst
            // does.
            ShadeCommand::Pair => {
                self.stop_on_arrival = false;
                self.push_with(out, Command::Prog, Repeats::Exactly(PAIR_REPEATS));
            }
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

    /// Forget a linked remote. [`NotFound`](crate::DomainError::NotFound) if it
    /// was not linked.
    ///
    /// The shade's **own** address is not a link and cannot be removed this
    /// way: it is what the controller transmits as, and dropping it would leave
    /// a shade nothing could command. `is_linked` answers `true` for it, which
    /// is why the check here is against the list rather than against that.
    pub fn unlink_remote(&mut self, addr: u32) -> Result<(), crate::DomainError> {
        let Some(at) = self.linked.iter().position(|held| *held == addr) else {
            return Err(crate::DomainError::NotFound);
        };
        self.linked.swap_remove(at);
        Ok(())
    }

    /// The remotes linked to this shade, for whoever has to persist them.
    ///
    /// Excludes the shade's own address, which is not a link — it is in
    /// [`ShadeConfig::address`] and is stored there.
    pub fn linked(&self) -> &[u32] {
        &self.linked
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
            //
            // **This arm is not inert, and for `Prog` that is the point.** The
            // `stop_on_arrival = false` above runs before the match, so an
            // overheard `Prog` drops any pending arrival stop — and it must.
            // Step one of the pairing procedure is a PROG press on a linked
            // wall remote, which arrives here; leaving the stop armed would
            // transmit a `My` into the programming window that press just
            // opened, and in programming mode `My` stores a favourite rather
            // than stopping anything. What it costs is the estimate: the seek
            // is abandoned without the motor being told, so the position is
            // wrong until the next move reaches a limit. That is the cheaper
            // half of the trade, and the same one the arms above make.
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

    /// Queue one ordinary frame, at whatever redundancy the controller is
    /// configured for.
    ///
    /// Everything the shade plans except pairing goes through here. A frame that
    /// needs a redundancy of its own calls [`Shade::push_with`] instead and says
    /// why at the call site — including, when R1 lands, the arrival stop in
    /// [`Shade::tick`], which is the one frame whose loss cannot be recovered
    /// from.
    fn push(&self, out: &mut Vec<PlannedTx, 4>, command: Command) {
        self.push_with(out, command, Repeats::Profile)
    }

    /// Queue one frame. Capacity 4 is generous: a single `handle`/`tick` call
    /// plans at most 2 frames (a sync-crossed arrival stop plus the command's
    /// own frame). Overflow would mean the caller is not draining `out`
    /// between calls; the frame is dropped rather than panicking on-device,
    /// but debug builds assert.
    fn push_with(&self, out: &mut Vec<PlannedTx, 4>, command: Command, repeats: Repeats) {
        let pushed = out.push(PlannedTx {
            address: self.config.address,
            command,
            repeats,
        });
        debug_assert!(pushed.is_ok(), "PlannedTx buffer overflow: out not drained");
    }
}
