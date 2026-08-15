//! Per-chip constants. Exactly one `chip-*` feature must be enabled; esp-hal's
//! own chip features are mutually exclusive, so "all four chips" means four
//! separate builds, never one.
//!
//! The pin numbers here are documentation and diagnostics; the pins the driver
//! actually claims come from [`cc1101_pins!`], which names the same GPIOs as
//! esp-hal singletons. The two must be edited together — a `u8` and a
//! `peripherals.GPIOn` field cannot be tied to each other at compile time, so
//! this is the one place in the crate where a mismatch would only show up as a
//! misleading log line. `check_pin_map` in `main.rs` compares them at boot.
//!
//! Note there is no blanket `allow(dead_code)` here: the pin constants are all
//! read by that check now, so an unused item in this file is once again worth
//! hearing about. Only `GDO2_RX` carries an allow, and only until a receive
//! path claims it.

use esp_hal::gpio::AnyPin;

#[cfg(not(any(
    feature = "chip-esp32",
    feature = "chip-s2",
    feature = "chip-s3",
    feature = "chip-c3"
)))]
compile_error!(
    "no chip selected: build with exactly one of \
     --features chip-esp32 | chip-s2 | chip-s3 | chip-c3"
);

// Two (or more) chip features enabled at once would otherwise expand two
// `pub mod pins` definitions into the same scope, which surfaces as a
// confusing "duplicate definition" error far from its real cause. Fail with
// a message that names the actual problem instead.
//
// In practice neither `compile_error!` above or below is what a
// mis-invoked build actually hits: esp-println's build script rejects a
// zero-feature build, and esp-metadata-generated emits ~24 duplicate-macro
// errors for a two-feature build, both before this crate is compiled. These
// stay as a backstop — they cost nothing, they name the problem in one line
// instead of twenty-four, and they keep working if the upstream checks ever
// move or are relaxed.
#[cfg(any(
    all(feature = "chip-esp32", feature = "chip-s2"),
    all(feature = "chip-esp32", feature = "chip-s3"),
    all(feature = "chip-esp32", feature = "chip-c3"),
    all(feature = "chip-s2", feature = "chip-s3"),
    all(feature = "chip-s2", feature = "chip-c3"),
    all(feature = "chip-s3", feature = "chip-c3"),
))]
compile_error!(
    "multiple chip features selected: build with exactly one of \
     --features chip-esp32 | chip-s2 | chip-s3 | chip-c3"
);

/// RMT source clock. **Must** be 80 MHz on ESP32 and ESP32-S2 (esp-hal
/// constraint); the others are configured the same for one tick model.
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
#[cfg(feature = "chip-s3")]
pub mod pins {
    pub const SCK: u8 = 12;
    pub const MOSI: u8 = 11;
    pub const MISO: u8 = 13;
    pub const CSN: u8 = 10;
    /// CC1101 GDO0 — TX data in. NOTE: GPIO3 is an S3 strapping pin
    /// (JTAG source select); proven in production but the first suspect
    /// for any boot anomaly.
    pub const GDO0_TX: u8 = 3;
    /// CC1101 GDO2 — RX data out. Nothing claims it yet; the receive path
    /// is a later task.
    #[allow(dead_code)]
    pub const GDO2_RX: u8 = 4;
}

// UNVERIFIED defaults: nobody has confirmed these against real hardware.
// The chip-s3 map above is the only one checked against a working device —
// see docs/provenance.md for where these numbers came from. Do not wire a
// board to these pins without checking them first.
#[cfg(feature = "chip-esp32")]
pub mod pins {
    pub const SCK: u8 = 18;
    pub const MOSI: u8 = 23;
    pub const MISO: u8 = 19;
    pub const CSN: u8 = 5;
    pub const GDO0_TX: u8 = 13;
    /// CC1101 GDO2 — RX data out. Nothing claims it yet; the receive path
    /// is a later task.
    #[allow(dead_code)]
    pub const GDO2_RX: u8 = 12;
}

// UNVERIFIED defaults — see the note above `chip-esp32`'s pin module.
#[cfg(feature = "chip-s2")]
pub mod pins {
    pub const SCK: u8 = 36;
    pub const MOSI: u8 = 35;
    pub const MISO: u8 = 37;
    pub const CSN: u8 = 34;
    pub const GDO0_TX: u8 = 15;
    /// CC1101 GDO2 — RX data out. Nothing claims it yet; the receive path
    /// is a later task.
    #[allow(dead_code)]
    pub const GDO2_RX: u8 = 14;
}

// UNVERIFIED defaults — see the note above `chip-esp32`'s pin module.
#[cfg(feature = "chip-c3")]
pub mod pins {
    pub const SCK: u8 = 15;
    pub const MOSI: u8 = 16;
    pub const MISO: u8 = 17;
    pub const CSN: u8 = 14;
    pub const GDO0_TX: u8 = 13;
    /// CC1101 GDO2 — RX data out. Nothing claims it yet; the receive path
    /// is a later task.
    #[allow(dead_code)]
    pub const GDO2_RX: u8 = 12;
}

/// The CC1101's pins, type-erased so one signature serves every chip.
///
/// GDO2 (the receive-data pin) is deliberately absent: nothing drives or reads
/// it yet, and claiming a pin the firmware does not use would suggest a
/// receive path exists.
pub struct Cc1101Pins<'d> {
    pub sck: AnyPin<'d>,
    pub mosi: AnyPin<'d>,
    pub miso: AnyPin<'d>,
    pub csn: AnyPin<'d>,
    /// CC1101 GDO0 — transmit data in.
    pub gdo0_tx: AnyPin<'d>,
}

/// Claims the CC1101's pins from an owned `Peripherals`.
///
/// A macro rather than a function because esp-hal's pin singletons are
/// distinct types moved out of `Peripherals` field by field: a function would
/// have to borrow the whole struct and would then keep `SPI2` and `RMT` locked
/// away behind that borrow. Expanding at the call site moves only the five
/// fields it names and leaves the rest usable.
#[cfg(feature = "chip-s3")]
#[macro_export]
macro_rules! cc1101_pins {
    ($peripherals:ident) => {
        $crate::chip::Cc1101Pins {
            sck: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO12),
            mosi: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO11),
            miso: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO13),
            csn: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO10),
            gdo0_tx: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO3),
        }
    };
}

/// See the `chip-s3` definition above for why this is a macro.
#[cfg(feature = "chip-esp32")]
#[macro_export]
macro_rules! cc1101_pins {
    ($peripherals:ident) => {
        $crate::chip::Cc1101Pins {
            sck: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO18),
            mosi: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO23),
            miso: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO19),
            csn: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO5),
            gdo0_tx: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO13),
        }
    };
}

/// See the `chip-s3` definition above for why this is a macro.
#[cfg(feature = "chip-s2")]
#[macro_export]
macro_rules! cc1101_pins {
    ($peripherals:ident) => {
        $crate::chip::Cc1101Pins {
            sck: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO36),
            mosi: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO35),
            miso: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO37),
            csn: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO34),
            gdo0_tx: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO15),
        }
    };
}

/// See the `chip-s3` definition above for why this is a macro.
#[cfg(feature = "chip-c3")]
#[macro_export]
macro_rules! cc1101_pins {
    ($peripherals:ident) => {
        $crate::chip::Cc1101Pins {
            sck: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO15),
            mosi: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO16),
            miso: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO17),
            csn: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO14),
            gdo0_tx: ::esp_hal::gpio::Pin::degrade($peripherals.GPIO13),
        }
    };
}
