use crate::{Direction, Pos};

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
    pub fn halt(&mut self, now_ms: u64, up_time_ms: u32, down_time_ms: u32) {
        let s = self.tick(now_ms, up_time_ms, down_time_ms);
        self.target = s.pos;
        self.start_pos = s.pos;
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
    pub fn reanchor(&mut self, now_ms: u64, up_time_ms: u32, down_time_ms: u32) {
        // Advance to where the *old* travel times say it is, then treat that as
        // the new starting point. The order matters: reading the position after
        // the config changed would be reading it through the very number that
        // has just moved.
        let snapshot = self.tick(now_ms, up_time_ms, down_time_ms);
        self.start_pos = snapshot.pos;
        self.move_start_ms = now_ms;
    }

    /// Advance the estimate for one tick, applying the direction-specific
    /// integration formula below (downward and upward travel are
    /// integrated from opposite ends, see the branches inline).
    pub fn tick(&mut self, now_ms: u64, up_time_ms: u32, down_time_ms: u32) -> MotionSnapshot {
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

        let travel_ms = if going_down { down_time_ms } else { up_time_ms } as u64;
        let elapsed = now_ms.saturating_sub(self.move_start_ms);
        let start_raw = self.start_pos.raw() as u64;

        let new_pos = if travel_ms == 0 {
            // Zero travel time means the motor has no known travel duration
            // for this direction, so treat the move as an instant jump to
            // the target rather than dividing by zero.
            self.target
        } else if going_down {
            // Moving down: express the start position as how many ms of
            // travel it represents from fully open (ms_from_0), add the ms
            // elapsed since this move started, and clamp to the full
            // travel time so the estimate never overruns. Converting that
            // clamped ms value back to a position ratio gives the new
            // position.
            let ms_from_0 = start_raw * travel_ms / FULL_RAW + elapsed;
            let ratio = ms_from_0.min(travel_ms) * FULL_RAW / travel_ms;
            Pos::from_raw(ratio as u16)
        } else {
            // Moving up: mirror the down-branch math from the closed end.
            // `consumed` is how many ms of up-travel it took to reach the
            // start position starting from fully closed; subtracting that
            // from the full travel time and adding elapsed gives the
            // remaining up-travel ms, clamped to the travel time. The
            // clamped value is a ratio measured from the closed end, so
            // flipping it (FULL_RAW - ratio) converts it back to this
            // crate's fully-open-relative position scale. The integer-
            // division floor here is placed deliberately, not simplified
            // to an equivalent-looking expression (see report deviation
            // note for why the placement matters).
            let consumed = start_raw * travel_ms / FULL_RAW;
            let ms_from_100 = travel_ms - consumed + elapsed;
            let ratio = ms_from_100.min(travel_ms) * FULL_RAW / travel_ms;
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
