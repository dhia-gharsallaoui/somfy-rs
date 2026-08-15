//! # somfy-rmt
//!
//! Packs Somfy OOK pulse trains into ESP32 RMT symbols.
//!
//! The RMT peripheral stores **two** (level, duration) pairs per 32-bit symbol,
//! each duration a 15-bit tick count. This crate performs that packing as pure
//! data so it is testable on the host: `somfy-rts` must stay free of hardware
//! types, and the `firmware` crate cannot be compiled for the host at all.
//! The firmware's only job is mapping [`RmtSymbol`] onto `esp_hal::rmt::PulseCode`.
//!
//! Ticks are 1 µs (80 MHz RMT source clock with `clk_divider = 80`).

#![cfg_attr(not(test), no_std)]

use heapless::Vec;
use somfy_rts::Pulse;

/// Tick period in microseconds. 80 MHz / 80 = 1 MHz.
pub const TICK_US: u32 = 1;

/// Maximum ticks in one RMT duration field (15 bits).
pub const MAX_TICKS: u32 = 32_767;

/// Upper bound on symbols for any single Somfy frame.
///
/// Worst case is an 80-bit first frame with a payload where no adjacent
/// Manchester halves merge: 2 wake-up + 24 hardware-sync + 2 software-sync +
/// 160 data = 188 pulses = 94 symbols. Rounded to 96 for headroom.
pub const MAX_SYMBOLS: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RmtSymbol {
    pub level1: bool,
    pub length1: u16,
    pub level2: bool,
    pub length2: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// A single pulse exceeds the 15-bit RMT duration field.
    TooLong { micros: u32 },
    /// The frame needs more symbols than [`MAX_SYMBOLS`].
    TooManySymbols { needed: usize },
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
/// for appending a zero-length `RmtSymbol` (or otherwise signalling
/// end-of-transmission) before handing the buffer to the RMT peripheral.
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

/// Compile-time guard: the longest Somfy pulse must fit the RMT length field.
/// If the timing model ever grows past this, the build fails here rather than
/// silently truncating a pulse on air.
const _: () = assert!(somfy_rts::TIMINGS::INTER_FRAME_GAP <= MAX_TICKS);
