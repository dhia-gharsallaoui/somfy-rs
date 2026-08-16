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
//! transmission can be replayed into receive code on the host. A received
//! buffer arrives in the same packed form the transmitter builds, so [`unpack`]
//! (and [`BurstCursor`], for a receiver handing out one pulse at a time) reads
//! it back out. [`IDLE_THRESHOLD_US`] is where the peripheral is told one
//! transmission ends.
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

/// Maximum ticks in an RMT **idle-threshold** field.
///
/// A different register from the duration field above, and narrower on some
/// chips than on others: 16 bits on the ESP32 and ESP32-S2, 15 on the ESP32-S3
/// and ESP32-C3. This is the narrowest of them, so a threshold that fits here
/// fits everywhere. The firmware asserts it against `esp-hal`'s own per-chip
/// constant rather than trusting this number on its own.
pub const MAX_IDLE_THRESHOLD_TICKS: u32 = 32_767;

/// How long the air must stay quiet before the receiver calls a transmission
/// finished, in microseconds.
///
/// This is the receive path's one genuinely free choice, and the only one that
/// cannot be checked against the transmitter: it has to sit above the longest
/// silence *inside* a real remote's frame and below the silence *between*
/// frames. Too low cuts every reception in two; too high glues consecutive
/// frames into a single reception that the buffer was not sized for.
///
/// **22 ms, chosen from measured captures rather than from [`TIMINGS`].** The
/// floor is [`somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US`] — 17738 µs, the
/// post-wake-up gap in the committed wall-remote captures, which is 2.4× the
/// `WAKEUP_LOW` this crate's own transmitter emits. Sizing against the
/// transmit constant instead would put the threshold *inside* every real first
/// frame. The ceiling is `TIMINGS::INTER_FRAME_GAP`, 27434 µs.
///
/// That leaves 4262 µs of margin below and 5434 µs above. The asymmetry is
/// deliberate in neither direction — it is what a round number in the middle of
/// the window gives — but note the two bounds are not equally trustworthy: the
/// floor is measured from real hardware, while the ceiling is *our* gap, and no
/// committed capture contains a real remote's repeat frame to confirm that a
/// remote's gap is the same. Capturing one is on the on-air bring-up list.
///
/// [`TIMINGS`]: somfy_rts::TIMINGS
pub const IDLE_THRESHOLD_US: u32 = 22_000;

/// [`IDLE_THRESHOLD_US`] as the tick count the hardware register takes.
pub const IDLE_THRESHOLD_TICKS: u32 = IDLE_THRESHOLD_US / TICK_US;

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
///
/// ## It bounds a *reception* too, and not by coincidence
///
/// A receiver needs the same budget arrived at differently, so this constant is
/// reused there rather than a second one invented. Receiving the same worst case
/// records one entry per edge — 188 — and the peripheral then terminates the
/// buffer with a zero-length entry of its own, giving 189 entries. At two
/// entries per symbol that is 95 symbols: the identical figure, because both
/// directions are "the worst-case edge count, plus one terminator, packed two to
/// a symbol". `tests/unpack.rs` pins it from the pulse trains rather than from
/// this arithmetic.
///
/// The spare symbol is therefore **one entry** of headroom on a reception, on
/// the chips where two blocks is exactly 96. That absorbs one unexpected
/// recorded entry and no more, and whether the peripheral records anything
/// beyond one-per-edge-plus-terminator — a leading idle segment, say — is not
/// something this repository has measured. Treat it as a bound that holds under
/// the documented behaviour, not as a margin.
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

/// The pulse a symbol buffer carries at `entry`, or `None` where the buffer
/// ends.
///
/// Each [`RmtSymbol`] packs **two** entries, one (level, duration) pair each,
/// so entry `n` is symbol `n / 2`'s first pair when `n` is even and its second
/// pair when `n` is odd. Durations come out of the buffer as tick counts and go
/// into a [`Pulse`] as microseconds; [`TICK_US`] converts.
///
/// `None` means the stream is over, covering both ways a received buffer can
/// end: the index has run past the slice, or the entry is zero-length — the
/// peripheral's end-of-transmission marker, written on the transmit side by
/// [`build_symbols`] and on the receive side by the hardware itself. A caller
/// must stop at its first `None` rather than probing past it: the marker is a
/// property of one position, and the entries beyond it are whatever an earlier
/// reception left in RMT RAM.
///
/// Note the terminator rule here is **per entry**, which is not the same
/// predicate as esp-hal's `PulseCode::is_end_marker` (true when *either* half of
/// a code is zero). The per-entry rule is the correct one for reading: a code
/// holding one real pulse and one zero still has a pulse to hand over, and
/// treating the whole code as a marker would silently drop the last pulse of
/// every odd-length burst. The two are not interchangeable.
pub fn pulse_at(symbols: &[RmtSymbol], entry: usize) -> Option<Pulse> {
    let symbol = symbols.get(entry / 2)?;
    let (high, length) = if entry.is_multiple_of(2) {
        (symbol.level1, symbol.length1)
    } else {
        (symbol.level2, symbol.length2)
    };
    if length == 0 {
        return None;
    }
    Some(Pulse {
        high,
        micros: length as u32 * TICK_US,
    })
}

