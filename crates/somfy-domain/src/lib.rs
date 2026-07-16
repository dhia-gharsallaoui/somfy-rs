#![cfg_attr(not(test), no_std)]

mod motion;
mod registry;
mod shade;
mod tilt;
mod types;

pub use motion::{Motion, MotionSnapshot};
pub use registry::{GroupId, Registry, RoomId, ShadeId};
pub use shade::{PlannedTx, Shade, ShadeCommand};
pub use tilt::tilt_first;
pub use types::{Direction, DomainError, Pos, ShadeConfig, ShadeKind, TiltMode};
