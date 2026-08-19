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
use crate::types::CalibrationSource;
use crate::{Direction, DomainError, FrameWidth, Pos, ShadeConfig};
use heapless::Vec;
use somfy_rts::Command;

/// Position raw span (0..=10_000). Mirrors [`Pos::FULL`] as a `u32` for the
/// integer step-size arithmetic below.
const FULL_RAW: u32 = 10_000;

/// Repeat frames the arrival stop asks for, on top of the first.
///
/// # Why this frame is not an ordinary one
///
/// It is the single point of failure in the whole position system. A motor
/// self-stops only at its own end stops, so a seek to any intermediate position
/// ends with exactly one `My`, and if that frame is lost nothing else will ever
/// tell the motor to stop — it runs to the limit. The loss is not recoverable
/// after the fact either: by the time the estimate could notice, the shade is
/// already there. So the stop is transmitted harder than the command that
/// started the move.
/// (`docs/specs/2026-08-15-position-accuracy-requirements.md` R1.)
///
/// # Why five, and why not more
///
/// Five repeats is a six-frame burst against an ordinary command's three, so it
/// squares the chance of losing every frame — at a 50% per-frame loss rate, from
/// one in eight to one in sixty-four.
///
/// The ceiling is not a matter of taste. On a real motor **the length of a `My`
/// burst carries meaning**: held long enough while the shade is idle, `My` is
/// how a favourite position is *stored*, and controllers in the field read a
/// burst of 35 repeats as exactly that. A tilt-capable motor reads 15 as a tilt
/// press. Six frames sits an order of magnitude below the first and well below
/// the second, so it is redundancy rather than a different command.
///
/// It costs latency only in the tail: repeat frames follow the first, and the
/// motor acts on the first frame it hears in full, so the extra three do not
/// delay the stop. What they cost is about 0.4 s more air time per stop.
///
/// # Why a floor rather than an exact count
///
/// [`Repeats::AtLeast`] takes the controller's configured count when that is
/// larger. A controller tuned generously for a weak RF path should send the
/// *most* important frame at least as hard as an ordinary one — and it cannot be
/// made worse by this, because a profile past the ceilings above would already
/// be turning every ordinary command into a tilt press. Contrast
/// [`PAIR_REPEATS`](crate::PAIR_REPEATS), which is pinned exactly because there
/// a longer burst is a different operation.
pub const STOP_REPEATS: u8 = 5;

/// Longest travel time this crate will accept from a calibration run, in
/// milliseconds.
///
/// Not chosen here: it is the ceiling deployed controllers already enforce on a
/// hand-entered travel time, adopted so that a measured value and a typed one
/// are bounded by the same number. A run still going after three minutes is one
/// where the operator walked away, and finishing it would store a travel time
/// that makes every later estimate nonsense.
pub const MAX_TRAVEL_TIME_MS: u32 = 180_000;

