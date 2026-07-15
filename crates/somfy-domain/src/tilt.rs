use crate::{Direction, Pos, TiltMode};

/// C++ integrated-tilt sequencing rule (Somfy.cpp:1072): an integrated
/// tilt motor must fully tilt open (0) before the shade lifts, and
/// fully tilt closed (FULL) before it lowers.
///
/// Ports the exact predicate:
/// `tiltType == integrated && ((direction == -1 && currentTiltPos != 0) ||
/// (direction == 1 && currentTiltPos != 100))`, where `direction == -1`
/// is [`Direction::Up`] (toward open/0) and `direction == 1` is
/// [`Direction::Down`] (toward closed/100), per Somfy.cpp:1071.
///
/// The tilt axis itself needs no new type: it is a [`crate::Motion`]
/// driven with `tilt_time_ms` as both travel times.
pub fn tilt_first(mode: TiltMode, lift_dir: Direction, tilt_pos: Pos) -> bool {
    if mode != TiltMode::Integrated {
        return false;
    }
    match lift_dir {
        Direction::Up => tilt_pos != Pos::ZERO,
        Direction::Down => tilt_pos != Pos::FULL,
        Direction::Idle => false,
    }
}
