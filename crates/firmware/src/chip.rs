//! Per-chip constants. Exactly one `chip-*` feature must be enabled; esp-hal's
//! own chip features are mutually exclusive, so "all four chips" means four
//! separate builds, never one.
//!
//! This skeleton only proves the build/link path end-to-end, so it doesn't
//! yet wire up the CC1101 SPI bus — `SCK`/`MOSI`/`MISO` are unused until that
//! lands, hence the blanket allow below rather than one per constant.
#![allow(dead_code)]

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
    /// CC1101 GDO2 — RX data out.
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
    pub const GDO2_RX: u8 = 12;
}
