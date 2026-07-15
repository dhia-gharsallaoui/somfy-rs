//! RTS receive-side decoder: a state machine that turns a stream of measured
//! OOK [`Pulse`]s back into the raw frame bytes.
//!
//! Ported from the C++ `somfy_rx_t` state machine (`ESPSomfy-RTS/src/Somfy.h`
//! lines 89-116 and its `Transceiver::handleReceive` driver in
//! `src/Somfy.cpp:4384-4516`). The sync-acquisition half (hardware-sync
//! counting, `>= 4` before accepting the software sync, and the `bit_length`
//! selection switch) mirrors the C++ verbatim.
//!
//! The data phase is the C++ transition algorithm (`Somfy.cpp:4440-4458`) made
//! level-aware, so ONE decoder accepts both pulse representations:
//!
//! 1. **Merged edge-to-edge streams** — what real hardware produces. The C++
//!    ISR runs off a `CHANGE` interrupt, so adjacent same-level half-symbols
//!    arrive pre-merged as single ~`2 * SYMBOL` (~1280µs) segments. The
//!    firmware's `rx.pulses[]` captures have this form.
//! 2. **Unmerged synthetic streams** — what this crate's [`render_pulses`]
//!    emits (it deliberately keeps every half-symbol separate; see its doc).
//!
//! How: the C++ stores each Manchester bit upon consuming that bit's *first*
//! half-symbol. A full-symbol (~1280µs) duration spans "second half of bit n +
//! first half of bit n+1" and the C++ stores `previous_bit ^ 1`; the second
//! ~640µs pulse of a half-pair is bit n+1's first half and the C++ stores
//! `previous_bit` unchanged. That duration-only rule is sound in the C++ only
//! because an edge-derived stream can never contain two consecutive same-level
//! segments. Our [`Pulse`] carries the level, which permits the strictly more
//! general rule used here: at every first-half event, `bit = !level` (a bit's
//! first half carries the inverted bit; polarity per [`render_pulses`]: bit 1
//! = low half then high half, MSB-first). On merged streams `!level`
//! reproduces the C++ toggle exactly — a merged segment's level is bit n,
//! which equals `!bit n+1`. On unmerged streams it reads the bit directly,
//! where the duration-only toggle would see no full-symbol segments at all.
//!
//! Storing at the first half also means the final bit completes the frame
//! before its second half arrives, so a last-bit-0 low half that merges into
//! the inter-frame silence (one long out-of-family low segment) can never cost
//! a data bit; whatever trails the frame lands harmlessly in `WaitingSync`.

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
    /// Same meaning as the C++ `waiting_half_symbol`: `true` after a lone
    /// half-symbol pulse has been consumed, i.e. the *next* half-symbol event
    /// is a bit's first half (the storage point). Entering the data phase
    /// with `false` lets the TX "start 0" low half flow through as the
    /// opening half of the pairing, exactly as in the C++ (`Somfy.cpp:4411`).
    waiting_half: bool,
}

/// Minimum hardware-sync half-pulses (edges) required before a software sync is
/// accepted. Matches the C++ `cpt_synchro_hw >= 4` guard (`Somfy.cpp:4404`):
/// a first frame emits 2 hardware syncs (4 half-pulses), a repeat 7 (14).
const MIN_HW_SYNCS: u8 = 4;

/// `±25%` tolerance window, a simplification of the C++ RX
/// `TOLERANCE_MIN 0.7 / MAX 1.3` windows (`Somfy.cpp:4218-4234`). It
/// comfortably covers ±10% real-world jitter while keeping the `HALF_SYMBOL`
/// (640), full-symbol (1280), `HW_SYNC_HALF` (2560) and `SW_SYNC_HIGH` (4850)
/// families separated.
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
    /// C++ switch at `Somfy.cpp:4414-4419`. Both lengths are exercised: a 56-bit
    /// repeat yields 14 hw-sync halves, an 80-bit repeat 12 (see
    /// [`crate::render_pulses`]), so the `12 | 13 => 80` arm is what makes an
    /// 80-bit transmission decode with `bit_length == 80`.
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
    /// delivers the final data bit (its first half — see module docs).
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
                } else {
                    // Anything else (noise, the wake-up pulse, its silence)
                    // breaks a partial sync run, matching the C++ reset.
                    self.hw_syncs = 0;
                }
                None
            }
            State::ReceivingData => {
                // C++ `Somfy.cpp:4443-4458`, level-aware (see module docs).
                if !self.waiting_half && within(p.micros, 2 * TIMINGS::HALF_SYMBOL) {
                    // Merged segment: second half of the previous bit plus
                    // the first half of the next bit; its level is the
                    // inverted new bit (equivalent to the C++
                    // `previous_bit ^ 1` toggle).
                    self.store_bit(!p.high as u8);
                    self.complete()
                } else if within(p.micros, TIMINGS::HALF_SYMBOL) {
                    if self.waiting_half {
                        // Second pulse of a half-pair == a bit's first half:
                        // its level is the inverted bit.
                        self.store_bit(!p.high as u8);
                        self.waiting_half = false;
                        self.complete()
                    } else {
                        self.waiting_half = true;
                        None
                    }
                } else {
                    // Out-of-family duration (inter-frame gap, a full symbol
                    // arriving mid-pair, or corruption): abandon this frame
                    // and re-acquire sync, as the C++ does.
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
