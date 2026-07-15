use crate::{Direction, Pos};

/// One axis of dead-reckoned movement. Port of the position math in
/// `SomfyShade::checkMovement` (Somfy.cpp:1052-1234): direction is derived
/// from position vs target every tick; while moving, the position is
/// `start_offset + elapsed` as a ratio of the direction's travel time.
///
/// Integer-only: all math in `u64` ms and `u16` hundredths-of-percent
/// (intentional deviation from the C++ float model — see crate docs).
/// Sun/wind/dry-contact/tilt logic from the C++ is deliberately excluded.
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

    /// Direction is recomputed from live position vs target every tick.
    /// Mirrors Somfy.cpp:1071:
    /// `pos == target ? 0 : pos > target ? -1 : 1` where -1 is toward open
    /// ([`Direction::Up`]) and +1 is toward closed ([`Direction::Down`]).
    pub fn direction(&self) -> Direction {
        use core::cmp::Ordering::*;
        match self.pos.cmp(&self.target) {
            Equal => Direction::Idle,
            Greater => Direction::Up,
            Less => Direction::Down,
        }
    }

    /// Records where and when movement began, like the C++
    /// `setTarget` + `setMovement` pair (Somfy.cpp:2754-2764: `moveStart`
    /// and `startPos` are captured when a non-idle move starts). The tick
    /// math integrates forward from this anchor.
    pub fn set_target(&mut self, target: Pos, now_ms: u64) {
        self.target = target;
        self.start_pos = self.pos;
        self.move_start_ms = now_ms;
    }

    /// Freeze at the live computed position. Mirrors the C++ `My`/stop path
    /// (Somfy.cpp:2437: `p_target(currentPos)`), where the target collapses
    /// onto the continuously-updated live position so the next tick derives
    /// [`Direction::Idle`] and stops. We advance to the live position first,
    /// then pin `target`/`start_pos` to it.
    pub fn halt(&mut self, now_ms: u64, up_time_ms: u32, down_time_ms: u32) {
        let s = self.tick(now_ms, up_time_ms, down_time_ms);
        self.target = s.pos;
        self.start_pos = s.pos;
    }

    /// Advance the estimate. Port of the down branch (Somfy.cpp:1125-1182)
    /// and the mirrored up branch (Somfy.cpp:1183-1234).
    pub fn tick(&mut self, now_ms: u64, up_time_ms: u32, down_time_ms: u32) -> MotionSnapshot {
        let dir = self.direction();
        if dir == Direction::Idle {
            return MotionSnapshot {
                pos: self.pos,
                direction: Direction::Idle,
                arrived: false,
            };
        }

        let travel_ms = match dir {
            Direction::Down => down_time_ms,
            _ => up_time_ms,
        } as u64;
        let elapsed = now_ms.saturating_sub(self.move_start_ms);
        let start_raw = self.start_pos.raw() as u64;

        let new_pos = if travel_ms == 0 {
            // Zero travel time = instant jump (Somfy.cpp:1126-1129 / 1184-1186).
            self.target
        } else {
            match dir {
                Direction::Down => {
                    // Somfy.cpp:1136-1143: msFrom0 = floor(startPos/100 * downTime)
                    // + elapsed, clamped to downTime.
                    let ms_from_0 = start_raw * travel_ms / FULL_RAW + elapsed;
                    let ratio = ms_from_0.min(travel_ms) * FULL_RAW / travel_ms;
                    Pos::from_raw(ratio as u16)
                }
                _ => {
                    // Somfy.cpp:1193-1201: msFrom100 = upTime
                    // - floor(startPos/100 * upTime) + elapsed, clamped to upTime.
                    // Faithful floor placement (see report deviation note).
                    let consumed = start_raw * travel_ms / FULL_RAW;
                    let ms_from_100 = travel_ms - consumed + elapsed;
                    let ratio = ms_from_100.min(travel_ms) * FULL_RAW / travel_ms;
                    Pos::from_raw((FULL_RAW - ratio) as u16)
                }
            }
        };

        // Snap to target on crossing (Somfy.cpp:1161-1162 down, 1212-1213 up).
        let crossed = match dir {
            Direction::Down => new_pos >= self.target,
            _ => new_pos <= self.target,
        };
        self.pos = if crossed { self.target } else { new_pos };
        MotionSnapshot {
            pos: self.pos,
            direction: if crossed { Direction::Idle } else { dir },
            arrived: crossed,
        }
    }
}
