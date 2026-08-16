//! Capturing a Somfy pulse train off the CC1101's data pin with the RMT
//! peripheral.
//!
//! The mirror image of [`rmt_tx`](super::rmt_tx), and it splits the same way —
//! more aggressively, in fact, because a receiver's mistakes are quieter than a
//! transmitter's. Walking a burst is `somfy_rmt::BurstCursor`, the frame
//! boundary the peripheral is configured with is `somfy_rmt::IDLE_THRESHOLD_US`
//! (chosen and pinned against real wall-remote captures), and the receive
//! budget is `somfy_rmt::MAX_SYMBOLS`. All three are pure data with host tests.
//!
//! What is left here is the part that can only exist against real hardware
//! types: mapping `esp_hal::rmt::PulseCode` back onto a `somfy_rmt::RmtSymbol`,
//! and driving one asynchronous RX transaction per burst. Deliberately no index
//! arithmetic — the first draft of this file kept its own cursor, and the bound
//! that stops it reading a previous burst was wrong, which nothing in a
//! four-chip build could have told anyone.
//!
//! ## What owns this
//!
//! The radio task, and nothing else. It is constructed once in `main` from the
//! `Rmt<Async>` receive channel and handed straight to `somfy_tasks::RadioLoop`
//! as its `PulseSource`. Deliberately no radio handle of its own: strobing the
//! CC1101 between receive and transmit belongs to [`super::air::Air`], which is
//! what keeps the half-duplex mode in one place.

use embassy_time::{Duration, Timer};
use esp_hal::{
    rmt::{Channel, Error as RmtError, PulseCode, Rx, RxChannelConfig, MAX_RX_IDLE_THRESHOLD},
    Async,
};
use heapless::Vec;
use somfy_rmt::{
    BurstCursor, PulseSource, RmtSymbol, IDLE_THRESHOLD_TICKS, MAX_IDLE_THRESHOLD_TICKS,
    MAX_SYMBOLS,
};
use somfy_rts::Pulse;

/// How long to wait before re-arming after a reception that delivered no
/// pulses at all.
///
/// A reception normally ends because the air went quiet for
/// `somfy_rmt::IDLE_THRESHOLD_US`, and delivers whatever preceded that silence.
/// Two things resolve *without* the receiver ever pending, though: a completed
/// reception carrying zero symbols, and a failure that is already latched when
/// the future is first polled. Retrying either immediately is right — a dropped
/// burst is not the end of the stream — but retrying it with no pause at all
/// turns a persistent one into a loop that resolves instantly forever and pins
/// the executor, which is the exact failure this type is `Async` to avoid,
/// reached from the other direction.
///
/// One millisecond is 1/22nd of the idle threshold, so it cannot cost a burst
/// that was going to arrive, and it caps an otherwise unbounded spin at a
/// thousand attempts a second.
const EMPTY_RECEPTION_BACKOFF_MS: u64 = 1;

/// RMT memory blocks reserved for the receive channel.
///
/// Same figure as the transmit side and for the same reason: one block holds 48
/// symbols on the chips with the smallest blocks, and a worst-case frame needs
/// 95. Two is the smallest allocation that holds a whole frame.
pub const MEMSIZE_BLOCKS: u8 = 2;

