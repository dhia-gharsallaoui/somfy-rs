#![cfg_attr(not(test), no_std)]

mod motion;
mod tilt;
mod types;

pub use motion::{Motion, MotionSnapshot};
pub use tilt::tilt_first;
pub use types::{Direction, DomainError, Pos, ShadeConfig, ShadeKind, TiltMode};
