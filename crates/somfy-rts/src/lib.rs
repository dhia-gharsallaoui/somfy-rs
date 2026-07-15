#![cfg_attr(not(test), no_std)]

mod command;
mod frame;
mod pulse;
mod rolling;
mod rx;

pub use command::Command;
pub use frame::{decode56, encode56, Frame, FrameError};
pub use pulse::{render_pulses, FrameKind, Pulse, TIMINGS};
pub use rolling::RollingCode;
pub use rx::{RxDecoder, RxFrame};
