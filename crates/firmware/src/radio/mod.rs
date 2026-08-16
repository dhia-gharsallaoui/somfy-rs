//! The radio: everything between an encoded Somfy frame and the air.
//!
//! Only code that genuinely needs `esp-hal` lives here. Frame encoding is
//! `somfy-rts`, the symbol pipeline is `somfy-rmt`, and the CC1101's register
//! set is `somfy-cc1101` — all three are host-testable crates precisely because
//! they are not in here. This crate cannot be compiled for the host at all
//! (esp-hal's build script rejects a host target), so anything placed here is
//! anything that can only be checked on a chip. Keep it small.

pub mod air;
pub mod rmt_rx;
pub mod rmt_tx;