/// Symbols one reception can deliver.
///
/// Sized to exactly the RMT memory reserved above rather than to a figure of
/// our own, because on the ESP32 and ESP32-S2 that is a hard limit: those chips
/// cannot wrap a reception around the end of channel RAM, and esp-hal rejects a
/// buffer larger than the reservation outright (`Error::InvalidDataLength`).
/// Deriving the length from the reservation makes that unrepresentable instead
/// of merely avoided.
///
/// The ESP32-S3 and ESP32-C3 *can* wrap, so a larger buffer would be legal
/// there and would let one reception carry more than the reserved RAM. It is
/// deliberately not taken: a single Somfy frame fits comfortably, and one
/// buffer size across four chips is one fewer thing that behaves differently on
/// the board nobody has in front of them.
///
/// ## What a reception longer than this costs
///
/// Not a truncation — a **loss**, and the two chips differ. On the ESP32-S3 and
/// ESP32-C3 esp-hal's reader marks the transaction failed as soon as the buffer
/// fills, and `receive` resolves to `Error::ReceiverError` with the pulses
/// already copied out discarded; the whole burst is dropped. On the ESP32 and
/// ESP32-S2 reception simply stops when channel RAM is full and what fits is
/// returned.
///
/// Two situations reach it, neither of them a 56-bit reception as things stand:
///
/// - **80-bit transmissions**, whose repeat frames carry no inter-frame gap at
///   all, so a whole burst arrives as one reception several times this size. No
///   80-bit-capable remote or fixture exists yet, so nothing here is sized for
///   one, and this is the reason to say so out loud rather than discover it.
/// - **A real remote whose inter-frame gap is shorter than
///   `somfy_rmt::IDLE_THRESHOLD_US`**, which would merge its first frame and
///   its repeat into one reception: 100 symbols for a representative 56-bit
///   payload and 124 in the worst case, against the 96 this constant is on the
///   ESP32-S3 and ESP32-C3. The ESP32 and ESP32-S2 reserve 128 and would take
///   it. Nothing has measured that gap — the threshold's upper bound is
///   inferred from *our* transmitter — which is what makes capturing a real
///   repeat frame worth doing on air.
pub const RX_SYMBOLS: usize = MEMSIZE_BLOCKS as usize * esp_hal::rmt::CHANNEL_RAM_SIZE;

// A reception has to be able to hold a whole frame. If it could not, the
// longest frames would be lost outright — see the note above — and the loss
// would look like a radio that simply never hears certain commands, which is
// indistinguishable from a range or antenna problem.
//
// `MAX_SYMBOLS` is checked here rather than a figure restated in this file, and
// `somfy-rmt` pins it on the host from *both* directions' worst cases — 95
// symbols each, transmit and receive, arrived at by different arithmetic. Note
// what that leaves: on the chips where two blocks is exactly 96, this passes
// with **one symbol** to spare, so the guard establishes that a worst-case
// reception fits and nothing more. It is not a margin. Any change that could
// add a recorded entry — a wider frame, a glitch filter turned off differently,
// or the peripheral recording something beyond one entry per edge plus a
// terminator, which nobody here has measured — needs the budget re-derived, not
// this assertion nudged.
const _: () = assert!(
    MAX_SYMBOLS <= RX_SYMBOLS,
    "a reception buffer must hold a worst-case frame"
);

// `somfy-rmt` picks the idle threshold and asserts it against both ends of the
// window it has to sit in, but it cannot see the register the value is written
// to — and that register is *narrower on some chips than others*: 16 bits on
// the ESP32 and ESP32-S2, 15 on the ESP32-S3 and ESP32-C3. So the host crate
// states the narrowest field it believes exists, and this is where that belief
// meets esp-hal's own per-chip constant, on all four builds. A threshold past
// the field would be rejected at run time by `configure_rx`, which surfaces as
// a receiver that never starts rather than as a bad number.
const _: () = assert!(
    MAX_IDLE_THRESHOLD_TICKS <= MAX_RX_IDLE_THRESHOLD as u32,
    "somfy-rmt's idle-threshold ceiling must fit this chip's idle-threshold field"
);

/// The RX channel configuration this receiver requires.
///
/// `clk_divider` comes from [`crate::chip`], which already asserts that it
/// divides the RMT source clock down to the 1 µs tick the threshold below is
/// expressed in.
///
/// The glitch filter is left **off**, on the grounds that nothing here knows
/// what it would be filtering. The three committed wall-remote captures contain
/// no sub-448 µs entries at all — the only one in the fixture set was injected
/// by hand into the synthetic file to exercise the loader — so this project has
/// no measurement of short-glitch behaviour on its own signal chain, and the
/// filter's threshold is a `u8` whose unit is not stated consistently enough to
/// predict what a given value would drop. Enabling it would add an unobserved
/// hardware behaviour to filter a phenomenon nobody here has recorded, while
/// the decoder already rejects out-of-family durations. Revisit during on-air
/// bring-up, with a measurement rather than an inference.
pub fn rx_channel_config() -> RxChannelConfig {
    RxChannelConfig::default()
        .with_clk_divider(crate::chip::RMT_CLK_DIVIDER)
        // Narrowing is lossless by a two-step chain, neither step of which can
        // be edited away: `somfy-rmt` asserts the threshold under
        // `MAX_IDLE_THRESHOLD_TICKS`, and the assertion above asserts that
        // under this chip's field — which is itself a `u16`.
        .with_idle_threshold(IDLE_THRESHOLD_TICKS as u16)
        .with_filter_threshold(0)
        .with_carrier_modulation(false)
        .with_memsize(MEMSIZE_BLOCKS)
}