/// How doubtful the estimate has to be before a go-to-position pays a whole
/// extra traverse to re-anchor at a limit.
///
/// **A policy figure, and said so rather than dressed up as a derivation.** What
/// it balances is accuracy against a shade that visibly runs somewhere nobody
/// asked it to go, and no measurement decides that. 20% of the range is chosen
/// because at that much doubt the estimate can no longer tell a quarter-open
/// shade from a half-open one — the granularity people actually ask for — while
/// below it a direct seek still lands recognisably where it was sent.
///
/// See `Shade::should_route` for the two conditions that go with it, one of
/// which is what keeps this from making an uncalibrated shade travel three times
/// as far for an answer that is no better.
pub const ROUTE_VIA_LIMIT_RAW: u16 = 2_000;

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
///
/// # The width is a value, not a policy
///
/// [`Repeats`] is a policy because the domain does not know what an ordinary
/// press is worth on this installation's RF path — that is a radio setting.
/// The width is the opposite kind of thing: a motor was paired at one width and
/// answers nothing else, so there is exactly one right answer per shade and it
/// is in that shade's own record. Nothing downstream may override it, and there
/// is no controller-wide width for it to be reconciled against — which is the
/// whole of the defect this field closed. A shade the previous controller drove
/// with wide frames used to import looking healthy and never move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedTx {
    pub address: u32,
    pub command: Command,
    pub repeats: Repeats,
    /// The width this frame must go out at — the paired width of the motor at
    /// [`PlannedTx::address`], copied from its [`ShadeConfig::frame_width`].
    pub width: FrameWidth,
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
    /// Close fully, then open just far enough to separate the slats.
    ///
    /// # It deliberately uses no position estimate
    ///
    /// That is the entire design, and it is the owner's:
    ///
    /// > I do prefer having a dedicated command that ensure that it's fully
    /// > closed going fully down than opening to reach only sun holes
    ///
    /// The vent position is where a perforated-slat shutter lets light through
    /// without lifting the curtain, and it is a few seconds of Up travel from
    /// the closed limit. Reaching it by seeking a position would inherit every
    /// weakness in this document — an uncalibrated travel time, a lost stop
    /// frame, error accumulated across partial moves. Reaching it *from the
    /// closed limit* inherits none of them, because the motor puts itself at
    /// that limit whatever this controller believes.
    ///
    /// So it is three steps and a clock, and it needs exactly one measured
    /// number, [`ShadeConfig::vent_band_ms`](crate::ShadeConfig::vent_band_ms):
    ///
    /// 1. **Down**, and wait out a whole `down_time_ms` so the motor is at its
    ///    end stop. It always runs, even on a shade the estimate already calls
    ///    closed — trusting the estimate to skip it would give back the one
    ///    thing this command was built to avoid depending on.
    /// 2. **Up**, for the slat-separation band.
    /// 3. **`My`**, at [`STOP_REPEATS`], because losing this one leaves the
    ///    shade opening all the way.
    ///
    /// The cost is accepted rather than hidden: a shade already open travels its
    /// whole range down first. Slower, and right every time.
    ///
    /// Refused with
    /// [`VentBandNotMeasured`](crate::DomainError::VentBandNotMeasured) while the
    /// band is zero — see that variant.
    Vent,
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
/// There is **no tilt state**: no command drives a tilt axis in this
/// generation, so [`Shade::tilt_pos`] is a constant. It used to be a second
/// [`Motion`] reserving the slot; see that method for why the reservation was
/// given back and what tilt will actually need.
/// A movement that needs more than one frame at more than one moment.
///
/// Everything else this crate plans is decided the instant a command arrives.
/// These two are not: each drives the motor to a physical limit, waits for it to
/// get there, and only then decides what to send next. The wait is the point —
/// it is what converts a limit the motor enforces into a position this
/// controller knows.
///
/// Held as one field rather than several flags so that the states are exclusive
/// by construction: a shade cannot be half-way through a vent and half-way
/// through a re-anchoring seek.
///
/// **Nothing here survives a restart**, and nothing should: the position
/// estimate does not either, and a sequence resumed against an estimate that
/// began again at fully-open would drive a motor on the strength of a clock
/// reading from before the reboot.
/// What a shade is part-way through, when it is part-way through anything.
///
/// # Why this is one enum and not three fields
///
/// Because they are exclusive in fact — a shade cannot be half-way through a
/// vent, half-way through a re-anchoring seek and being timed by an operator at
/// the same moment — and because on this device an enum is what makes that
/// exclusivity cost nothing. See [`Activity`]'s own note on where these live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// A vent's Down leg is running; at `at_ms` the motor is at the closed
    /// limit and the Up leg may start.
    ClosingToVent { at_ms: u64 },
    /// A vent's Up leg is running the slat-separation band; at `at_ms` the slats
    /// are apart and the stop goes out.
    Separating { at_ms: u64 },
    /// A seek is being routed via a limit because the estimate was not worth
    /// timing from; at `at_ms` the motor is at that limit and `target` can be
    /// sought from a position that is known rather than calculated.
    ///
    /// `open_limit` says which limit, as a flag rather than a [`Pos`], because
    /// only the two ends are reachable and a byte here is a byte in a table the
    /// boot stack carries several copies of.
    Anchoring {
        at_ms: u64,
        open_limit: bool,
        target: Pos,
    },
    /// An operator is timing a traverse. See [`Calibrating`].
    Calibrating(Calibrating),
}

/// Which traverse a calibration run is timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationLeg {
    /// A full traverse toward open, started at the closed limit. Measures
    /// `up_time_ms`.
    Up,
    /// A full traverse toward closed, started at the open limit. Measures
    /// `down_time_ms`.
    Down,
}

/// A calibration run in progress: which direction, and when the frame went out.
///
/// Two fields, and that is the whole of it. Until 2026-08-19 there were two
/// more — the moments an operator reported the shade first stirring and the
/// curtain separating from the slats, which fixed
/// [`start_lag_ms`](crate::ShadeConfig::start_lag_ms) and the dead bands. They
/// are entered by hand now; see [`Shade::finish_calibration`] for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calibrating {
    leg: CalibrationLeg,
    started_ms: u64,
}

/// What a finished calibration run stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationOutcome {
    /// Which traverse was timed.
    pub leg: CalibrationLeg,
    /// The traverse time now stored for that direction.
    pub travel_ms: u32,
}

