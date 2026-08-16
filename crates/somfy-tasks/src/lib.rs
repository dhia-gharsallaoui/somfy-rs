//! # somfy-tasks
//!
//! The two loops the firmware runs as Embassy tasks, and the channels between
//! them — with no `esp-hal` type anywhere, so all of it can be exercised on the
//! host.
//!
//! `crates/firmware` cannot be compiled for a host target at all (`esp-hal`'s
//! build script rejects one), so a task body written there is a task body no
//! test can ever reach. What lives there instead is the handful of lines that
//! genuinely need a chip: an executor, an `#[embassy_executor::task]` wrapper
//! per loop, and the implementations of the two traits below. Everything those
//! wrappers actually *do* is here.
//!
//! ## The two loops
//!
//! - [`RadioLoop`] — sole owner of the radio. It consumes
//!   [`TransmitRequest`](somfy_store::TransmitRequest)s and publishes decoded
//!   [`Frame`](somfy_rts::Frame)s. It is written against
//!   [`somfy_rmt::PulseSource`] and [`Transmitter`], so
//!   [`somfy_rmt::ReplayPulseSource`] and a recording transmitter drive the
//!   whole thing on the host from a real wall-remote capture.
//! - [`StateMachine`] — owns the `somfy-domain` `Controller`. Commands and
//!   overheard frames go in; every frame the domain plans leaves through
//!   [`somfy_store::transmit`], which is the only path to a queue.
//!
//! They are split because radio timing must never wait on anything else. The
//! state task's heaviest operation is a flash commit, which on this hardware
//! disables interrupts for as long as an erase takes; [`StateMachine`]'s docs
//! say exactly what that costs the receiver and why the cost lands where it
//! does the least harm.
//!
//! ## The producer end is not exposed
//!
//! `somfy-store` makes it impossible to enqueue a frame without having
//! committed its rolling code first — [`TransmitTicket`](somfy_store::TransmitTicket)
//! is unforgeable and `enqueue` takes nothing else. That argument only holds if
//! the queue implementation has no *second* door: an inherent `send`, a public
//! field, a getter handing back the underlying channel sender.
//! [`TransmitQueueHandle`] is where that door is kept shut, and its docs carry
//! the argument.

#![cfg_attr(not(test), no_std)]

mod queue;
mod radio;
mod state;

pub use queue::{QueueFull, TransmitChannel, TransmitQueueHandle, TRANSMIT_QUEUE_DEPTH};
pub use radio::{FrameChannel, RadioEvent, RadioLoop, Transmitter, FRAME_QUEUE_DEPTH};
pub use state::{
    CommandChannel, ControlCommand, DeltaChannel, Dispatch, StateMachine, TxProfile,
    COMMAND_QUEUE_DEPTH, DEFAULT_REPEATS, DELTA_QUEUE_DEPTH, DELTA_SUBSCRIBERS,
};
