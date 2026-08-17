use crate::{Direction, Pos, TravelProfile};

/// One axis of open-loop dead-reckoned movement. RTS is a one-way protocol
/// and the motor never reports its position back, so there is no feedback
/// to correct against — the only option is to estimate position by timing:
/// direction is derived from position vs target every tick, and while
/// moving, the position is `start_offset + elapsed` as a ratio of the
/// direction's travel time.
///
/// Integer-only: all math in `u64` ms and `u16` hundredths-of-percent — a
/// deterministic integer replacement for the floating-point percentage
/// model deployed controllers use (see crate docs). Sun/wind/dry-contact/
/// tilt inputs are deliberately out of scope here; this estimator only
/// tracks lift position over time.
///
/// # Two deliberate divergences from the model this was ported from
///
/// The controller this estimator reproduces integrates from the instant a
/// command is planned, at one flat rate per direction. Both of those are wrong
/// on the hardware in front of us, and both are corrected here rather than
/// faithfully preserved. [`TravelProfile`] carries the picture; in short:
///
/// 1. **Motion does not begin when a command is planned.** The frame takes time
///    on air and the motor has a soft-start ramp, so the first
///    [`TravelProfile::start_lag_ms`] of a commanded move produce no position
///    change and are not integrated.
/// 2. **Travel is not linear at the closed end.** Leaving [`Pos::FULL`] upward,
///    a perforated-slat shutter spends [`TravelProfile::vent_band_ms`]
///    separating its slats before the curtain rises; closing, it spends
///    [`TravelProfile::close_band_ms`] compressing them after the curtain has
///    reached the sill. Neither interval moves the position.
///
/// Both reduce to the original model exactly when their figures are zero, which
/// is what an un-calibrated shade carries. `docs/provenance.md` records the
/// divergence; the requirements that asked for it are R5 and R8 of
/// `docs/specs/2026-08-15-position-accuracy-requirements.md`.
#[derive(Debug, Clone, Copy)]
pub struct Motion {
    pos: Pos,
    target: Pos,
    start_pos: Pos,
    move_start_ms: u64,
}

/// Result of a single [`Motion::tick`]. `arrived` is true only on the tick
/// that first reaches the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionSnapshot {
    pub pos: Pos,
    pub direction: Direction,
    pub arrived: bool,
}

const FULL_RAW: u64 = 10_000;

impl Motion {
    pub fn new(start: Pos) -> Motion {
        Motion {
            pos: start,
            target: start,
            start_pos: start,
            move_start_ms: 0,
        }
    }

    pub fn pos(&self) -> Pos {
        self.pos
    }

    pub fn target(&self) -> Pos {
        self.target
    }

    /// Where the move in progress began — the anchor the tick math integrates
    /// forward from, and therefore what the distance covered so far is measured
    /// against.
    pub fn start_pos(&self) -> Pos {
        self.start_pos
    }

    /// Direction is recomputed from live position vs target every tick,
    /// rather than stored as state: `pos == target` is idle, `pos > target`
    /// is moving toward open ([`Direction::Up`]), `pos < target` is moving
    /// toward closed ([`Direction::Down`]).
    pub fn direction(&self) -> Direction {
        use core::cmp::Ordering::*;
        match self.pos.cmp(&self.target) {
            Equal => Direction::Idle,
            Greater => Direction::Up,
            Less => Direction::Down,
        }
    }

    /// Records where and when movement began: the start position and start
    /// time are captured whenever a non-idle move begins. The tick math
    /// integrates forward from this anchor.
    pub fn set_target(&mut self, target: Pos, now_ms: u64) {
        self.target = target;
        self.start_pos = self.pos;
        self.move_start_ms = now_ms;
    }

    /// Freeze at the live computed position. The target collapses onto the
    /// continuously-updated live position, so the next tick derives
    /// [`Direction::Idle`] and the estimator stops there — this is how a
    /// stop command (`My`) is applied to an in-progress estimate. We
    /// advance to the live position first, then pin `target`/`start_pos`
    /// to it.
    pub fn halt(&mut self, now_ms: u64, travel: TravelProfile) {
        let s = self.tick(now_ms, travel);
        self.target = s.pos;
        self.start_pos = s.pos;
    }

