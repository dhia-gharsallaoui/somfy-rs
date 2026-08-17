//! The [`Controller`] facade: the single object the firmware task talks to.
//!
//! Commands and overheard RX frames go in; the [`PlannedTx`] radio work and the
//! [`StateDelta`] state changes the UI/telemetry layer must surface come out.
//! It owns the [`Registry`] (shades/groups/rooms) and an [`RxDeduper`] and holds
//! the one piece of cross-call state a delta stream needs: the last state it
//! reported per shade slot, so unchanged ticks stay silent.
//!
//! # Contracts
//! - Overheard frames drive the estimate but NEVER retransmit (that would
//!   double-drive the motor) — see [`Shade::apply_overheard`].
//! - `Command::Stop` is never planned by the domain (the `My` button stops a
//!   56-bit motor) — see [`Shade`].

use crate::registry::{GroupId, Registry, ShadeId};
use crate::shade::{
    Activity, Calibrating, CalibrationLeg, CalibrationMark, CalibrationOutcome, PlannedTx,
    ShadeCommand,
};
use crate::{Direction, DomainError, Pos};
use heapless::Vec;
use somfy_rts::{Frame, RxDeduper};

/// RX dedupe window: repeats of one button press inside this span collapse to a
/// single logical event. A physical RTS remote fires ~7 repeats over well under
/// a second per press, so 2 s comfortably covers one press without swallowing a
/// deliberate second press.
pub const RX_DEDUPE_WINDOW_MS: u32 = 2_000;

/// Caller-facing TX buffer capacity, sized to the structural worst case so
/// overflow is impossible rather than merely documented: a full group holds
/// [`MAX_SHADES`](crate::MAX_SHADES) = 32 members and
/// [`Shade::handle`](crate::Shade::handle) plans at most 2 frames per shade (a
/// sync-crossed arrival stop plus the command's own frame), so `command_group`
/// can plan at most 32 x 2 = **64** frames in one call. `tick` is bounded lower
/// (32 shades x at most 1 arrival-stop frame = 32) and `command_shade` at 2.
pub const TX_CAPACITY: usize = crate::registry::MAX_SHADES * 2;

/// Caller-facing [`StateDelta`] buffer capacity, sized to the structural worst
/// case: every call emits at most one delta per shade (`tick` touches each of
/// the [`MAX_SHADES`](crate::MAX_SHADES) slots once; `command_group` fans out to
/// at most a full group = `MAX_SHADES` members), so a `MAX_SHADES`-deep buffer
/// can never overflow through the public API.
pub const DELTA_CAPACITY: usize = crate::registry::MAX_SHADES;

/// The observable state every shade is assumed to start at (fully open, at
/// rest). A shade sitting at this baseline produces no delta — deltas report
/// *changes* from it, so a freshly added, untouched shade is silent until it
/// actually moves. Also the reset point a slot returns to when a new shade
/// reuses it (see [`Controller::emit_if_changed`]).
const RESTING: (Pos, Pos, Direction) = (Pos::ZERO, Pos::ZERO, Direction::Idle);

/// One shade whose observable state `(pos, tilt_pos, direction)` changed this
/// call. The firmware fans these out to MQTT/websocket/telemetry consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateDelta {
    pub id: ShadeId,
    pub pos: Pos,
    pub tilt_pos: Pos,
    pub direction: Direction,
}

/// Facade the firmware talks to: commands in, [`PlannedTx`] + [`StateDelta`] out.
pub struct Controller {
    pub registry: Registry,
    dedupe: RxDeduper,
    /// Last state reported per shade slot, tagged with the shade's radio
    /// address. The address tag makes slot reuse safe: the registry hands a
    /// freed slot to the next `add_shade`, so a bare `(pos, tilt, dir)` cache
    /// keyed only by slot index could suppress a re-added shade's first delta
    /// when it coincidentally matched the previous occupant. Comparing the
    /// address first treats any slot now holding a *different* shade as having
    /// no baseline (i.e. [`RESTING`]).
    last_emitted: [Option<(u32, Pos, Pos, Direction)>; crate::registry::MAX_SHADES],
    /// The multi-step movements in flight, and the calibration run if there is
    /// one. See [`MAX_ACTIVITIES`].
    activities: Vec<(ShadeId, Activity), MAX_ACTIVITIES>,
}

