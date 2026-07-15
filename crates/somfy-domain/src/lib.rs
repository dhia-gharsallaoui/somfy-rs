#![cfg_attr(not(test), no_std)]

mod motion;
mod types;

pub use motion::{Motion, MotionSnapshot};
pub use types::{Direction, DomainError, Pos, ShadeConfig, ShadeKind, TiltMode};