    /// Place the estimate at a physical limit and treat it as the new anchor.
    ///
    /// # The one source of ground truth this protocol has
    ///
    /// The motor stops itself at its own end stops, and it does that whatever
    /// this controller believes. So a move that ended at a limit is the only
    /// event in the whole system that says where the shade *is* rather than
    /// where it is calculated to be — and taking it is what stops error
    /// accumulating monotonically across partial moves.
    /// (`docs/specs/2026-08-15-position-accuracy-requirements.md` R3.)
    ///
    /// The controller this crate was ported from snaps the position to the limit
    /// too, but only as a clamp inside its integrator; it carries no error term,
    /// so it has nothing to reset. Zeroing the accumulated uncertainty is
    /// [`Shade`](crate::Shade)'s half of the same event.
    pub fn resync(&mut self, limit: Pos, now_ms: u64) {
        self.pos = limit;
        self.target = limit;
        self.start_pos = limit;
        self.move_start_ms = now_ms;
    }

    /// Milliseconds until the estimate reaches its target, or `None` when idle.
    ///
    /// Counts the part of the dead phase still ahead as well as the travel
    /// itself, so it is honest during the lag at the start of a move rather than
    /// reporting a move that has not begun as nearly over.
    ///
    /// # What reads it
    ///
    /// The arrival stop, which has to be sent **early**. A `My` frame takes the
    /// same [`TravelProfile::start_lag_ms`] to reach the motor as the command
    /// that started the move, and the motor keeps travelling throughout — so a
    /// stop planned at the moment the estimate reaches the target arrives one
    /// lag late and overshoots by that much. Planning it a lag early is what
    /// makes the motor stop where it was asked to.
    ///
    /// Redundancy costs nothing here: repeat frames follow the first, and the
    /// motor acts on the first one it hears in full, so R1's longer stop burst
    /// does not lengthen this delay.
    pub fn remaining_ms(&self, now_ms: u64, travel: TravelProfile) -> Option<u64> {
        let going_down = match self.direction() {
            Direction::Idle => return None,
            Direction::Down => true,
            Direction::Up => false,
        };
        let span_ms = travel.span_ms(going_down) as u64;
        let dead_ms = self.dead_ms(going_down, travel);
        let dead_left = dead_ms.saturating_sub(now_ms.saturating_sub(self.move_start_ms));
        let gap_raw = self.pos.raw().abs_diff(self.target.raw()) as u64;
        Some(dead_left + gap_raw * span_ms / FULL_RAW)
    }

    /// Milliseconds at the start of this move during which the position does not
    /// change.
    ///
    /// Always the start lag. Additionally the slat-separation band when — and
    /// only when — the move is upward *from the closed limit*, because that is
    /// the only place the slats are known to be compressed shut. Anywhere else
    /// they are already apart and the curtain rises immediately.
    ///
    /// The test is `start_pos == Pos::FULL` rather than "near the bottom" on
    /// purpose: the estimate reads exactly [`Pos::FULL`] only when it has been
    /// snapped there by reaching the limit, so this fires on knowledge rather
    /// than on a number that happens to be close.
    fn dead_ms(&self, going_down: bool, travel: TravelProfile) -> u64 {
        let lag = travel.start_lag_ms as u64;
        if !going_down && self.start_pos == Pos::FULL {
            lag + travel.vent_band_ms as u64
        } else {
            lag
        }
    }

    /// Re-anchor an in-progress move at where it has actually reached, keeping
    /// the same target.
    ///
    /// # Why this exists
    ///
    /// [`Motion::tick`] computes position **absolutely** from `start_pos`,
    /// `move_start_ms` and the travel time — it does not integrate
    /// incrementally. That is deliberate and it is what makes the estimate
    /// immune to a missed tick, but it means the travel time is read as though
    /// it had applied for the whole move. So changing a travel time *while a
    /// shade is moving* re-interprets everything that already happened.
    ///
    /// Concretely, and this is the case it was written for: a shade travelling
    /// down with a 30 s time is 10 s in, so about 33% closed. An operator
    /// times it with a stopwatch and saves 10 s — which is the whole of the
    /// calibration workflow the position-accuracy requirements ask for. The
    /// next tick computes `elapsed = 10000`, clamps it to the new
    /// `travel_ms = 10000`, and reports **arrived, fully closed**: the
    /// controller plans a stop that halts the motor at 33% and then tells
    /// everybody the shade is shut.
    ///
    /// Re-anchoring first makes the new time apply only to the travel that has
    /// not happened yet, which is the only reading of it that can be right —
    /// nothing knows what the *old* number should have been.
    pub fn reanchor(&mut self, now_ms: u64, travel: TravelProfile) {
        // Advance to where the *old* travel times say it is, then treat that as
        // the new starting point. The order matters: reading the position after
        // the config changed would be reading it through the very number that
        // has just moved.
        let snapshot = self.tick(now_ms, travel);
        self.start_pos = snapshot.pos;
        self.move_start_ms = now_ms;
    }

