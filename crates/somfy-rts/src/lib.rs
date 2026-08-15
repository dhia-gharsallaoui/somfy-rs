//! # somfy-rts
//!
//! `no_std` Somfy RTS protocol engine: 56/80-bit frame encoding and decoding,
//! rolling-code management, OOK pulse-train rendering (TX) and pulse-stream
//! decoding (RX).
//!
//! This crate is hardware-free. TX produces [`Pulse`] sequences for any replay
//! mechanism (ESP32 RMT, tests); RX consumes measured [`Pulse`] sequences from
//! any capture source (RMT RX, GPIO interrupts, files). No GPIO, timer, or
//! radio type appears anywhere in its API.
//!
//! ## What it does
//!
//! - **Frames** — [`encode56`]/[`decode56`] and [`encode80`]/[`decode80`]
//!   handle both RTS frame widths, including the RTS XOR-chain obfuscation and
//!   checksum. [`Frame`] is the shared model; [`Command`] covers the RTS
//!   command set (Up/Down/My plus the 80-bit StepUp/StepDown/Favorite/Stop
//!   extensions).
//! - **Rolling codes** — [`RollingCode`] mirrors the C++ increment-on-send wire
//!   sequence. Storage is *next-to-send* (the C++ persists *last-sent*), so
//!   migrated values need a `+1`; the caller must persist before transmitting
//!   (see [`RollingCode`] docs).
//! - **TX** — [`render_pulses`] turns a frame into an OOK pulse train (wake-up,
//!   hardware/software sync, 640µs Manchester half-symbols (`SYMBOL`,
//!   Somfy.cpp:23), inter-frame gap, reduced-sync repeats). Timing constants
//!   live in [`TIMINGS`], ported verbatim from the C++ transmitter with
//!   per-constant source lines.
//! - **RX** — [`RxDecoder`] is a single level-aware state machine that decodes
//!   **both** pulse representations: merged edge-to-edge streams (what real
//!   `CHANGE`-interrupt hardware and the firmware's `rx.pulses[]` captures
//!   produce) and the unmerged half-symbol streams [`render_pulses`] emits.
//!   [`RxFrame`] carries the decoded result.
//! - **Dedupe** — [`RxDeduper`] collapses the N repeat frames of one button
//!   press into a single logical event, keyed on `(address, rolling_code)`
//!   within a time window.
//!
//! ## Validation
//!
//! The suite runs entirely on the host: software TX/RX loopback, per-layer unit
//! tests, and property tests. Golden fixtures under `tests/fixtures/` pin the
//! engine against pulses the reference C++ firmware actually produced; a
//! checked-in *synthetic* capture exercises the loader on every CI run, while
//! the real-device captures (and their three `#[ignore]`d tests) are pending one
//! capture session on a running device — see `tests/fixtures/README.md`.
//!
//! Reference implementation: ESPSomfy-RTS (C++ / Arduino).

#![cfg_attr(not(test), no_std)]

mod command;
mod dedupe;
mod frame;
mod pulse;
mod rolling;
mod rx;

pub use command::Command;
pub use dedupe::RxDeduper;
pub use frame::{decode56, decode80, encode56, encode80, Frame, FrameError};
pub use pulse::{render_pulses, FrameKind, Pulse, TIMINGS};
pub use rolling::RollingCode;
pub use rx::{RxDecoder, RxFrame};

/// Test-only: expose de-obfuscation so integration tests can assert on the
/// raw wire bytes (the C++ tail map is defined pre-obfuscation).
#[doc(hidden)]
pub fn deobfuscate_for_test(b: &mut [u8; 10]) {
    frame::deobfuscate_slice(b)
}