/// Walk every pulse a symbol buffer carries, in order — the receive-side
/// inverse of [`pack`].
///
/// For callers that hold a whole buffer. A receiver handing out one pulse at a
/// time keeps its own entry index and calls [`pulse_at`] instead, because an
/// iterator borrowing the buffer cannot live in the same struct as the buffer.
pub fn unpack(symbols: &[RmtSymbol]) -> Unpack<'_> {
    Unpack { symbols, entry: 0 }
}

/// Iterator over a symbol buffer's pulses. See [`unpack`].
///
/// Fused: it stops permanently at the first end marker, and never resumes into
/// whatever follows.
pub struct Unpack<'a> {
    symbols: &'a [RmtSymbol],
    entry: usize,
}

impl Iterator for Unpack<'_> {
    type Item = Pulse;

    fn next(&mut self) -> Option<Pulse> {
        // The index is only advanced on a hit, so an exhausted iterator keeps
        // re-reading the entry that stopped it and keeps returning `None`.
        let pulse = pulse_at(self.symbols, self.entry)?;
        self.entry += 1;
        Some(pulse)
    }
}

impl core::iter::FusedIterator for Unpack<'_> {}

/// A cursor over one received burst, handing out its pulses one at a time.
///
/// What [`Unpack`] is for a caller holding a whole buffer, this is for a
/// receiver that must yield a single pulse per call and cannot store an iterator
/// beside the buffer it borrows. It holds only a position, so it lives happily
/// in the same struct as the burst it walks.
///
/// ## The contract, which is the whole point
///
/// `symbols` must be **exactly the prefix the last reception filled**, not the
/// whole buffer it was received into. Everything past that prefix is whatever an
/// earlier burst left in RMT RAM, and a receiver that read into it would decode
/// old traffic as live signal. Bounding by the slice is not belt-and-braces for
/// the end marker: on the chips that cannot wrap a reception, a burst that fills
/// channel RAM stops with **no marker at all**, so the slice length is the only
/// thing that stops the walk.
///
/// Call [`BurstCursor::restart`] whenever a new reception replaces the buffer's
/// contents — including one that failed or was cancelled, where the right
/// position is the start of nothing rather than the middle of something stale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BurstCursor {
    /// Entries already handed out. Two entries per symbol.
    entry: usize,
}

impl BurstCursor {
    /// A cursor positioned before the first entry of a burst.
    pub const fn new() -> Self {
        Self { entry: 0 }
    }

    /// Return to the first entry, abandoning whatever is left of the current
    /// burst.
    pub fn restart(&mut self) {
        self.entry = 0;
    }

    /// The next pulse of `symbols`, or `None` once this burst is spent.
    ///
    /// `None` is sticky until [`BurstCursor::restart`]: the position only
    /// advances on a hit, so an exhausted cursor keeps re-reading whatever
    /// stopped it. A caller that pumps until `None` and then asks again cannot
    /// be handed a pulse it has already seen.
    #[allow(clippy::should_implement_trait)] // not `Iterator`: the burst is an argument, not state
    pub fn next(&mut self, symbols: &[RmtSymbol]) -> Option<Pulse> {
        let pulse = pulse_at(symbols, self.entry)?;
        self.entry += 1;
        Some(pulse)
    }
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

// The idle threshold has to stay inside a window whose ends are facts about
// hardware rather than preferences, and a drift outside it fails in a way that
// points nowhere: below the floor, every reception is cut at the wake-up gap of
// a frame that is otherwise perfectly good; above the ceiling, two frames share
// one reception and overrun a buffer sized for one. Neither reports itself as a
// threshold problem. Tie the value to both bounds so that edit cannot compile.
//
// The floor is deliberately the *measured* constant and not `WAKEUP_LOW`: a
// real remote's post-wake-up silence is over twice the value this crate's own
// transmitter emits, so an assertion written against `TIMINGS` would pass while
// asserting something the committed captures contradict.
// Both ends carry a margin rather than a bare inequality, because neither bound
// is exact: the floor is three captures of one remote, and the ceiling is our
// own gap standing in for a real remote's, which nothing has measured. A
// threshold that merely satisfied the inequality would be sitting on the
// accuracy of numbers that do not have that much accuracy to give.
const IDLE_THRESHOLD_MARGIN_US: u32 = 4_000;

const _: () = assert!(
    IDLE_THRESHOLD_US >= somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US + IDLE_THRESHOLD_MARGIN_US,
    "the idle threshold must clear the longest silence measured inside a real frame, with margin"
);
const _: () = assert!(
    IDLE_THRESHOLD_US + IDLE_THRESHOLD_MARGIN_US <= somfy_rts::TIMINGS::INTER_FRAME_GAP,
    "the idle threshold must stay below the silence that separates frames, with margin"
);
const _: () = assert!(
    IDLE_THRESHOLD_TICKS <= MAX_IDLE_THRESHOLD_TICKS,
    "the idle threshold must fit the narrowest chip's idle-threshold field"
);
