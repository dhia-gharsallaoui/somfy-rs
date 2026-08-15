//! Bring-up binary for the transmit path.
//!
//! Brings the CC1101 up over SPI, builds one Somfy `Up` frame and clocks it out
//! of the RMT peripheral as a first frame plus its repeats. It transmits to a
//! **synthetic address**, not a paired shade: a real address moves real
//! hardware, and deliberately driving a motor is Task 7's job, done knowingly.
//! A production controller acting as receiver decodes this address whether or
//! not anything is paired to it, so the frame still proves itself on the air.

#![no_std]
#![no_main]

mod chip;
mod radio;

use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output, OutputConfig, Pin},
    main,
    rmt::{Rmt, TxChannelCreator},
    spi::{
        master::{Config as SpiConfig, Spi},
        Mode,
    },
    time::Rate,
};
use radio::rmt_tx::{self, tx_channel_config, RmtTx, TxError};
use somfy_cc1101::{Cc1101, Cc1101Error};
use somfy_rts::{encode56, Command, FrameError, FrameKind, RollingCode};

/// Not a paired shade. See the module docs.
const BRING_UP_ADDRESS: u32 = 0x00C0DE;

/// Starting rolling code for the bring-up frame. Nothing persists it: this
/// binary does not own a real remote's counter, and pretending otherwise would
/// invite someone to point it at a paired shade.
const BRING_UP_ROLLING_CODE: u16 = 0x000A;

/// Repeat frames sent after the first frame.
const REPEATS: u8 = 2;

/// SPI clock for the CC1101. The part accepts up to 10 MHz for single-byte
/// access but only 6.5 MHz for the burst reads this driver uses, so 4 MHz sits
/// clear of both.
const SPI_HZ: u32 = 4_000_000;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    // Nothing left to do on a bare-metal panic: halt here rather than spin
    // on real work, so an empty loop is the correct body, not a mistake.
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Anything that can stop bring-up, reported rather than panicked so the
/// failure names itself over the serial line.
///
/// Each payload exists precisely to be printed. rustc's dead-code analysis
/// deliberately does not count a derived `Debug` as a read, so without the
/// allow it reports every one of them as unused.
#[allow(dead_code)]
#[derive(Debug)]
enum BringUpError {
    Spi(esp_hal::spi::master::ConfigError),
    ChipSelect,
    Radio(Cc1101Error),
    Rmt(esp_hal::rmt::ConfigError),
    Frame(FrameError),
    Tx(TxError),
    /// A pin claimed does not match the pin `chip::pins` documents.
    PinMap {
        claimed: u8,
        documented: u8,
    },
}

#[main]
fn entry() -> ! {
    match bring_up() {
        Ok(()) => esp_println::println!("tx bring-up complete"),
        Err(error) => esp_println::println!("tx bring-up failed: {:?}", error),
    }
    #[allow(clippy::empty_loop)]
    loop {}
}

fn bring_up() -> Result<(), BringUpError> {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let pins = crate::cc1101_pins!(peripherals);
    check_pin_map(&pins)?;

    let bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SPI_HZ))
            .with_mode(Mode::_0),
    )
    .map_err(BringUpError::Spi)?
    .with_sck(pins.sck)
    .with_mosi(pins.mosi)
    .with_miso(pins.miso);

    let chip_select = Output::new(pins.csn, Level::High, OutputConfig::default());

    // `new`, not `new_no_delay`: the CC1101's reset sequence holds chip select
    // low across a settle delay, expressed as a delay operation *inside* the
    // SPI transaction. The no-delay constructor panics when it meets one, which
    // would abort inside `init` on the very first radio call and read exactly
    // like a dead or miswired radio.
    let device = ExclusiveDevice::new(bus, chip_select, Delay::new())
        .map_err(|_| BringUpError::ChipSelect)?;

    let mut radio = Cc1101::new(device);
    radio.init().map_err(BringUpError::Radio)?;

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(chip::RMT_CLOCK_MHZ))
        .map_err(BringUpError::Rmt)?;
    let channel = rmt
        .channel0
        .configure_tx(&tx_channel_config())
        .map_err(BringUpError::Rmt)?
        .with_pin(pins.gdo0_tx);
    let mut tx = RmtTx::new(channel);

    let mut rolling = RollingCode(BRING_UP_ROLLING_CODE);
    let frame = rolling.next_frame(Command::Up, BRING_UP_ADDRESS);
    let bytes = encode56(&frame).map_err(BringUpError::Frame)?;

    // Logged before the radio is keyed, not per frame inside the loop below.
    // Serial output takes milliseconds, and the loop is a timing-critical
    // region: a 56-bit frame carries its own 27434 µs inter-frame gap at the
    // tail of its pulse train, and a print between frames would stretch that
    // by however long the serial transport happened to take. Every field here
    // is identical across the burst anyway; only the kind varies, and it is
    // just first-then-repeats.
    esp_println::println!(
        "tx: address={:#08X} command={:?} rolling_code={} kind={:?} then {} x {:?}",
        frame.address,
        frame.command,
        frame.rolling_code,
        FrameKind::First,
        REPEATS,
        FrameKind::Repeat,
    );

    // The radio only keys a carrier to follow the data pin while it is in TX,
    // so it must be strobed in before the first symbol and back out after the
    // last one — otherwise the RMT clocks a perfectly good pulse train into
    // a transmitter that is switched off.
    radio.set_tx().map_err(BringUpError::Radio)?;
    // `set_tx` returns as soon as the strobe is on the wire, but the chip
    // calibrates its synthesiser before enabling the transmitter and radiates
    // nothing until that finishes. Waiting here is what keeps the wake-up
    // pulse's leading edge on the air; see `rmt_tx::TX_SETTLE_US`.
    Delay::new().delay_micros(rmt_tx::TX_SETTLE_US);

    let mut result = Ok(());
    for repeat in 0..=REPEATS {
        let kind = if repeat == 0 {
            FrameKind::First
        } else {
            FrameKind::Repeat
        };
        if let Err(error) = tx.transmit_frame(&bytes, kind) {
            result = Err(BringUpError::Tx(error));
            break;
        }
    }

    // Park the radio whether or not the transmission succeeded: leaving the
    // synthesiser running after a failure wastes power and holds the band. A
    // transmit failure outranks a parking failure in the report — it is the
    // one that explains why nothing moved.
    let parked = radio.set_idle().map_err(BringUpError::Radio);
    result.and(parked)
}

/// Check the pins actually claimed against the map `chip::pins` documents.
///
/// The two are independent statements of the same fact — `chip::pins` is the
/// hardware-verified record referenced by `docs/provenance.md`, while
/// `cc1101_pins!` is what the silicon is told to do — and nothing ties them
/// together at compile time, because a GPIO singleton's number is not available
/// as a constant. A divergence would put the radio on pins the documentation
/// does not describe, which presents as a radio that answers on SPI and then
/// transmits nothing: the exact fault this project has already lost hours to.
fn check_pin_map(pins: &chip::Cc1101Pins<'_>) -> Result<(), BringUpError> {
    for (claimed, documented) in [
        (pins.sck.number(), chip::pins::SCK),
        (pins.mosi.number(), chip::pins::MOSI),
        (pins.miso.number(), chip::pins::MISO),
        (pins.csn.number(), chip::pins::CSN),
        (pins.gdo0_tx.number(), chip::pins::GDO0_TX),
    ] {
        if claimed != documented {
            return Err(BringUpError::PinMap {
                claimed,
                documented,
            });
        }
    }
    Ok(())
}
