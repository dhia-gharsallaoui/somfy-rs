//! Keying the CC1101 around a burst, and clocking the frames out in between.
//!
//! [`somfy_tasks::Transmitter`] is three operations because those three are all
//! that need a chip; the *order* they happen in — key on, first frame, repeats,
//! park whatever happened — is `somfy-tasks`' business and is host-tested
//! there. This file is the implementation and nothing else.
//!
//! ## Who owns the radio's mode
//!
//! This type does, and it is the only thing that may. The CC1101 is
//! half-duplex: in asynchronous serial mode it keys a carrier to follow GDO0
//! while transmitting and drives demodulated data onto GDO2 while receiving,
//! and it does exactly one of those at a time. The receiver
//! ([`super::rmt_rx::RmtPulseSource`]) deliberately holds no radio handle, so
//! there is nowhere else a mode change could come from — which is what makes
//! "the radio is receiving unless a burst is in flight" a property of the code
//! rather than of a convention.

use embedded_hal::spi::SpiDevice;
use esp_hal::delay::Delay;
use somfy_cc1101::{Cc1101, Cc1101Error};
use somfy_rts::FrameKind;
use somfy_tasks::Transmitter;

use super::rmt_tx::{RmtTx, TxError, TX_SETTLE_US};

/// Why a burst did not reach the air.
///
/// No `Eq`: [`TxError`] carries `esp_hal::rmt::Error`, which only implements
/// `PartialEq`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AirError {
    /// The radio refused a mode strobe over SPI.
    Radio(Cc1101Error),
    /// The RMT peripheral refused the frame.
    Tx(TxError),
}

/// The CC1101 and the RMT transmit channel, as one thing that puts frames on
/// the air.
pub struct Air<'ch, SPI> {
    radio: Cc1101<SPI>,
    tx: RmtTx<'ch>,
    delay: Delay,
}

impl<'ch, SPI: SpiDevice> Air<'ch, SPI> {
    /// Takes ownership of an initialised radio and a configured transmit
    /// channel.
    ///
    /// The radio is left in whatever mode `init` left it (IDLE); call
    /// [`Air::listen`] before running the radio loop, or the first thing the
    /// receiver does is read a pin nothing is driving.
    pub fn new(radio: Cc1101<SPI>, tx: RmtTx<'ch>) -> Self {
        Self {
            radio,
            tx,
            delay: Delay::new(),
        }
    }

    /// Put the radio into receive and wait for it to be listening.
    ///
    /// Called once before the loop starts; after that [`Transmitter::key_off`]
    /// does it at the end of every burst.
    pub fn listen(&mut self) -> Result<(), AirError> {
        self.radio.set_rx().map_err(AirError::Radio)?;
        // Same settle as the transmit side, for the same reason: `MCSM0`
        // calibrates the synthesiser on the way out of IDLE and the strobe
        // returns before that finishes. Without the wait the receiver's first
        // reception is whatever the demodulator produces mid-calibration.
        self.delay.delay_micros(TX_SETTLE_US);
        Ok(())
    }
}

impl<SPI: SpiDevice> Transmitter for Air<'_, SPI> {
    type Error = AirError;

    /// IDLE first, then TX.
    ///
    /// Going through IDLE rather than strobing `STX` straight out of RX is
    /// deliberate: `MCSM0.FS_AUTOCAL` is configured to calibrate on the
    /// IDLE→TX transition specifically, so this is the path the 2026-08-15
    /// on-air bring-up actually exercised. A direct RX→TX strobe would skip the
    /// calibration the settle below is sized for.
    fn key_on(&mut self) -> Result<(), AirError> {
        self.radio.set_idle().map_err(AirError::Radio)?;
        self.radio.set_tx().map_err(AirError::Radio)?;
        self.delay.delay_micros(TX_SETTLE_US);
        Ok(())
    }

    async fn send_frame(&mut self, bytes: &[u8], kind: FrameKind) -> Result<(), AirError> {
        self.tx
            .transmit_frame(bytes, kind)
            .await
            .map_err(AirError::Tx)
    }

    /// Park the transmitter and go back to listening.
    ///
    /// Not merely IDLE: a controller that stopped receiving after its first
    /// transmission would lose every overheard wall-remote press from then on,
    /// and the position estimate would drift with nothing reporting why.
    fn key_off(&mut self) -> Result<(), AirError> {
        self.radio.set_idle().map_err(AirError::Radio)?;
        self.listen()
    }
}