/// Shades that may be part-way through a multi-step movement at once.
///
/// # Why this is here and not a field on every shade
///
/// **Because it is measured in boot stack.** The state machine — the whole
/// registry of thirty-two shades — is materialised on the main stack about five
/// times on the way into its Embassy task, so a byte added to a `Shade` costs
/// roughly a hundred and seventy bytes of the deepest chain this firmware runs.
/// A vent's deadline and a calibration run come to about sixty bytes each, which
/// per shade is more stack than the whole boot path had left.
///
/// A table of four costs sixty-eight bytes total, and the thing being bounded is
/// not really storage: a vent is a person deciding to vent a window, and a
/// calibration is a person standing at one with a stopwatch. Four at once is
/// already more than the estate this was built for has windows facing the same
/// weather.
///
/// **What it costs:** a fifth concurrent sequence is refused with
/// [`DomainError::TooManySequences`], which is what a group vent of more than
/// four shades meets. `crates/firmware/src/heap.rs` is where the arithmetic that
/// forces this lives.
pub const MAX_ACTIVITIES: usize = 4;

impl Controller {
    pub fn new() -> Controller {
        Controller {
            registry: Registry::new(),
            dedupe: RxDeduper::new(RX_DEDUPE_WINDOW_MS),
            last_emitted: [None; crate::registry::MAX_SHADES],
            activities: Vec::new(),
        }
    }

    /// Replace what `id` is doing, and say whether it could be stored.
    ///
    /// `None` clears the entry, which is what every ordinary command means: it
    /// abandons whatever preceded it. `Some` replaces or inserts, and an insert
    /// past [`MAX_ACTIVITIES`] is refused — see that constant for what is being
    /// traded and why.
    fn set_activity(&mut self, id: ShadeId, next: Option<Activity>) -> Result<(), DomainError> {
        let at = self.activities.iter().position(|(held, _)| *held == id);
        match (at, next) {
            (Some(at), Some(activity)) => self.activities[at].1 = activity,
            (Some(at), None) => {
                self.activities.swap_remove(at);
            }
            (None, Some(activity)) => self
                .activities
                .push((id, activity))
                .map_err(|_| DomainError::TooManySequences)?,
            (None, None) => {}
        }
        Ok(())
    }

    /// What `id` is doing, if anything.
    fn activity(&self, id: ShadeId) -> Option<Activity> {
        self.activities
            .iter()
            .find(|(held, _)| *held == id)
            .map(|(_, activity)| *activity)
    }

    /// The calibration run on `id`, if that is what it is doing.
    fn run(&self, id: ShadeId) -> Option<Calibrating> {
        match self.activity(id) {
            Some(Activity::Calibrating(run)) => Some(run),
            _ => None,
        }
    }

    /// Whether a calibration run is in progress on `id`.
    pub fn is_calibrating(&self, id: ShadeId) -> bool {
        self.run(id).is_some()
    }

    /// Push a [`StateDelta`] for `id` iff its observable state differs from what
    /// was last reported for the shade currently in that slot. A slot whose
    /// cached address does not match the live shade (fresh controller, or the
    /// slot reused by a different shade) is compared against [`RESTING`], so a
    /// re-added shade still emits its first real change and an untouched shade
    /// at rest stays silent.
    ///
    /// The push is attempted **before** the cache slot is updated, and the slot
    /// is updated only on a successful push. `deltas` is sized to
    /// [`DELTA_CAPACITY`], the structural worst case, so a failed push means the
    /// capacity math itself regressed — debug builds scream via the assert. In
    /// release the drop is non-fatal AND self-healing: the stale slot leaves the
    /// state looking un-reported, so the delta re-emits on the next call rather
    /// than being permanently suppressed. (Generic over the buffer depth so the
    /// overflow path is unit-testable; callers pass a `DELTA_CAPACITY` buffer.)
    ///
    /// **The registry lookup below is what keeps the cache index in bounds, and
    /// it has to stay first.** [`ShadeId`] is a public tuple struct, so any
    /// caller can build a `ShadeId(200)` out of nothing, and `last_emitted` is
    /// a bare `[_; MAX_SHADES]` indexed by the raw byte. It is in range only
    /// because [`Registry::shade`](crate::Registry::shade) has already answered
    /// `Some` for this id, and it can only do that for a slot the registry
    /// holds — whose array cannot be longer than `MAX_SHADES`. Reordering these
    /// two lines, or reaching the cache on a path that skips the lookup, is an
    /// out-of-bounds panic on a value a caller made up. Ids the caller chooses
    /// rather than the registry — see
    /// [`Registry::add_shade_with_id`](crate::Registry::add_shade_with_id) —
    /// make that easier to do by accident, which is why it is written down.
    fn emit_if_changed<const N: usize>(&mut self, id: ShadeId, deltas: &mut Vec<StateDelta, N>) {
        let Some(shade) = self.registry.shade(id) else {
            return;
        };
        let addr = shade.config.address;
        let now = (shade.pos(), shade.tilt_pos(), shade.direction());
        let slot = &mut self.last_emitted[id.0 as usize];
        let baseline = match *slot {
            Some((a, p, t, d)) if a == addr => (p, t, d),
            _ => RESTING,
        };
        if baseline != now {
            let pushed = deltas.push(StateDelta {
                id,
                pos: now.0,
                tilt_pos: now.1,
                direction: now.2,
            });
            debug_assert!(
                pushed.is_ok(),
                "StateDelta buffer overflow — capacity math violated"
            );
            // Only record the reported state once it is actually in the buffer.
            // A dropped push leaves the slot stale so the delta re-emits next
            // call (self-healing) instead of being lost forever.
            if pushed.is_ok() {
                *slot = Some((addr, now.0, now.1, now.2));
            }
        }
    }