pub struct Shade {
    pub config: ShadeConfig,
    lift: Motion,
    my_pos: Option<Pos>,
    /// True only while seeking an explicitly-set position target (a `GoTo`
    /// seek). The mid-range arrival stop fires only when this flag is set —
    /// Step targets and native Up/Down moves never schedule a stop.
    stop_on_arrival: bool,
    /// How far the estimate may be from the truth, in raw [`Pos`] units.
    ///
    /// Grows with every move that does not end at a physical limit and returns
    /// to zero at one. See [`Shade::confidence`].
    uncertainty_raw: u16,
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
            my_pos: None,
            stop_on_arrival: false,
            // **Fully open and certain of it, which is a claim and not a
            // measurement.** A shade this controller has never moved has never
            // been anywhere, so its position is a convention rather than an
            // observation — the same convention `Motion::new(Pos::ZERO)` makes
            // one line up. The first Open or Close corrects both against a
            // physical limit, which is the only thing that ever could.
            uncertainty_raw: 0,
            linked: Vec::new(),
        }
    }

    pub fn pos(&self) -> Pos {
        self.lift.pos()
    }

    /// How far the position estimate may be from the truth, in raw [`Pos`]
    /// units: `0` means it was last set by a physical limit, `Pos::FULL` means
    /// it says nothing at all.
    ///
    /// # Why this exists
    ///
    /// A one-way protocol cannot correct its estimate, so the estimate is only
    /// ever as good as the numbers it was computed from — and one of those
    /// numbers is routinely a factory default nobody chose. Reporting "≈60%" is
    /// more honest than a confidently wrong "60%", and it is what tells a
    /// go-to-position whether it can time from here or should route via a limit
    /// first. (`docs/specs/2026-08-15-position-accuracy-requirements.md` R4.)
    ///
    /// # How it moves
    ///
    /// It **grows** by the travelled distance times its direction's
    /// [`CalibrationSource::relative_error_raw`] whenever a move ends anywhere
    /// other than a limit, and by the whole distance still to run when something
    /// this controller does not drive takes the motor over — an overheard wall
    /// remote, or a pairing press — because after that nothing knows where it
    /// stopped.
    ///
    /// It **returns to zero** on reaching [`Pos::ZERO`] or [`Pos::FULL`], where
    /// the motor's own end stop is the answer regardless of what was calculated.
    ///
    /// So it is non-decreasing between limits and floors at every limit, which
    /// is the shape the requirement asks for. On a shade still carrying factory
    /// travel times the first partial move saturates it, and that is the correct
    /// report rather than a defect: those numbers are not evidence of anything.
    pub fn confidence(&self) -> u16 {
        self.uncertainty_raw
    }

    /// The tilt axis's position, which is always [`Pos::ZERO`].
    ///
    /// # Why this is a constant and not a stored estimate
    ///
    /// It used to be a second [`Motion`], and that `Motion` was **provably
    /// inert**: nothing in this crate ever set a tilt target, so its position
    /// never left `Pos::ZERO` and `reconfigure`'s re-anchor of it was a no-op on
    /// a value that could not change. It was a reserved slot rather than a
    /// working axis.
    ///
    /// It was removed because the slot is not free. Sixteen bytes on a `Shade`
    /// is five hundred and twelve across the registry, and the whole state
    /// machine is materialised on the main stack about five times on its way
    /// into an Embassy task — so the reservation cost roughly **2.8 KB of the
    /// deepest chain this firmware runs**, on a device where that chain had
    /// about 1.5 KB of headroom left. See `crates/firmware/src/heap.rs`.
    ///
    /// **Nothing observable changed**, which is what made the removal safe: this
    /// returns the same value the stored axis always did.
    ///
    /// **What tilt will need when it lands** is not this slot anyway. A driven
    /// tilt axis needs command semantics per [`TiltMode`](crate::TiltMode) —
    /// long-press redirection, a hold window to tell a tilt press from a lift
    /// press — and the storage is the small half of that. Whoever builds it
    /// should size it against the same stack budget rather than inherit a
    /// reservation made before the budget was measured.
    pub fn tilt_pos(&self) -> Pos {
        Pos::ZERO
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
    ///
    /// # The return value replaces whatever was in flight
    ///
    /// `Some` starts a multi-step movement — a vent, or a seek routed via a
    /// limit — and `None` says there is none, which for every command means
    /// *cancel any there was*. Every command abandons what preceded it, so the
    /// two cases are exactly the two answers to "what is this shade doing now".
    ///
    /// It is returned rather than stored because there are thirty-two shades and
    /// at most a handful of movements: see [`Activity`] for where they live and
    /// what per-shade storage costs on this device. Callers that have no use for
    /// it — every test that only wants the frames — may drop it.
    pub fn handle(
        &mut self,
        cmd: ShadeCommand,
        now_ms: u64,
        out: &mut Vec<PlannedTx, 4>,
    ) -> Option<Activity> {
        self.sync(now_ms, out);
        match cmd {
            // Up/Down always seek a hard limit; the motor self-stops there so
            // no My is scheduled.
            ShadeCommand::Up => {
                self.abandon(now_ms);
                self.lift.set_target(Pos::ZERO, now_ms);
                self.push(out, Command::Up);
                None
            }
            ShadeCommand::Down => {
                self.abandon(now_ms);
                self.lift.set_target(Pos::FULL, now_ms);
                self.push(out, Command::Down);
                None
            }
            // Step 1 of three. The other two are in [`Shade::advance`], on a clock,
            // because the motor has to actually reach its end stop before the
            // second is worth sending — see [`ShadeCommand::Vent`].
            //
            // The wait is a whole `down_time_ms` from *here*, not from wherever
            // the estimate says the shade is. A shade the estimate calls closed
            // still waits it out, and that is the design rather than a
            // pessimisation: the estimate is the thing this command exists not
            // to depend on.
            ShadeCommand::Vent => {
                self.abandon(now_ms);
                self.lift.set_target(Pos::FULL, now_ms);
                self.push(out, Command::Down);
                Some(Activity::ClosingToVent {
                    at_ms: now_ms + self.config.down_time_ms as u64,
                })
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
                // A stop halts whatever is in flight — including a vent's Down
                // leg or a re-anchoring run, which are movements a person
                // watching the shade would expect a stop button to end. Returning
                // `None` is what ends them: the caller drops the activity.
                if self.lift.direction() != Direction::Idle {
                    self.abandon(now_ms);
                    self.lift.halt(now_ms, self.config.travel());
                    self.push(out, Command::My);
                    None
                } else if let Some(fav) = self.my_pos {
                    self.seek_reliably(fav, now_ms, out)
                } else {
                    None
                }
            }
            ShadeCommand::GoTo(p) => self.seek_reliably(p, now_ms, out),
            ShadeCommand::StepUp => {
                self.step(Direction::Up, now_ms, out);
                None
            }
            ShadeCommand::StepDown => {
                self.step(Direction::Down, now_ms, out);
                None
            }
            // Favorite set/clear is a pure state change; the physical
            // prog-button pairing flow that sets a favorite on real hardware
            // is a pairing-assistant concern (Plan 5+).
            ShadeCommand::SetMy(p) => {
                self.my_pos = p;
                None
            }
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
                self.abandon(now_ms);
                self.push_with(out, Command::Prog, Repeats::Exactly(PAIR_REPEATS));
                None
            }
        }
    }

    /// Register a remote whose overheard RX frames should drive this shade's
    /// estimate. Rejects the sentinel addresses (0 / 0xFFFFFF — reserved
    /// values that never identify a real remote, the same guard used by
    /// [`ShadeConfig::new`]), duplicates (including this shade's own
    /// address), and overflow past the 7-remote link limit.
    /// Replace this shade's configuration, keeping any move it is making
    /// honest.
    ///
    /// # Why a method rather than an assignment to `config`
    ///
    /// Because `config` carries the travel times, and [`Shade::tick`] reads
    /// them **absolutely**: it computes where the shade is from the start
    /// anchor, the elapsed time and the travel time, rather than integrating
    /// step by step. So assigning a new travel time mid-move re-interprets the
    /// travel that has already happened, and the failure is not subtle — a
    /// shade 10 s into a 30 s close, given a corrected 10 s time, is reported
    /// as *arrived and fully shut* on the very next tick, and the controller
    /// plans a stop that halts the motor a third of the way down.
    ///
    /// That is not an adversarial case. It is the calibration workflow the
    /// position-accuracy requirements ask for — time the shade with a
    /// stopwatch, then save what you measured — performed while the shade is
    /// still moving, which is exactly when somebody has just timed it.
    ///
    /// So the move is re-anchored at where the *old* times say it has reached,
    /// and the new times apply only to what is left. Nothing can recover what
    /// the old number should have been, so this is the only reading that can be
    /// right.
    ///
    /// **The address is not taken from `config`.** A motor obeys an address;
    /// nothing in this protocol can tell it the address moved, and nothing can
    /// ask it what it learned — so a shade whose address changed is a shade
    /// that stops responding and is fixed only by walking to it. The incoming
    /// value is overwritten with the one this shade already has.
    ///
    /// **Neither is [`ShadeConfig::pairing_state`]**, for a related reason: it
    /// records what a person reported, and an edit is not a report. Carrying it
    /// through would let a rename confirm a shade nobody has watched move, and
    /// would let a corrected travel time retire the entities of one that works.
    /// [`Shade::confirm_pairing`] is the only way it changes.
    pub fn reconfigure(&mut self, mut config: ShadeConfig, now_ms: u64) {
        self.lift.reanchor(now_ms, self.config.travel());
        config.address = self.config.address;
        config.pairing_state = self.config.pairing_state;
        self.config = config;
    }

    /// Record that an operator reported this shade working, and say whether
    /// that was news.
    ///
    /// **This transmits nothing and observes nothing.** It is the one place
    /// [`PairingState`](crate::PairingState) moves, and what it moves it from
    /// is a person's account — see that type for why the controller can never
    /// supply one itself.
    ///
    /// One direction only. There is no `unconfirm`, and the omission is the
    /// same kind as the missing unpair command: the recoverable failure is a
    /// shade that has to be confirmed again, and the unrecoverable one is a
    /// working entity retired out from under an automation because something
    /// decided it looked unconfirmed. Removing the shade is the way to undo
    /// this, and it is deliberately the loud way.
    ///
    /// `false` means it was already confirmed, which the caller uses to avoid
    /// scheduling a flash write for a change that is not one.
    pub fn confirm_pairing(&mut self) -> bool {
        if self.config.pairing_state.is_confirmed() {
            return false;
        }
        self.config.pairing_state = crate::PairingState::ConfirmedByOperator;
        true
    }

    /// Start timing a full traverse, transmitting the command that starts it.
    ///
    /// # The whole of the guided calibration, and why it is this small
    ///
    /// Nothing on this device can see the shade. The only instrument available
    /// is a person watching it and a clock, so a calibration is: send the
    /// traverse, let the operator say when it stopped, and store the interval.
    ///
    /// One end of that interval is *this device's* clock, which is what makes
    /// the run worth using at all: only the stop carries the operator's reaction
    /// delay, where timing the same traverse with a wristwatch carries it at
    /// both ends.
    ///
    /// The caller is responsible for the shade being at the **opposite** limit
    /// first, and can get it there with an ordinary Close or Open; the domain
    /// cannot check it, because checking would mean trusting the estimate this
    /// run exists to replace.
    ///
    /// Starting a run replaces any run already in progress. There is no state
    /// worth protecting — a half-finished run has stored nothing — and refusing
    /// would leave an operator who mis-tapped with no way forward but a reboot.
    ///
    /// Both directions are timed separately and neither is derived from the
    /// other. On the estate this came from, up takes 30 s and down 27 s, because
    /// closing is gravity-assisted; a routine that measured one and mirrored it
    /// would be wrong by that 10%.
    pub fn begin_calibration(
        &mut self,
        leg: CalibrationLeg,
        now_ms: u64,
        out: &mut Vec<PlannedTx, 4>,
    ) -> Activity {
        self.sync(now_ms, out);
        self.abandon(now_ms);
        let (limit, command) = match leg {
            CalibrationLeg::Up => (Pos::ZERO, Command::Up),
            CalibrationLeg::Down => (Pos::FULL, Command::Down),
        };
        self.lift.set_target(limit, now_ms);
        self.push(out, command);
        Activity::Calibrating(Calibrating {
            leg,
            started_ms: now_ms,
        })
    }

    /// End the run: store what it measured, and take the limit it ended at.
    ///
    /// `now_ms` is the moment the operator reported the shade stopping, so the
    /// traverse time is the whole interval since the command went out — which is
    /// what a stopwatch measures and what
    /// [`ShadeConfig::up_time_ms`](crate::ShadeConfig::up_time_ms) has always
    /// meant. The lag and the bands are *parts of* that interval rather than
    /// additions to it, so a traverse that grows or shrinks does not move them.
    ///
    /// The run ends at a physical limit by construction, so this is also an
    /// endpoint resynchronisation: the estimate is snapped there and the
    /// accumulated doubt cleared.
    ///
    /// # One number, where there used to be three
    ///
    /// Until 2026-08-19 a run also carried two `mark` presses, which fixed
    /// [`start_lag_ms`](crate::ShadeConfig::start_lag_ms) and this leg's dead
    /// band. They were dropped, and the reason is that they measured worst the
    /// thing they were for: each was a *single* press against a moment a
    /// fraction of a second wide, so each carried a whole reaction delay against
    /// the interval it defined — where the same delay is a fraction of a percent
    /// of a half-minute traverse. Both values are entered by hand instead, which
    /// R9 of the position-accuracy spec already required as a MUST.
    ///
    /// So the arithmetic below is the traverse and nothing else. What the marks
    /// *did* leave behind is the [`checked_bands`](ShadeConfig::checked_bands)
    /// call: a hand-entered band and a freshly measured traverse are two numbers
    /// that have to agree, and a 30 s slat separation on a shade that turns out
    /// to open in 8 s is refused here rather than stored.
    ///
    /// Refused, storing nothing, if the result would not be a shade this crate
    /// would accept: a traverse of zero, or over [`MAX_TRAVEL_TIME_MS`], or one
    /// too short for the bands already stored against it.
    pub fn finish_calibration(
        &mut self,
        run: Calibrating,
        now_ms: u64,
    ) -> Result<CalibrationOutcome, DomainError> {
        let elapsed = now_ms.saturating_sub(run.started_ms);
        if elapsed == 0 || elapsed > MAX_TRAVEL_TIME_MS as u64 {
            return Err(DomainError::CalibrationImplausible);
        }
        let travel_ms = elapsed as u32;

        // Applied to a copy first, so a run whose numbers do not survive
        // validation leaves the shade exactly as it was.
        let mut next = self.config.clone();
        match run.leg {
            CalibrationLeg::Up => {
                next.up_time_ms = travel_ms;
                next.up_time_source = CalibrationSource::Measured;
            }
            CalibrationLeg::Down => {
                next.down_time_ms = travel_ms;
                next.down_time_source = CalibrationSource::Measured;
            }
        }
        next.checked_bands()?;

        self.config = next;
        let limit = match run.leg {
            CalibrationLeg::Up => Pos::ZERO,
            CalibrationLeg::Down => Pos::FULL,
        };
        self.reach_limit(limit, now_ms);
        Ok(CalibrationOutcome {
            leg: run.leg,
            travel_ms,
        })
    }

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
        let _ = self.lift.tick(now_ms, self.config.travel());
        self.abandon(now_ms);
        match cmd {
            Command::Up => self.lift.set_target(Pos::ZERO, now_ms),
            Command::Down => self.lift.set_target(Pos::FULL, now_ms),
            Command::My => {
                if self.lift.direction() != Direction::Idle {
                    self.lift.halt(now_ms, self.config.travel());
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
        let travel = self.config.travel();
        let snap = self.lift.tick(now_ms, travel);

        if self.stop_on_arrival {
            // **The stop goes out a start lag early**, because a `My` takes that
            // long to reach the motor and the motor keeps travelling meanwhile.
            // With no measured lag this is `remaining == 0`, i.e. exactly the
            // arrival test it refines.
            let due = snap.arrived
                || self
                    .lift
                    .remaining_ms(now_ms, travel)
                    .is_some_and(|left| left <= travel.start_lag_ms as u64);
            if due {
                // Guarded on the **target**, not on where the estimate has got
                // to: firing early means the two differ, and it is the target
                // that decides whether the motor will stop by itself. At a hard
                // limit it will, and a `My` there would be a favourite recall
                // rather than a stop.
                let target = self.lift.target();
                if target != Pos::ZERO && target != Pos::FULL {
                    self.push_with(out, Command::My, Repeats::AtLeast(STOP_REPEATS));
                }
                self.stop_on_arrival = false;
            }
        }

        if snap.arrived {
            // R3: a move that ended at a limit ended where the motor's own end
            // stop is, whatever was calculated on the way. Take it.
            if snap.pos == Pos::ZERO || snap.pos == Pos::FULL {
                self.reach_limit(snap.pos, now_ms);
            } else {
                self.charge_arrival();
            }
        }

        snap
    }

    /// Drive one multi-step movement forward: the two legs of a vent, and the
    /// second half of a seek routed via a limit.
    ///
    /// Each waits out a whole traverse time before acting, so that what happens
    /// next is decided from a position the **motor** established rather than one
    /// this controller calculated. That wait is the whole value; shortening it
    /// against the estimate would put the estimate back in the path.
    ///
    /// Returns what the shade is doing *next*, so the caller can store it — the
    /// same contract as [`Shade::handle`], and for the same reason.
    pub fn advance(
        &mut self,
        activity: Activity,
        now_ms: u64,
        out: &mut Vec<PlannedTx, 4>,
    ) -> Option<Activity> {
        match activity {
            Activity::ClosingToVent { at_ms } if now_ms >= at_ms => {
                // The motor has had a full close and stopped itself at the end
                // stop, so this is one of the two moments in the whole system
                // where the position is known rather than computed.
                self.reach_limit(Pos::FULL, now_ms);
                self.push(out, Command::Up);
                Some(Activity::Separating {
                    at_ms: now_ms
                        + self.config.start_lag_ms as u64
                        + self.config.vent_band_ms as u64,
                })
            }
            Activity::Separating { at_ms } if now_ms >= at_ms => {
                // Losing this frame leaves the shade opening all the way, so it
                // goes out at the same redundancy as any other arrival stop.
                self.push_with(out, Command::My, Repeats::AtLeast(STOP_REPEATS));
                // The curtain has not risen — that is what a vent *is* — so the
                // lift estimate is still the closed limit, and still known,
                // which is why nothing here touches it or the uncertainty.
                None
            }
            Activity::Anchoring {
                at_ms,
                open_limit,
                target,
            } if now_ms >= at_ms => {
                self.reach_limit(if open_limit { Pos::ZERO } else { Pos::FULL }, now_ms);
                self.seek(target, now_ms, out)
            }
            // Not due yet, or a calibration run, which is driven by the operator
            // rather than by the clock.
            other => Some(other),
        }
    }

    /// Take the one piece of ground truth a one-way protocol offers.
    ///
    /// The motor stops itself at its end stops whatever this controller
    /// believes, so a move that ended at one says where the shade *is*. Snap the
    /// estimate onto it and drop the accumulated error, which is what stops that
    /// error growing monotonically across partial moves.
    fn reach_limit(&mut self, limit: Pos, now_ms: u64) {
        self.lift.resync(limit, now_ms);
        self.uncertainty_raw = 0;
    }

    /// Charge the estimate for a move that ended somewhere no limit could
    /// confirm.
    ///
    /// The error is proportional to the distance travelled and to how much the
    /// travel time it was computed from is worth — see
    /// [`CalibrationSource::relative_error_raw`]. Charged at the end of the move
    /// rather than continuously so that it counts moves, which is what makes it
    /// monotone per move and easy to reason about.
    fn charge_arrival(&mut self) {
        let travelled = self.lift.pos().raw().abs_diff(self.lift.start_pos().raw());
        let source = if self.lift.target() > self.lift.start_pos() {
            self.config.down_time_source
        } else {
            self.config.up_time_source
        };
        let rate = source.relative_error_raw();
        self.add_doubt((travelled as u32 * rate as u32) / FULL_RAW);
    }

    /// Raise the uncertainty by `raw` units, saturating at the full span.
    fn add_doubt(&mut self, raw: u32) {
        self.uncertainty_raw = (self.uncertainty_raw as u32)
            .saturating_add(raw)
            .min(FULL_RAW) as u16;
    }

    /// Give up whatever this controller had in flight, and pay for it.
    ///
    /// Called wherever something takes the motor out of our hands: a new
    /// command, an overheard wall remote, a pairing press. The pending stop is
    /// dropped without being sent — the trade every one of those call sites
    /// already made — and the distance the abandoned move had *left to run* is
    /// charged to the uncertainty **in full**, not scaled by a calibration
    /// rate. An abandoned move is not a mis-timed one: it is one whose end
    /// nobody observed, so the motor may have covered all of that distance or
    /// none of it.
    fn abandon(&mut self, _now_ms: u64) {
        if self.stop_on_arrival {
            let remaining = self.lift.pos().raw().abs_diff(self.lift.target().raw());
            self.add_doubt(remaining as u32);
        }
        self.stop_on_arrival = false;
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
    fn seek(&mut self, target: Pos, now_ms: u64, out: &mut Vec<PlannedTx, 4>) -> Option<Activity> {
        let current = self.lift.pos();
        if current == target {
            return None;
        }
        let cmd = if target > current {
            Command::Down
        } else {
            Command::Up
        };
        self.stop_on_arrival = true;
        self.lift.set_target(target, now_ms);
        self.push(out, cmd);
        None
    }

    /// Seek `target`, going via a physical limit first when the estimate is no
    /// longer worth timing from.
    ///
    /// # Why this is allowed to take longer
    ///
    /// Timing a seek from a position that may be anywhere produces a shade that
    /// stops anywhere. Timing it from a limit produces a shade that stops where
    /// it was asked to, at the cost of one extra traverse. Above
    /// [`ROUTE_VIA_LIMIT_RAW`] of doubt the second is the better trade, and
    /// below it the first is. (`R3`: "when accumulated uncertainty is high, a
    /// go-to-position MAY route via the nearest limit first and time from
    /// there".)
    ///
    /// # Which limit, and why the estimate does not get a vote
    ///
    /// "Nearest" cannot be answered by a position that is under suspicion — that
    /// is the situation this branch is *for*. So the choice is made on the
    /// **worst case**, which needs no estimate at all: reaching a limit costs at
    /// most one full traverse in that direction, and the run back to the target
    /// costs a known fraction of the other. Whichever sum is smaller wins.
    fn seek_reliably(
        &mut self,
        target: Pos,
        now_ms: u64,
        out: &mut Vec<PlannedTx, 4>,
    ) -> Option<Activity> {
        if !self.should_route(target) {
            return self.seek(target, now_ms, out);
        }
        let up = self.config.up_time_ms as u64;
        let down = self.config.down_time_ms as u64;
        let target_raw = target.raw() as u64;
        // Via fully open: run up (at most a whole up traverse), then down to the
        // target. Via fully closed: run down, then up to the target.
        let via_open = up + down * target_raw / FULL_RAW as u64;
        let via_closed = down + up * (FULL_RAW as u64 - target_raw) / FULL_RAW as u64;
        let (open_limit, run_ms, command) = if via_open <= via_closed {
            (true, up, Command::Up)
        } else {
            (false, down, Command::Down)
        };
        self.stop_on_arrival = false;
        self.lift
            .set_target(if open_limit { Pos::ZERO } else { Pos::FULL }, now_ms);
        self.push(out, command);
        Some(Activity::Anchoring {
            at_ms: now_ms + run_ms,
            open_limit,
            target,
        })
    }

    /// Whether a seek to `target` should be routed via a limit first.
    ///
    /// Three conditions, and the last two are what stop this from being a
    /// pessimisation:
    ///
    /// - The estimate is doubtful enough to be worth an extra traverse
    ///   ([`ROUTE_VIA_LIMIT_RAW`]).
    /// - The target is not itself a limit. A seek to a limit already ends at
    ///   one, so routing would be a detour to somewhere it was already going.
    /// - **Both travel times are worth timing from.** Re-anchoring buys a known
    ///   starting position and nothing else; if the number that converts time
    ///   into distance is a factory default nobody chose, the leg back from the
    ///   limit is as wrong as the direct seek would have been, and the shade has
    ///   travelled its whole range to learn nothing. On such a shade the
    ///   estimate stays saturated and says so, which is the report that gets it
    ///   calibrated.
    fn should_route(&self, target: Pos) -> bool {
        self.uncertainty_raw >= ROUTE_VIA_LIMIT_RAW
            && target != Pos::ZERO
            && target != Pos::FULL
            && self.config.up_time_source != CalibrationSource::FactoryDefault
            && self.config.down_time_source != CalibrationSource::FactoryDefault
    }

    /// Internal Step: nudge the target one step, arm no arrival stop, and emit
    /// the extended Step command. The estimate math lives in [`Shade::step_target`]
    /// (shared with overheard steps); this adds the TX the internal path owes.
    ///
    /// We transmit whenever `step_target` applied the nudge (i.e. travel
    /// time is non-zero) — even if clamping meant the position ends up
    /// unchanged, the button press itself is still a real physical event
    /// that must go out on the radio. Two things skip it entirely: a genuinely
    /// zero travel time (motor not configured), which moves the estimate no
    /// more than it moves the shade, and a `StepUp` on a shade paired at the
    /// narrow frame width, which has no wire representation at all — see the
    /// body.
    ///
    /// NOTE (deliberate): a Step arriving mid-GoTo clears `stop_on_arrival`,
    /// so it abandons the pending mid-range My stop of the in-flight seek.
    /// This is the safer choice even though it means discarding a stop that
    /// was already armed: a stray Step should not leave a phantom My
    /// scheduled against a target the step has just moved past.
    fn step(&mut self, dir: Direction, now_ms: u64, out: &mut Vec<PlannedTx, 4>) {
        let command = match dir {
            Direction::Up => Command::StepUp,
            _ => Command::StepDown,
        };
        // Checked before anything moves, including the estimate. `StepUp` is an
        // extended command and a narrow frame has no field for it — the nibble
        // it would occupy is `StepDown`'s — so on a shade paired at that width
        // there is no frame to send, and moving the estimate one step up for a
        // frame that never went out would leave this controller believing a
        // position the motor never reached.
        //
        // `Controller::command_shade` refuses the same case with
        // `DomainError::CommandNotAtThisWidth` so the operator is told; this is
        // the same rule stated where the frame is actually built, so a caller
        // holding a `Shade` directly cannot plan one either.
        if !self.config.frame_width.carries(command) {
            return;
        }
        // Before the target moves, not after: `abandon` charges the distance the
        // seek being given up still had to run, and reading that against the
        // step's own new target would charge one step instead of the whole
        // abandoned move.
        self.abandon(now_ms);
        if self.step_target(dir, now_ms) {
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
    /// A frame that needs a redundancy of its own calls [`Shade::push_with`]
    /// instead and says why at the call site. Two do: the pairing burst, whose
    /// length is part of what it means ([`PAIR_REPEATS`](crate::PAIR_REPEATS)),
    /// and the arrival stop, which is the one frame whose loss cannot be
    /// recovered from ([`STOP_REPEATS`]).
    fn push(&self, out: &mut Vec<PlannedTx, 4>, command: Command) {
        self.push_with(out, command, Repeats::Profile)
    }

    /// Queue one frame. Capacity 4 is generous: a single `handle`/`tick` call
    /// plans at most 2 frames (a sync-crossed arrival stop plus the command's
    /// own frame). Overflow would mean the caller is not draining `out`
    /// between calls; the frame is dropped rather than panicking on-device,
    /// but debug builds assert.
    fn push_with(&self, out: &mut Vec<PlannedTx, 4>, command: Command, repeats: Repeats) {
        debug_assert!(
            self.config.frame_width.carries(command),
            "planned a command the shade's own frame width cannot carry"
        );
        let pushed = out.push(PlannedTx {
            address: self.config.address,
            command,
            repeats,
            // The shade's own width, never a caller's: the motor at this
            // address answers one width and this record is the only thing that
            // knows which. Read here rather than at the radio so that a frame
            // and the width it must go out at cannot be separated.
            width: self.config.frame_width,
        });
        debug_assert!(pushed.is_ok(), "PlannedTx buffer overflow: out not drained");
    }
}
