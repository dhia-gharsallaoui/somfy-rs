//! # somfy-rmt
//!
//! The RMT peripheral's half of the Somfy radio, as pure data.
//!
//! Neither neighbour can hold this code: `somfy-rts` must stay free of hardware
//! concepts, and the `firmware` crate cannot be compiled for the host at all.
//! So the shapes the RMT peripheral imposes live here, where a host compiler
//! and a host test suite can still reach them, and the firmware is left with
//! only the calls that need a chip.
//!
//! **Transmit.** The peripheral stores **two** (level, duration) pairs per
//! 32-bit symbol, each duration a 15-bit tick count. [`build_symbols`] owns
//! that whole pipeline — render an encoded frame to pulses, merge them
//! edge-to-edge, pack two per symbol, and terminate the buffer — leaving the
//! firmware to map each [`RmtSymbol`] onto `esp_hal::rmt::PulseCode`.
//!
//! **Receive.** [`PulseSource`] is the seam the receive path is written
//! against: a stream of merged edge-to-edge [`somfy_rts::Pulse`]s, which is
//! both what the peripheral hands back and what `somfy_rts::RxDecoder` already
//! consumes. [`ReplayPulseSource`] implements it over a slice, so a captured
//! transmission can be replayed into receive code on the host.
//!
//! Ticks are 1 µs (80 MHz RMT source clock with `clk_divider = 80`).

#![cfg_attr(not(test), no_std)]

mod source;

use heapless::Vec;
use somfy_rts::{merge_pulses, render_pulses, FrameKind, Pulse};

pub use source::{PulseSource, ReplayPulseSource};

/// Tick period in microseconds. 80 MHz / 80 = 1 MHz.
pub const TICK_US: u32 = 1;

/// Maximum ticks in one RMT duration field (15 bits).
pub const MAX_TICKS: u32 = 32_767;

/// Encoded length of a 56-bit frame.
pub const FRAME56_BYTES: usize = 7;

/// Encoded length of an 80-bit frame.
pub const FRAME80_BYTES: usize = 10;

/// Upper bound on symbols for any single Somfy frame.
///
/// Worst case is an 80-bit first frame with a payload where no adjacent
/// Manchester halves merge — all-zero bytes: 2 wake-up + 24 hardware-sync +
/// 2 software-sync + 160 data = 188 pulses = 94 symbols, every half of every
/// symbol full. [`build_symbols`] then appends a **95th** symbol as the
/// end-of-transmission marker, since nothing in a full buffer says "stop".
///
/// 96 leaves one spare symbol, and is also exactly two RMT memory blocks on the
/// chips with the smallest blocks (48 symbols each). The firmware asserts that
/// relationship against `esp-hal`'s own block-size constant rather than
/// restating it here.
pub const MAX_SYMBOLS: usize = 96;

/// Scratch capacity for pulse rendering, fixed by `somfy_rts::render_pulses`.
const PULSE_CAPACITY: usize = 320;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RmtSymbol {
    pub level1: bool,
    pub length1: u16,
    pub level2: bool,
    pub length2: u16,
}

