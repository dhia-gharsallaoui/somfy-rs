use crate::{Direction, Pos, TiltMode};

/// Integrated-tilt sequencing rule: when the tilt axis and lift axis share
/// one motor (`TiltMode::Integrated`), the motor cannot move the lift axis
/// until the tilt axis has finished traveling to the end matching the lift
/// direction — fully open (0) before lifting up, fully closed (`FULL`)
/// before lowering down. For every other tilt mode this gate does not
/// apply.
///
/// The predicate: for `Integrated` mode only, moving up ([`Direction::Up`],
/// toward open/0) is blocked while tilt isn't yet fully open, and moving
/// down ([`Direction::Down`], toward closed/100) is blocked while tilt
/// isn't yet fully closed. Idle lift direction never blocks.
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
