//! Clocking a Somfy pulse train out of the RMT peripheral and onto the
//! CC1101's data pin.
//!
//! The interesting work — rendering a frame to pulses, merging them
//! edge-to-edge, packing two per symbol and terminating the buffer — is
//! `somfy_rmt::build_symbols`, which is pure data and covered by host tests.
//! What is left here is the part that can only exist against real hardware
//! types: mapping a packed symbol onto `esp_hal::rmt::PulseCode`, and driving
//! one blocking TX transaction per frame.

use esp_hal::{
    gpio::Level,
    rmt::{Channel, Error as RmtError, PulseCode, Tx, TxChannelConfig},
    Blocking,
};
use heapless::Vec;
use somfy_rmt::{build_symbols, PackError, RmtSymbol, MAX_SYMBOLS};
use somfy_rts::FrameKind;

/// RMT memory blocks reserved for the transmit channel.
///
/// One block is not enough: it holds 48 symbols on the chips with the smallest
/// blocks, and a worst-case 80-bit first frame needs 95. Two blocks is the
/// smallest allocation that holds a whole frame, and costs nothing we use —
/// only one TX channel is ever configured.
pub const MEMSIZE_BLOCKS: u8 = 2;

/// Settle time to allow between strobing the radio into TX and clocking the
/// first symbol at it.
///
/// `somfy-cc1101` sets `MCSM0.FS_AUTOCAL`, so `STX` from IDLE calibrates the
/// synthesiser *before* enabling the transmitter. The strobe returns
/// immediately; the chip keys no carrier until that completes. Start the RMT
/// inside that window and the leading edge of the wake-up pulse is simply not
/// transmitted — the frame goes out shortened, with nothing reporting it.
///
/// **A margin choice, not a measurement**, in the same spirit as the driver's
/// post-reset settle: comfortably beyond the calibration and PLL-settling
/// figures the datasheet quotes, and paid once per transmit burst rather than
/// per frame. Not yet checked against a scope.
pub const TX_SETTLE_US: u32 = 1_000;

// `somfy-rmt` rejects any pulse past its own tick ceiling, and this is the
// hardware field that ceiling is supposed to describe. The two are equal today,
// but only because two files independently say 32767 — nothing links them. A
// divergence would neither fail to build nor fail to link; frames would simply
// start going out with truncated pulses and shades would stop responding, with
// no error anywhere pointing at the cause. Tie them together so that edit
// cannot compile.
const _: () = assert!(
    somfy_rmt::MAX_TICKS == PulseCode::MAX_LEN as u32,
    "somfy-rmt's tick ceiling must match the RMT hardware's length field"
);

// The frame budget must fit the memory actually reserved, taken from esp-hal's
// own per-chip constant rather than from a figure copied out of a design
// document. Overflowing it is not fatal — esp-hal refills the channel from the
// remaining data on a threshold interrupt — but a frame that fits entirely in
// RMT RAM never depends on that refill keeping up with a real-time OOK stream.
const _: () = assert!(
    MAX_SYMBOLS <= MEMSIZE_BLOCKS as usize * esp_hal::rmt::CHANNEL_RAM_SIZE,
    "a worst-case frame must fit the reserved RMT memory blocks"
);

/// Everything that can stop a frame reaching the air.
///
/// No `Eq`: `esp_hal::rmt::Error` only implements `PartialEq`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TxError {
    /// The frame could not be turned into a symbol buffer.
    Pack(PackError),
    /// A packed duration did not fit the hardware's 15-bit length field.
    /// `somfy-rmt` already rejects those, so this cannot happen — it is an
    /// error rather than a panic because a panic here would take the whole
    /// controller off the air.
    LengthOutOfRange,
    /// The RMT peripheral refused the buffer or reported a failed
    /// transmission.
    Rmt(RmtError),
    /// The channel is missing. Only reachable if a previous transmit was
    /// interrupted between taking the channel and giving it back.
    ChannelUnavailable,
}

impl From<PackError> for TxError {
    fn from(error: PackError) -> Self {
        Self::Pack(error)
    }
}

/// The TX channel configuration this transmitter requires.
///
/// `clk_divider` comes from [`crate::chip`], which already asserts that it
/// divides the RMT source clock down to the 1 µs tick `somfy-rmt` packs
/// against. The idle level is driven low so the CC1101's carrier is off
/// between frames rather than left at whatever the last symbol ended on.
pub fn tx_channel_config() -> TxChannelConfig {
    TxChannelConfig::default()
        .with_clk_divider(crate::chip::RMT_CLK_DIVIDER)
        .with_idle_output(true)
        .with_idle_output_level(Level::Low)
        .with_carrier_modulation(false)
        .with_memsize(MEMSIZE_BLOCKS)
}

