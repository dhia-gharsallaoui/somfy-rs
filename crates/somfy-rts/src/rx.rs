//! RTS receive-side decoder: a state machine that turns a stream of measured
//! OOK [`Pulse`]s back into the raw frame bytes.
//!
//! Ported from the C++ `somfy_rx_t` state machine (`ESPSomfy-RTS/src/Somfy.h`
//! lines 89-116 and its `Transceiver::handleReceive` driver in
//! `src/Somfy.cpp:4384-4516`). The sync-acquisition half (hardware-sync
//! counting, `>= 4` before accepting the software sync, and the `bit_length`
//! selection switch) mirrors the C++ verbatim.
//!
//! The data-phase decode intentionally *diverges* from the C++ ISR, and the
//! reason is the pulse representation. The C++ ISR runs off a `CHANGE`
//! interrupt, so it only ever measures *edge-to-edge* durations: physically
//! adjacent same-level half-symbols are already merged into one segment by the
//! hardware, which is why the C++ decodes transitions (toggling `previous_bit`
//! on a full 2*SYMBOL segment) and ignores pulse polarity. Our TX layer
//! ([`render_pulses`]) deliberately does *not* merge adjacent same-level
//! half-symbols (see its doc comment), so the loopback stream this decoder
//! consumes is a sequence of discrete `SYMBOL`-length half-pulses whose
//! polarity is meaningful. Decoding that stream by transition-toggling would
//! collapse every bit to the same value; instead we read the Manchester bit
//! straight off the second half-symbol's level (`bit == second half is high`,
//! MSB-first), exactly matching the polarity [`render_pulses`] emits.
//!
//! This unmerged form is strictly more informative than the merged one: it
//! preserves the boundary between a frame's final `0` half-symbol and the
//! inter-frame gap, which a merging decoder would lose. A future real-radio RX
//! driver that only sees edges would need the C++ transition algorithm (or a
//! merge shim in front of this decoder).

use crate::pulse::{Pulse, TIMINGS};
use heapless::Vec;

/// A decoded frame: the raw (still-obfuscated) payload bytes plus the bit
/// length that was detected from the sync pattern. Feed `bytes` to
/// [`crate::decode56`] for a 56-bit frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RxFrame {
    pub bytes: Vec<u8, 10>,
    pub bit_length: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    WaitingSync,
    ReceivingData,
}

/// Streaming RTS decoder. Feed measured pulses with [`RxDecoder::push`]; it
/// returns a [`RxFrame`] on the pulse that completes a frame.
pub struct RxDecoder {
    state: State,
    hw_syncs: u8,
    bit_length: u8,
    bits: u16,
    payload: [u8; 10],
    /// `false` = the next half-symbol is the first half of a Manchester
    /// symbol (advance only); `true` = it is the second half (its level is
    /// the bit).
    waiting_half: bool,
    /// The "start 0" low half-symbol that `render_pulses` emits between the
    /// software sync and the first data bit must be consumed without being
    /// paired; this flag skips exactly that one half-symbol.
    skip_start: bool,
}

/// Minimum hardware-sync half-pulses (edges) required before a software sync is
/// accepted. Matches the C++ `cpt_synchro_hw >= 4` guard (`Somfy.cpp:4404`):
/// a first frame emits 2 hardware syncs (4 half-pulses), a repeat 7 (14).
const MIN_HW_SYNCS: u8 = 4;

/// `±25%` tolerance window, the brief's simplification of the C++ RX
/// `TOLERANCE_MIN 0.7 / MAX 1.3` windows (`Somfy.cpp:4218-4234`). Kept because
/// it comfortably covers the ±10% jitter the loopback tests inject while still
/// separating the `HALF_SYMBOL` (640), `HW_SYNC_HALF` (2560) and
/// `SW_SYNC_HIGH` (4850) families.
fn within(actual: u32, expected: u32) -> bool {
    let lo = expected - expected / 4;
    let hi = expected + expected / 4;
    actual >= lo && actual <= hi
}

impl RxDecoder {
    pub fn new() -> Self {
        RxDecoder {
            state: State::WaitingSync,
            hw_syncs: 0,
            bit_length: 56,
            bits: 0,
            payload: [0; 10],
            waiting_half: false,
            skip_start: false,
        }
    }

    pub fn reset(&mut self) {
        *self = RxDecoder::new();
    }

    /// Append one Manchester bit (MSB-first within each byte).
    fn store_bit(&mut self, bit: u8) {
        let idx = (self.bits / 8) as usize;
        if idx < self.payload.len() {
            self.payload[idx] = (self.payload[idx] << 1) | (bit & 1);
        }
        self.bits += 1;
    }

    /// Emit a frame if the full `bit_length` has been collected, resetting the
    /// decoder for the next frame.
    fn complete(&mut self) -> Option<RxFrame> {
        if self.bits == self.bit_length as u16 {
            let n = (self.bit_length / 8) as usize;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&self.payload[..n]).ok()?;
            let f = RxFrame {
                bytes,
                bit_length: self.bit_length,
            };
            self.reset();
            return Some(f);
        }
        None
    }

    /// Detect frame bit length from the accumulated hardware-sync count, per the
    /// C++ switch at `Somfy.cpp:4414-4419`. Only 56-bit frames are exercised in
    /// Task 6; the 80-bit arms are carried over for fidelity and Task 7.
    fn detect_bit_length(&self) -> u8 {
        match self.hw_syncs {
            0..=7 => 56,
            12 | 13 => 80,
            14 => 56,
            n if n > 17 => 80,
            _ => 56,
        }
    }

    /// Feed one measured pulse. Returns a complete frame on the pulse that
    /// delivers the final data bit.
    pub fn push(&mut self, p: Pulse) -> Option<RxFrame> {
        match self.state {
            State::WaitingSync => {
                if within(p.micros, TIMINGS::HW_SYNC_HALF) {
                    // Every hardware-sync half-pulse (high or low) is counted,
                    // mirroring the C++ ISR which sees each edge segment.
                    self.hw_syncs = self.hw_syncs.saturating_add(1);
                } else if self.hw_syncs >= MIN_HW_SYNCS
                    && p.high
                    && within(p.micros, TIMINGS::SW_SYNC_HIGH)
                {
                    self.bit_length = self.detect_bit_length();
                    self.state = State::ReceivingData;
                    self.bits = 0;
                    self.payload = [0; 10];
                    self.waiting_half = false;
                    self.skip_start = true;
                } else {
                    // Anything else (noise, the wake-up pulse, its silence)
                    // breaks a partial sync run, matching the C++ reset.
                    self.hw_syncs = 0;
                }
                None
            }
            State::ReceivingData => {
                if within(p.micros, TIMINGS::HALF_SYMBOL) {
                    if self.skip_start {
                        // Consume the lone "start 0" half-symbol; leave the
                        // pairing phase (`waiting_half == false`) untouched so
                        // the next pulse begins the first real symbol.
                        self.skip_start = false;
                        return None;
                    }
                    if self.waiting_half {
                        // Second half of a symbol: its level is the bit value
                        // (Manchester: bit 1 = low then high).
                        self.store_bit(p.high as u8);
                        self.waiting_half = false;
                        return self.complete();
                    }
                    self.waiting_half = true;
                    None
                } else {
                    // Out-of-family duration (inter-frame gap or corruption):
                    // abandon this frame and re-acquire sync.
                    self.reset();
                    None
                }
            }
        }
    }
}

impl Default for RxDecoder {
    fn default() -> Self {
        Self::new()
    }
}