    /// Advance the estimate for one tick, applying the direction-specific
    /// integration formula below (downward and upward travel are
    /// integrated from opposite ends, see the branches inline).
    pub fn tick(&mut self, now_ms: u64, travel: TravelProfile) -> MotionSnapshot {
        let dir = self.direction();
        // Resolve the moving direction to a single flag; [`Direction::Idle`]
        // returns early, so the rest of the method never needs an `Up`-aliasing
        // catch-all — `going_down` carries the branch explicitly.
        let going_down = match dir {
            Direction::Idle => {
                return MotionSnapshot {
                    pos: self.pos,
                    direction: Direction::Idle,
                    arrived: false,
                };
            }
            Direction::Down => true,
            Direction::Up => false,
        };

        // The *lifting* span, not the whole traverse: the start lag and the
        // direction's dead band are intervals inside the traverse during which
        // the curtain does not move, so what is left is what the position is a
        // ratio of. With both at zero — an un-calibrated shade — this is the
        // traverse itself and every line below is unchanged.
        let span_ms = travel.span_ms(going_down) as u64;
        // Elapsed time that has actually moved the curtain. Ahead of it sits the
        // command's dead phase; see `Motion::dead_ms`.
        let moving_ms = now_ms
            .saturating_sub(self.move_start_ms)
            .saturating_sub(self.dead_ms(going_down, travel));
        let start_raw = self.start_pos.raw() as u64;

        let new_pos = if span_ms == 0 {
            // No known lifting duration for this direction — either no travel
            // time is configured, or the compensations consume all of it, which
            // `ShadeConfig::checked_bands` refuses at every boundary a value
            // enters through. Treat the move as an instant jump to the target
            // rather than dividing by zero.
            self.target
        } else if going_down {
            // Moving down: express the start position as how many ms of
            // travel it represents from fully open (ms_from_0), add the ms
            // spent moving since this move started, and clamp to the span
            // so the estimate never overruns. Converting that clamped ms
            // value back to a position ratio gives the new position.
            //
            // The closing dead band needs no term of its own here: it is
            // time spent compressing the slats *after* the curtain reaches
            // the sill, so the clamp is already holding the position at
            // `Pos::FULL` throughout it.
            let ms_from_0 = start_raw * span_ms / FULL_RAW + moving_ms;
            let ratio = ms_from_0.min(span_ms) * FULL_RAW / span_ms;
            Pos::from_raw(ratio as u16)
        } else {
            // Moving up: mirror the down-branch math from the closed end.
            // `consumed` is how many ms of up-travel it took to reach the
            // start position starting from fully closed; subtracting that
            // from the span and adding the moving time gives the remaining
            // up-travel ms, clamped to the span. The clamped value is a
            // ratio measured from the closed end, so flipping it
            // (FULL_RAW - ratio) converts it back to this crate's
            // fully-open-relative position scale. The integer-division
            // floor here is placed deliberately, not simplified to an
            // equivalent-looking expression (see report deviation note for
            // why the placement matters).
            //
            // The slat-separation band is not a term here either: it is
            // charged against `moving_ms` up front by `dead_ms`, because
            // unlike the closing band it happens *before* the curtain
            // starts to rise rather than after it stops.
            let consumed = start_raw * span_ms / FULL_RAW;
            let ms_from_100 = span_ms - consumed + moving_ms;
            let ratio = ms_from_100.min(span_ms) * FULL_RAW / span_ms;
            Pos::from_raw((FULL_RAW - ratio) as u16)
        };

        // The per-tick integration can overshoot the target between one
        // tick and the next (a discrete step landing past a continuous
        // target), so snap to the target exactly on crossing rather than
        // reporting the overshot value — this is what lets the caller
        // detect arrival cleanly.
        let crossed = if going_down {
            new_pos >= self.target
        } else {
            new_pos <= self.target
        };
        self.pos = if crossed { self.target } else { new_pos };
        MotionSnapshot {
            pos: self.pos,
            direction: if crossed { Direction::Idle } else { dir },
            arrived: crossed,
        }
    }
}