impl RmtSymbol {
    /// A wholly zero-length symbol. The RMT peripheral stops transmitting when
    /// it reaches a zero-length entry, so this is how a symbol buffer says
    /// "end of transmission" rather than running on into whatever follows it
    /// in RMT RAM.
    pub const END_MARKER: Self = Self {
        level1: false,
        length1: 0,
        level2: false,
        length2: 0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// A single pulse exceeds the 15-bit RMT duration field.
    TooLong { micros: u32 },
    /// The frame needs more symbols than [`MAX_SYMBOLS`].
    TooManySymbols { needed: usize },
    /// The byte slice is neither [`FRAME56_BYTES`] nor [`FRAME80_BYTES`] long,
    /// so it is not an encoded Somfy frame.
    UnsupportedFrameLength { bytes: usize },
}

/// Pack merged, edge-to-edge pulses into RMT symbols.
///
/// Input MUST already be merged (see `somfy_rts::merge_pulses`); packing
/// unmerged half-symbols wastes half the symbol budget and can overflow.
///
/// An odd pulse count leaves the trailing half zero-length, which is RMT's
/// end-of-transmission marker — the desired behaviour, not padding. An
/// **even** pulse count fills every half of every symbol with real data, so
/// `pack` emits no terminator at all in that case: the caller is responsible
/// for appending [`RmtSymbol::END_MARKER`] (or otherwise signalling
/// end-of-transmission) before handing the buffer to the RMT peripheral.
///
/// [`build_symbols`] does that, and is what a transmitter should call. This is
/// the packing step on its own, for callers holding an already-merged train.
pub fn pack(merged: &[Pulse], out: &mut Vec<RmtSymbol, MAX_SYMBOLS>) -> Result<(), PackError> {
    out.clear();
    let needed = merged.len().div_ceil(2);
    if needed > MAX_SYMBOLS {
        return Err(PackError::TooManySymbols { needed });
    }
    for p in merged {
        if p.micros > MAX_TICKS {
            return Err(PackError::TooLong { micros: p.micros });
        }
    }
    for chunk in merged.chunks(2) {
        let first = chunk[0];
        let second = chunk.get(1).copied();
        out.push(RmtSymbol {
            level1: first.high,
            length1: (first.micros / TICK_US) as u16,
            level2: second.map(|p| p.high).unwrap_or(false),
            length2: second.map(|p| (p.micros / TICK_US) as u16).unwrap_or(0),
        })
        .map_err(|_| PackError::TooManySymbols { needed })?;
    }
    Ok(())
}

/// Turn one encoded frame into the exact symbol buffer the RMT peripheral
/// transmits: render the OOK pulse train, merge it edge-to-edge, pack two
/// pulses per symbol, and make sure the result ends in a marker.
///
/// `bytes` must be an encoded frame — [`FRAME56_BYTES`] or [`FRAME80_BYTES`]
/// long. Any other length is rejected rather than rendered: the renderer treats
/// "not ten bytes" as a 56-bit frame, so a mis-sized slice would go out as a
/// well-formed transmission of the wrong bit count — wrong on air with nothing
/// anywhere reporting it — and a long enough slice would overflow the renderer's
/// fixed pulse buffer.
///
/// The trailing marker is a zero-length entry, which is how the peripheral is
/// told to stop. Packing an odd number of pulses already leaves one in the last
/// symbol's unused second half; an even number fills every half, so a wholly
/// zero symbol is appended. Omitting it there would let the peripheral run on
/// into whatever else is in RMT RAM.
///
/// Uses roughly 5 KB of stack for the two rendering buffers.
pub fn build_symbols(
    bytes: &[u8],
    kind: FrameKind,
    out: &mut Vec<RmtSymbol, MAX_SYMBOLS>,
) -> Result<(), PackError> {
    out.clear();
    if bytes.len() != FRAME56_BYTES && bytes.len() != FRAME80_BYTES {
        return Err(PackError::UnsupportedFrameLength { bytes: bytes.len() });
    }

    let mut rendered: Vec<Pulse, PULSE_CAPACITY> = Vec::new();
    render_pulses(bytes, kind, &mut rendered);

    let mut merged: Vec<Pulse, PULSE_CAPACITY> = Vec::new();
    merge_pulses(&rendered, &mut merged);

    pack(&merged, out)?;

    if merged.len().is_multiple_of(2) && out.push(RmtSymbol::END_MARKER).is_err() {
        // Unreachable for the two accepted frame widths — the worst of them
        // needs 95 of the 96 slots. Reported rather than unwrapped anyway: a
        // panic here would take a shade controller off the air entirely.
        out.clear();
        return Err(PackError::TooManySymbols {
            needed: merged.len() / 2 + 1,
        });
    }
    Ok(())
}

// Compile-time guard: the longest pulse that can reach a 15-bit length field
// must fit in one. If the timing model ever grows past this, the build fails
// here rather than a frame failing on air.
//
// The quantity to guard is the longest **merged** pulse, not the longest
// rendered one. A `0` bit ends on a LOW half-symbol, and the inter-frame gap
// that follows it is also LOW, so the two merge into a single entry — 640 µs
// longer than the gap constant on its own. Guarding the bare gap would leave
// the check 640 µs looser than the property it claims to establish.
const _: () =
    assert!(somfy_rts::TIMINGS::INTER_FRAME_GAP + somfy_rts::TIMINGS::HALF_SYMBOL <= MAX_TICKS);

// `TICK_US` divides every duration below. The firmware separately asserts that
// its RMT clock and divider resolve to this value, but this crate builds and
// ships on its own, so it guards its own divisor.
const _: () = assert!(TICK_US > 0);