    /// Drain a shade's local [`PlannedTx`] buffer (capacity 4: `handle`/`tick`
    /// plan at most 2 frames each) into the caller's `tx`. The caller buffer is
    /// sized to [`TX_CAPACITY`], the structural worst case, so a failed push
    /// means the capacity math itself regressed — scream in debug builds.
    fn drain(local: &Vec<PlannedTx, 4>, tx: &mut Vec<PlannedTx, TX_CAPACITY>) {
        for t in local {
            let pushed = tx.push(*t);
            debug_assert!(
                pushed.is_ok(),
                "PlannedTx buffer overflow — capacity math violated"
            );
        }
    }

    /// Apply a command to one shade: update its motion model, queue any radio
    /// frame(s), and emit a delta if its state changed. [`DomainError::NotFound`]
    /// if the slot is empty or out of range.
    ///
    /// Plans at most 2 frames; `tx` is sized to [`TX_CAPACITY`] so a shared
    /// buffer also survives the `command_group`/`tick` worst cases.
    pub fn command_shade(
        &mut self,
        id: ShadeId,
        cmd: ShadeCommand,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<(), DomainError> {
        let shade = self.registry.shade_mut(id).ok_or(DomainError::NotFound)?;
        // Refused here rather than inside `Shade::handle`, for the same reason
        // the group gate below is: `handle` plans frames and cannot decline, so
        // a command that must not be attempted has to be stopped before it gets
        // there. A vent with no measured slat-separation band would close the
        // shade, send an Up and stop it in the same instant — a full traverse
        // that ends exactly where a plain Close would, and looks to the operator
        // like the command did nothing.
        if matches!(cmd, ShadeCommand::Vent) && shade.config.vent_band_ms == 0 {
            return Err(DomainError::VentBandNotMeasured);
        }
        let mut local: Vec<PlannedTx, 4> = Vec::new();
        let next = shade.handle(cmd, now_ms, &mut local);
        // Stored *before* the frames are drained, so a command that starts a
        // sequence this controller has no room for is refused with nothing on
        // the queue rather than half-applied — the same standard the group gate
        // below holds.
        self.set_activity(id, next)?;
        Self::drain(&local, tx);
        self.emit_if_changed(id, deltas);
        Ok(())
    }

    /// Fan a command out to every member of a group. [`DomainError::NotFound`]
    /// if the group slot is empty or out of range; an existing but empty group
    /// is `Ok(())` with no work.
    ///
    /// [`ShadeCommand::Pair`] is refused with [`DomainError::NotAGroupCommand`],
    /// **before anything is planned**. It is the one command here that is not a
    /// movement: it teaches a motor a remote address, works only while a person
    /// standing at that motor has just put it into programming mode, and has no
    /// inverse a later command can apply. Fanned across a group it is a `Prog`
    /// burst at every shade in the house with nobody at any of them.
    ///
    /// Refused structurally rather than left to whichever caller happens to
    /// build a [`ShadeCommand`] today — the same standard
    /// [`Repeats::Exactly`](crate::Repeats::Exactly) holds the burst length to.
    ///
    /// `tx` is sized to [`TX_CAPACITY`] = 32 members x 2 frames per
    /// [`Shade::handle`](crate::Shade::handle) = 64, the structural worst case of
    /// a full group commanded at once — overflow is impossible, no frame is ever
    /// dropped.
    pub fn command_group(
        &mut self,
        g: GroupId,
        cmd: ShadeCommand,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<(), DomainError> {
        // Checked before the group is even looked up, so a group that does not
        // exist and a command that may not fan out cannot be confused for one
        // another — and so no partial fan-out is possible.
        if matches!(cmd, ShadeCommand::Pair) {
            return Err(DomainError::NotAGroupCommand);
        }
        if !self.registry.group_exists(g) {
            return Err(DomainError::NotFound);
        }
        // A group holds at most SOMFY_MAX_GROUPED_SHADES (= MAX_SHADES) members;
        // collect the ids so the fan-out below can take `&mut self`.
        let members: Vec<ShadeId, { crate::registry::MAX_SHADES }> =
            self.registry.group_shades(g).collect();
        // Vent may fan out — it is a movement somebody can watch and undo, which
        // is the test `Pair` fails — but it is checked across the *whole* group
        // first. `command_shade` refuses a member whose slat-separation band was
        // never measured, and discovering that half way through would leave the
        // rest of the group already closing with no vent coming. Same standard
        // as the gate above: no partial fan-out.
        if matches!(cmd, ShadeCommand::Vent) {
            for id in &members {
                let shade = self.registry.shade(*id).ok_or(DomainError::NotFound)?;
                if shade.config.vent_band_ms == 0 {
                    return Err(DomainError::VentBandNotMeasured);
                }
            }
        }
        for id in members {
            self.command_shade(id, cmd, now_ms, tx, deltas)?;
        }
        Ok(())
    }

    /// Start timing a traverse on one shade, and queue the frame that starts
    /// it.
    ///
    /// Plans exactly one frame, so the [`TX_CAPACITY`] arithmetic above is
    /// unchanged.
    pub fn begin_calibration(
        &mut self,
        id: ShadeId,
        leg: CalibrationLeg,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<(), DomainError> {
        let shade = self.registry.shade_mut(id).ok_or(DomainError::NotFound)?;
        let mut local: Vec<PlannedTx, 4> = Vec::new();
        let run = shade.begin_calibration(leg, now_ms, &mut local);
        self.set_activity(id, Some(run))?;
        Self::drain(&local, tx);
        self.emit_if_changed(id, deltas);
        Ok(())
    }

    /// Record a moment the operator reported during a run. Transmits nothing.
    pub fn mark_calibration(
        &mut self,
        id: ShadeId,
        mark: CalibrationMark,
        now_ms: u64,
    ) -> Result<(), DomainError> {
        let mut run = self.run(id).ok_or(DomainError::NotCalibrating)?;
        run.mark(mark, now_ms);
        self.set_activity(id, Some(Activity::Calibrating(run)))
    }

    /// End a run and store what it measured. Transmits nothing — the traverse is
    /// over, and the run ends at a limit the motor stopped itself at, which is
    /// why the estimate can be re-anchored there.
    ///
    /// Emits a delta, because that re-anchoring may move the reported position.
    pub fn finish_calibration(
        &mut self,
        id: ShadeId,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<CalibrationOutcome, DomainError> {
        let run = self.run(id).ok_or(DomainError::NotCalibrating)?;
        let outcome = self
            .registry
            .shade_mut(id)
            .ok_or(DomainError::NotFound)?
            .finish_calibration(run, now_ms)?;
        // Cleared only once the run has been accepted: a refused one leaves the
        // conversation open so the operator can tap again rather than start
        // over.
        self.set_activity(id, None)?;
        self.emit_if_changed(id, deltas);
        Ok(outcome)
    }

    /// Abandon a run, storing nothing. Transmits nothing, and deliberately does
    /// **not** stop the shade: the operator cancelling a measurement has not
    /// asked for the motor to halt where it happens to be, and a `My` they did
    /// not ask for is a shade left mid-window.
    pub fn cancel_calibration(&mut self, id: ShadeId) -> Result<(), DomainError> {
        if self.run(id).is_none() {
            return Err(DomainError::NotCalibrating);
        }
        self.set_activity(id, None)
    }

    /// Route a decoded RX frame to the shade that owns its address (own or a
    /// linked remote), tracking the estimate without retransmitting. Repeats of
    /// the same press within [`RX_DEDUPE_WINDOW_MS`] and frames from unknown
    /// addresses are ignored.
    pub fn on_rx_frame(
        &mut self,
        frame: &Frame,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) {
        // `RxDeduper` keys on a u32 monotonic clock (Plan 1 API). Truncating the
        // u64 is safe here: the deduper's arithmetic is wrapping and the window
        // (2 s) is far shorter than the ~49.7-day u32 ms rollover.
        if !self.dedupe.accept(frame, now_ms as u32) {
            return;
        }
        let Some(id) = self.registry.shade_by_address(frame.address) else {
            return;
        };
        if let Some(shade) = self.registry.shade_mut(id) {
            shade.apply_overheard(frame.command, now_ms);
        }
        // A wall remote has taken the motor over, so whatever this controller
        // had in flight is over too — including a calibration run, whose timing
        // is now against a movement somebody else caused. Infallible: clearing
        // never needs a slot.
        let _ = self.set_activity(id, None);
        self.emit_if_changed(id, deltas);
    }

    /// Advance every shade to `now_ms`: plan any arrival-stop frames and emit a
    /// delta for each shade whose state changed since its last reported one.
    ///
    /// Plans at most 32 frames (32 shades x at most 1 arrival-stop each from
    /// [`Shade::tick`](crate::Shade::tick)); `tx` is sized to [`TX_CAPACITY`] =
    /// 64 so overflow is impossible even in the larger `command_group` worst
    /// case.
    pub fn tick(
        &mut self,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) {
        let ids: Vec<ShadeId, { crate::registry::MAX_SHADES }> =
            self.registry.shades().map(|(id, _)| id).collect();
        for id in ids {
            let activity = self.activity(id);
            if let Some(shade) = self.registry.shade_mut(id) {
                let mut local: Vec<PlannedTx, 4> = Vec::new();
                shade.tick(now_ms, &mut local);
                // The multi-step movements, driven here rather than inside the
                // shade because that is where they are stored — see
                // `MAX_ACTIVITIES`.
                let next =
                    activity.and_then(|activity| shade.advance(activity, now_ms, &mut local));
                Self::drain(&local, tx);
                // Infallible: the entry already exists, so this replaces or
                // clears rather than inserting.
                let _ = self.set_activity(id, next);
            }
            self.emit_if_changed(id, deltas);
        }
    }
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod delta_overflow {
    //! Fix-4 regression: `emit_if_changed` pushes the delta BEFORE updating its
    //! cache slot, so a dropped push cannot permanently suppress a state. The
    //! real `DELTA_CAPACITY` buffer never overflows, so these drive the private
    //! generic helper with a deliberately-undersized buffer to exercise the
    //! overflow path directly.
    use super::*;
    use crate::ShadeConfig;

    /// Two shades, each driven into a moving (non-[`RESTING`]) state so both
    /// would emit a delta.
    fn two_moving_shades() -> (Controller, ShadeId, ShadeId) {
        let mut c = Controller::new();
        let a = c
            .registry
            .add_shade(ShadeConfig::new("A", 0x101).unwrap())
            .unwrap();
        let b = c
            .registry
            .add_shade(ShadeConfig::new("B", 0x102).unwrap())
            .unwrap();
        let mut scratch: Vec<PlannedTx, 4> = Vec::new();
        c.registry
            .shade_mut(a)
            .unwrap()
            .handle(ShadeCommand::Down, 0, &mut scratch);
        scratch.clear();
        c.registry
            .shade_mut(b)
            .unwrap()
            .handle(ShadeCommand::Down, 0, &mut scratch);
        (c, a, b)
    }

    /// Debug contract: an overflowing push trips the `debug_assert!` and panics
    /// ("scream in debug"). The pre-fix code updated the cache before a silent
    /// drop and never panicked, so this is a genuine failing-first guard.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "StateDelta buffer overflow")]
    fn overflow_screams_in_debug() {
        let (mut c, a, b) = two_moving_shades();
        let mut small: Vec<StateDelta, 1> = Vec::new();
        c.emit_if_changed(a, &mut small); // fills the 1-slot buffer
        c.emit_if_changed(b, &mut small); // overflow -> debug_assert panics
    }

    /// Release contract: an overflowing push is non-fatal AND self-healing —
    /// the dropped shade's cache slot stays stale, so its delta re-emits on the
    /// next call instead of being lost forever. (Pre-fix, the cache was updated
    /// before the drop, permanently suppressing the state.)
    #[cfg(not(debug_assertions))]
    #[test]
    fn dropped_delta_reemits_in_release() {
        let (mut c, a, b) = two_moving_shades();
        let mut small: Vec<StateDelta, 1> = Vec::new();
        c.emit_if_changed(a, &mut small);
        c.emit_if_changed(b, &mut small); // dropped: buffer full
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].id, a);
        let mut fresh: Vec<StateDelta, 1> = Vec::new();
        c.emit_if_changed(b, &mut fresh);
        assert_eq!(fresh.len(), 1, "dropped delta must re-emit, not vanish");
        assert_eq!(fresh[0].id, b);
    }
}
