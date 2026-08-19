//! Per-chip constants — for one chip, since 2026-08-19, and the feature is kept
//! anyway. `chip-s3` still has to be named on a build: esp-hal's own chip
//! features are mutually exclusive and it has no default, so "which chip" is a
//! question this crate is better off asking out loud than answering by
//! accident.
//!
//! The pin numbers here are documentation and diagnostics; the pins the driver
//! actually claims come from [`cc1101_pins!`], which names the same GPIOs as
//! esp-hal singletons. The two must be edited together — a `u8` and a
//! `peripherals.GPIOn` field cannot be tied to each other at compile time, so
//! this is the one place in the crate where a mismatch would only show up as a
//! misleading log line. `check_pin_map` in `main.rs` compares them at boot.
//!
//! Note there is no blanket `allow(dead_code)` here: every pin constant is read
//! by that check, so an unused item in this file is worth hearing about.

use esp_hal::gpio::AnyPin;

// **The two-chip guard is gone with the second chip; the zero-chip one stays.**
//
// It is not what a mis-invoked build actually hits — esp-println's build script
// rejects a zero-feature build before this crate is compiled — but it costs
// nothing, it names the problem in one line instead of leaving it to a
// dependency, and it keeps working if the upstream check ever moves or is
// relaxed. It is also the reason `rust-analyzer.toml` exists: an editor
// checking with no features sees this sentence rather than a grey crate.
#[cfg(not(feature = "chip-s3"))]
compile_error!("no chip selected: build with --features chip-s3");

/// RMT source clock, and the divider giving 1 µs ticks from it.
pub const RMT_CLOCK_MHZ: u32 = 80;

/// Divider giving 1 µs ticks from `RMT_CLOCK_MHZ`.
pub const RMT_CLK_DIVIDER: u8 = 80;

// The two constants above are only correct relative to each other, and
// `somfy-rmt` converts every pulse duration to ticks assuming the pair
// resolves to `somfy_rmt::TICK_US`. A change to one without the other would
// not fail to build or to link — it would silently scale every duration in
// every frame, so a shade would simply stop responding with no error anywhere
// to explain why. Tie them together so that edit cannot compile.
const _: () = assert!(
    RMT_CLK_DIVIDER as u32 == RMT_CLOCK_MHZ * somfy_rmt::TICK_US,
    "RMT_CLK_DIVIDER must divide RMT_CLOCK_MHZ down to somfy_rmt::TICK_US"
);

// Pin map verified against a real working ESP32-S3 device on 2026-08-15.
pub mod pins {
    pub const SCK: u8 = 12;
    pub const MOSI: u8 = 11;
    pub const MISO: u8 = 13;
    pub const CSN: u8 = 10;
    /// CC1101 GDO0 — TX data in. NOTE: GPIO3 is an S3 strapping pin
    /// (JTAG source select); proven in production but the first suspect
    /// for any boot anomaly.
    pub const GDO0_TX: u8 = 3;
    /// CC1101 GDO2 — RX data out, claimed by the receive path.
    pub const GDO2_RX: u8 = 4;
}

/// The CC1101's pins, type-erased so one signature serves every chip.
pub struct Cc1101Pins<'d> {
    pub sck: AnyPin<'d>,
    pub mosi: AnyPin<'d>,
    pub miso: AnyPin<'d>,
    pub csn: AnyPin<'d>,
    /// CC1101 GDO0 — transmit data in.
    pub gdo0_tx: AnyPin<'d>,
    /// CC1101 GDO2 — receive data out.
    pub gdo2_rx: AnyPin<'d>,
}

/// Claims the CC1101's pins from an owned `Peripherals`.
///
/// A macro rather than a function because esp-hal's pin singletons are
/// distinct types moved out of `Peripherals` field by field: a function would
/// have to borrow the whole struct and would then keep `SPI2` and `RMT` locked
/// away behind that borrow. Expanding at the call site moves only the five
/// fields it names and leaves the rest usable.
///
/// It is still a macro with one chip left, for that reason rather than for the
/// per-chip one it also had.
#[macro_export]
macro_rules! cc1101_pins {
    ($peripherals:ident) => {
        $crate::chip::Cc1101Pins {
            sck: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO12),
            mosi: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO11),
            miso: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO13),
            csn: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO10),
            gdo0_tx: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO3),
            gdo2_rx: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO4),
        }
    };
}

/// Claims the two RMT channel creators the radio needs: transmit first,
/// receive second.
///
/// A macro for the same reason [`cc1101_pins!`] is one — the creators are
/// distinct types in named fields of `Rmt`, and moving two of them out by name
/// leaves the rest of the struct usable — and the numbers are easy to get wrong
/// in two ways that outlive the multi-chip matrix that first exposed them:
///
/// - **Not every channel can receive.** The ESP32-S3 splits them 0-3 transmit,
///   4-7 receive. Asking channel 1 to receive is not a runtime error, it is a
///   missing trait implementation. (Other Espressif parts split them
///   differently again — the ESP32-C3, built here until 2026-08-19, was 0-1 and
///   2-3 — so this stays a macro rather than becoming two constants.)
/// - **Each channel is configured for two memory blocks**, so it occupies its
///   neighbour's as well. That is why the receive channel is never the one
///   immediately after the transmit channel on a chip that shares a block pool:
///   channel 0 with `memsize = 2` already owns block 1.
///
/// ESP32-S3: channels 0-3 transmit, 4-7 receive, with separate memory
/// banks per direction.
#[macro_export]
macro_rules! rmt_channels {
    ($rmt:ident) => {
        ($rmt.channel0, $rmt.channel4)
    };
}