/// Map one packed symbol onto the peripheral's word.
///
/// `PulseCode` is a packed `u32` and esp-hal already owns that bit layout; a
/// second copy of it here would be a divergence waiting to happen, so this is a
/// delegation and nothing more. It uses the fallible constructor: the
/// infallible one panics on an out-of-range length.
pub fn to_pulse_code(symbol: RmtSymbol) -> Option<PulseCode> {
    PulseCode::try_new(
        Level::from(symbol.level1),
        symbol.length1,
        Level::from(symbol.level2),
        symbol.length2,
    )
}

/// A configured RMT transmit channel, ready to clock out frames.
///
/// The radio must already be transmitting when [`RmtTx::transmit_frame`] is
/// called: this drives the CC1101's data pin, and in asynchronous serial mode
/// the chip only keys a carrier to follow that pin while it is in TX. Strobing
/// the radio into and out of TX is the caller's job — this type owns the
/// timing, not the radio — and the caller must also wait [`TX_SETTLE_US`] after
/// that strobe before the first call here.
///
/// ## Known limitation: no transmit timeout
///
/// [`RmtTx::transmit_frame`] blocks in esp-hal's `TxTransaction::wait`, which
/// busy-loops with no deadline. A channel that never raised its end or error
/// interrupt would hang the caller — as bad an outcome as a panic.
///
/// This is not fixable at this esp-hal version and is deliberately not
/// papered over. `TxTransaction` *owns* the channel and hands it back only
/// from `wait`; `poll` returns a bare `bool`, and there is no abort that
/// returns the channel. So a poll-with-deadline loop that gave up would have
/// to drop the transaction, taking the channel with it and leaving the
/// transmitter permanently dead — trading a recoverable hang for an
/// unrecoverable one. Revisit if esp-hal grows a channel-returning abort.
pub struct RmtTx<'ch> {
    /// `Option` because esp-hal's blocking transmit consumes the channel by
    /// value and hands it back on completion. See [`RmtTx::transmit_frame`].
    channel: Option<Channel<'ch, Blocking, Tx>>,
}

impl<'ch> RmtTx<'ch> {
    /// Takes ownership of a channel that has already been configured with
    /// [`tx_channel_config`] and connected to the CC1101's data pin.
    pub fn new(channel: Channel<'ch, Blocking, Tx>) -> Self {
        Self {
            channel: Some(channel),
        }
    }

    /// Transmit one encoded frame and block until the peripheral is done.
    ///
    /// Repeats are separate calls, not one long buffer. The two frame widths
    /// differ in why: a 56-bit frame carries its inter-frame gap at the end of
    /// its own pulse train, while an 80-bit frame suppresses that gap and
    /// re-encodes part of its payload for each repeat, so each repeat is a
    /// different frame rather than the same one sent again.
    ///
    /// Needs roughly **6.5 KB of stack** — most of it inside `build_symbols`,
    /// which renders and merges into two fixed 320-pulse buffers. That is
    /// comfortable on the main stack but is more than a default async task
    /// gets, so whichever task ends up owning the radio has to be sized for it
    /// deliberately.
    pub fn transmit_frame(&mut self, bytes: &[u8], kind: FrameKind) -> Result<(), TxError> {
        let mut symbols: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
        build_symbols(bytes, kind, &mut symbols)?;

        let mut codes = [PulseCode::default(); MAX_SYMBOLS];
        for (code, symbol) in codes.iter_mut().zip(symbols.iter()) {
            *code = to_pulse_code(*symbol).ok_or(TxError::LengthOutOfRange)?;
        }

        let channel = self.channel.take().ok_or(TxError::ChannelUnavailable)?;
        let (channel, result) = send(channel, &codes[..symbols.len()]);
        // One restore, unconditional, and nothing between it and the `take`
        // above can return early — `send` returns the channel in its type, so
        // there is no path on which it is dropped. A failed transmit that ate
        // the channel would leave the controller permanently unable to send.
        self.channel = Some(channel);
        result
    }
}

/// Run one transmit transaction, returning the channel whatever happens.
///
/// esp-hal's `transmit` and `wait` both consume the channel and both hand it
/// back in their error arms. Returning it alongside the outcome — rather than
/// assigning it in each arm — makes losing it a type error instead of a
/// reviewing exercise. Note that neither call can use `?`: their error type is
/// a tuple carrying the channel, so the compiler rejects any attempt to
/// propagate without unpacking it.
fn send<'ch>(
    channel: Channel<'ch, Blocking, Tx>,
    codes: &[PulseCode],
) -> (Channel<'ch, Blocking, Tx>, Result<(), TxError>) {
    let transaction = match channel.transmit(codes) {
        Ok(transaction) => transaction,
        Err((error, channel)) => return (channel, Err(TxError::Rmt(error))),
    };

    match transaction.wait() {
        Ok(channel) => (channel, Ok(())),
        Err((error, channel)) => (channel, Err(TxError::Rmt(error))),
    }
}