/// Map one received symbol off the peripheral's word.
///
/// The inverse of [`super::rmt_tx::to_pulse_code`], and a delegation for the
/// same reason: `PulseCode` is a packed `u32` whose bit layout esp-hal already
/// owns, and a second copy of it here would be a divergence waiting to happen.
pub fn from_pulse_code(code: PulseCode) -> RmtSymbol {
    RmtSymbol {
        level1: code.level1().into(),
        length1: code.length1(),
        level2: code.level2().into(),
        length2: code.length2(),
    }
}

/// A configured RMT receive channel, handing out one measured pulse at a time.
///
/// The channel must already be configured with [`rx_channel_config`] and
/// connected to the CC1101's GDO2 pin, and the radio must be in receive mode —
/// in asynchronous serial mode the chip drives that pin with demodulated data
/// only while it is receiving. Strobing the radio is the caller's job, as it is
/// on the transmit side.
///
/// ## Why the channel is `Async`
///
/// Not a style choice. esp-hal's blocking `RxTransaction::wait` busy-polls a
/// status register with no deadline and no yield. On the transmit side that
/// spin is bounded by a ~100 ms frame; here it is bounded by nothing at all,
/// because a shade may go untouched for hours. A blocking receive would hold
/// the executor for the whole of that silence, starving every other task on it
/// — and it would present as "the state task stopped ticking", not as a receive
/// bug, which is the kind of symptom that sends debugging in the wrong
/// direction for a long time.
///
/// ## One reception at a time, and what happens at the seam
///
/// The peripheral hands back a whole burst, but this yields pulses singly,
/// refilling from a fresh reception once a burst is spent. Nothing is
/// preserved across that seam and nothing needs to be: every burst opens with
/// the hardware-sync preamble, whose 2560 µs half-pulses match neither timing
/// family the decoder's data phase accepts and so reset it. Whatever state a
/// truncated burst left the decoder in is discarded by the next burst's first
/// few pulses.
///
/// The gap between one reception finishing and the next starting is real,
/// though, and pulses arriving inside it are lost. It is short — the next
/// `receive` is issued from the same task the moment the buffer runs dry — and
/// it lands in the silence that ended the previous burst, which by construction
/// is at least `somfy_rmt::IDLE_THRESHOLD_US` long. **Whether that is short
/// enough in practice is a question only on-air testing can answer.**
pub struct RmtPulseSource<'ch> {
    /// Not an `Option` like the transmit side's channel: `Channel<Async, Rx>`
    /// receives through `&mut self` and is never consumed, so there is no
    /// window in which it can be lost.
    channel: Channel<'ch, Async, Rx>,
    /// Where the peripheral writes. Only ever read by `receive_burst`, and only
    /// as far as the reception reported filling.
    buffer: [PulseCode; RX_SYMBOLS],
    /// The last burst, in this project's own symbol type and already cut to the
    /// length the reception delivered. Its length *is* the received prefix, so
    /// there is no separate count that could disagree with it, and nothing
    /// downstream can reach the stale RMT RAM beyond it.
    ///
    /// Costs a second copy of the burst — 768 bytes at the largest — bought
    /// deliberately: the alternative is index arithmetic in this file, which is
    /// the one crate in the workspace no host test can reach.
    burst: Vec<RmtSymbol, RX_SYMBOLS>,
    /// Position within `burst`. Lives in `somfy-rmt` so that the arithmetic
    /// that can silently hand back the wrong pulse is covered by host tests.
    cursor: BurstCursor,
}

impl<'ch> RmtPulseSource<'ch> {
    /// Takes ownership of a channel that has already been configured with
    /// [`rx_channel_config`] and connected to the CC1101's data-out pin.
    pub fn new(channel: Channel<'ch, Async, Rx>) -> Self {
        Self {
            channel,
            // Contents are irrelevant until a reception sets `filled`, which
            // bounds every read; initialised to the end marker rather than to
            // an arbitrary value only so that a buffer dumped during debugging
            // reads as empty instead of as a burst of nonsense.
            buffer: [PulseCode::end_marker(); RX_SYMBOLS],
            burst: Vec::new(),
            cursor: BurstCursor::new(),
        }
    }

    /// The next pulse of the burst already in hand, or `None` once it is spent.
    ///
    /// No arithmetic of its own, on purpose: `burst` is already exactly what the
    /// last reception delivered, and where a walk over it has to stop is
    /// `BurstCursor`'s business, which host tests cover. Stopping in the right
    /// place is not belt-and-braces for the end marker — on the chips that
    /// cannot wrap a reception, a burst that fills channel RAM stops with no
    /// marker at all, so a walk that trusted the marker would run into whatever
    /// the *previous* reception left behind and decode it as live signal.
    fn take_pulse(&mut self) -> Option<Pulse> {
        self.cursor.next(&self.burst)
    }

    /// Wait for the next burst and make it the current one.
    async fn receive_burst(&mut self) -> Result<(), RmtError> {
        // Emptied before the await, not after it: a cancelled or failed
        // reception must leave nothing behind that could be handed out as if it
        // had been measured.
        self.burst.clear();
        self.cursor.restart();

        let filled = self.channel.receive(&mut self.buffer).await?;

        // esp-hal already bounds what it writes by the slice it was given; the
        // clamp is what keeps a future change to that from overrunning `burst`.
        // With it, the slice is at most `RX_SYMBOLS` long — exactly `burst`'s
        // capacity — so no `push` here can fail.
        for code in &self.buffer[..filled.min(RX_SYMBOLS)] {
            if self.burst.push(from_pulse_code(*code)).is_err() {
                break;
            }
        }
        Ok(())
    }
}

impl PulseSource for RmtPulseSource<'_> {
    /// ## When this yields `None`
    ///
    /// Almost never, and deliberately so. A failed or empty capture is a
    /// dropped burst, not the end of the stream — the radio is still there and
    /// the next transmission is still coming — so this retries rather than
    /// reporting exhaustion, which would stop a caller's pump loop for good.
    ///
    /// The one exception is `InvalidDataLength`, and it is here for a specific
    /// reason. It means the buffer does not fit the channel's reserved RMT
    /// memory, which is a configuration mistake rather than a radio event: it
    /// is permanent, and esp-hal reports it *without ever pending*. Retrying it
    /// would spin the executor at full tilt forever — the exact failure this
    /// type is `Async` to avoid, arrived at from the other direction. A source
    /// that can never receive anything is a source whose hardware is gone as
    /// far as its caller is concerned, so it says so.
    ///
    /// [`rx_channel_config`] and [`RX_SYMBOLS`] between them make that
    /// configuration unreachable; this is what happens if some future call site
    /// configures the channel itself and gets it wrong.
    ///
    /// `esp_hal::rmt::Error` is `#[non_exhaustive]`, so treating
    /// `InvalidDataLength` as the only *permanent* failure is a property of
    /// today's esp-hal that nothing in this repository pins. Re-check this arm
    /// when bumping esp-hal.
    ///
    /// ## Every other unproductive reception backs off
    ///
    /// Reporting exhaustion is reserved for a configuration that can never
    /// work; anything else is retried, because a dropped burst is not the end
    /// of the stream. But a reception that delivers nothing and resolves
    /// without pending — a completed transaction carrying zero symbols, or a
    /// failure already latched when the future is first polled — would
    /// otherwise be retried at whatever rate the executor can manage. See
    /// [`EMPTY_RECEPTION_BACKOFF_MS`].
    async fn next_pulse(&mut self) -> Option<Pulse> {
        loop {
            if let Some(pulse) = self.take_pulse() {
                return Some(pulse);
            }
            if let Err(RmtError::InvalidDataLength) = self.receive_burst().await {
                return None;
            }
            // `receive_burst` empties `burst` before it awaits and only refills
            // it from a reception that succeeded, so this covers both the
            // zero-symbol case and every failure that is not permanent.
            if self.burst.is_empty() {
                Timer::after(Duration::from_millis(EMPTY_RECEPTION_BACKOFF_MS)).await;
            }
        }
    }
}
