//! # somfy-cc1101
//!
//! A minimal CC1101 driver for **on-off-keyed, asynchronous-serial**
//! transmission in the 433 MHz band.
//!
//! In asynchronous serial mode the CC1101 is a dumb modulator: it keys the
//! carrier on and off to follow whatever level the MCU drives onto the chip's
//! GDO0 pin, and keys a recovered level back out on GDO2. None of Somfy's
//! framing, symbol timing or rolling codes lives here — that is `somfy-rts`
//! (frames and pulse trains) and `somfy-rmt` (packing those pulses for the
//! ESP32 RMT peripheral). This crate's entire job is to put the radio in the
//! state where driving that one pin transmits, and where the other pin carries
//! what the radio hears.
//!
//! The driver speaks to nothing but [`embedded_hal::spi::SpiDevice`], so it
//! builds for `no_std` targets and is fully testable on the host. That is why
//! it is a crate of its own rather than a module inside the firmware: the
//! firmware crate cannot be compiled for the host at all.
//!
//! ## What `init` proves, and what it does not
//!
//! [`Cc1101::init`] returning `Ok` means the SPI **control path** works — the
//! chip answered with a plausible part number and version and accepted a
//! register set.
//!
//! It says nothing whatsoever about the **data path**: whether GDO0 and GDO2
//! are wired to the pins the firmware believes they are, whether an antenna is
//! attached, whether anything is radiating. Those are separate facts and must
//! be reported separately. A single "radio OK" indicator that an SPI register
//! read alone can satisfy is worse than none, because it actively steers
//! diagnosis away from a misconfigured data pin — which is a fault this project
//! has already lost hours to once.
//!
//! ## Register values
//!
//! Every byte [`Cc1101::init`] writes is assembled from named bit-field
//! constants in [`config`], each carrying the arithmetic that produced it —
//! where arithmetic exists. Several values have none, and each says so where it
//! is defined: the PA table entry (a datasheet lookup, no formula published),
//! the post-reset settle delay (a margin choice; the datasheet publishes no
//! figure), and the three AGC registers (the datasheet publishes no AGC
//! formulas at all and defers ASK/OOK gain settings to a vendor application
//! note, whose recommendations are measured rather than derived — and which
//! this driver then has to depart from on its own measurements, for reasons
//! recorded beside [`config::AGCCTRL2`]).

#![cfg_attr(not(test), no_std)]

pub mod config;

use embedded_hal::spi::{Operation, SpiDevice};

use config::{
    CONFIG, HEADER_BURST, HEADER_READ, KNOWN_VERSIONS, PARTNUM_CC1101, PATABLE_OOK, REG_PARTNUM,
    REG_PATABLE, REG_VERSION, RESET_SETTLE_NS, STROBE_SIDLE, STROBE_SRES, STROBE_SRX, STROBE_STX,
};

/// Everything that can go wrong talking to the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cc1101Error {
    /// The SPI transaction itself failed. The bus error is not carried: there
    /// is nothing this driver or its callers can do differently for one kind of
    /// bus failure versus another.
    Spi,
    /// `PARTNUM` did not identify a CC1101.
    BadPartNumber(u8),
    /// `VERSION` was not a silicon revision this driver knows.
    BadVersion(u8),
}

/// A CC1101 on the far end of an SPI device.
pub struct Cc1101<SPI> {
    spi: SPI,
}

impl<SPI: SpiDevice> Cc1101<SPI> {
    /// Wraps an SPI device. Talks to nothing until [`Cc1101::init`] is called.
    pub fn new(spi: SPI) -> Self {
        Self { spi }
    }

    /// Resets the chip, checks it really is a CC1101, and writes the OOK
    /// asynchronous-serial configuration.
    ///
    /// On success the radio is in IDLE, configured, and will transmit whatever
    /// is driven onto GDO0 once [`Cc1101::set_tx`] is strobed. See the crate
    /// docs for the limits of what this having succeeded actually tells you.
    pub fn init(&mut self) -> Result<(), Cc1101Error> {
        self.reset()?;

        let part_number = self.read_part_number()?;
        if part_number != PARTNUM_CC1101 {
            return Err(Cc1101Error::BadPartNumber(part_number));
        }

        let version = self.read_version()?;
        if !KNOWN_VERSIONS.contains(&version) {
            return Err(Cc1101Error::BadVersion(version));
        }

        for (address, value) in CONFIG {
            self.write_register(*address, *value)?;
        }

        self.write_patable()
    }

    /// Enables the transmitter. From IDLE this also calibrates the synthesiser,
    /// because `MCSM0.FS_AUTOCAL` was configured to make it.
    pub fn set_tx(&mut self) -> Result<(), Cc1101Error> {
        self.strobe(STROBE_STX)
    }

    /// Enables the receiver. From IDLE this also calibrates the synthesiser,
    /// for the same `MCSM0.FS_AUTOCAL` reason as [`Cc1101::set_tx`].
    ///
    /// In asynchronous serial mode the chip drives demodulated data onto GDO2
    /// only while it is receiving, so a receiver that never strobes this reads
    /// a pin that is simply idle — indistinguishable from a quiet band, and
    /// therefore from a working receiver that has heard nothing.
    ///
    /// Reception continues until something strobes the chip out of RX. That is
    /// a property of asynchronous serial mode, where the packet handler is
    /// bypassed entirely: there is no packet to end, so `MCSM1.RXOFF_MODE` —
    /// which this driver leaves at its reset default — has nothing to act on.
    pub fn set_rx(&mut self) -> Result<(), Cc1101Error> {
        self.strobe(STROBE_SRX)
    }

    /// Returns the chip to IDLE, switching off the synthesiser and whichever of
    /// TX or RX was running.
    pub fn set_idle(&mut self) -> Result<(), Cc1101Error> {
        self.strobe(STROBE_SIDLE)
    }

    /// Reads the `PARTNUM` status register.
    pub fn read_part_number(&mut self) -> Result<u8, Cc1101Error> {
        self.read_status(REG_PARTNUM)
    }

    /// Reads the `VERSION` status register.
    pub fn read_version(&mut self) -> Result<u8, Cc1101Error> {
        self.read_status(REG_VERSION)
    }

    /// Writes one configuration register.
    pub fn write_register(&mut self, address: u8, value: u8) -> Result<(), Cc1101Error> {
        self.spi
            .transaction(&mut [Operation::Write(&[address, value])])
            .map_err(|_| Cc1101Error::Spi)
    }

    /// Strobes the reset command and holds chip select low while the chip comes
    /// back.
    ///
    /// The delay is an operation *inside* the transaction rather than a wait
    /// between two transactions, which is what keeps chip select asserted
    /// across it — the condition the reset sequence requires. It does mean the
    /// `SpiDevice` implementation must honour [`Operation::DelayNs`]; bus
    /// wrappers that are constructed without a delay source typically panic on
    /// it rather than ignoring it.
    fn reset(&mut self) -> Result<(), Cc1101Error> {
        self.spi
            .transaction(&mut [
                Operation::Write(&[STROBE_SRES]),
                Operation::DelayNs(RESET_SETTLE_NS),
            ])
            .map_err(|_| Cc1101Error::Spi)
    }

    /// Burst-writes the two OOK power-table entries.
    ///
    /// A burst is not an optimisation here: index 0 is the only entry
    /// reachable by a single write, so the "1" level can only be set this way.
    fn write_patable(&mut self) -> Result<(), Cc1101Error> {
        let buffer = [HEADER_BURST | REG_PATABLE, PATABLE_OOK[0], PATABLE_OOK[1]];
        self.spi
            .transaction(&mut [Operation::Write(&buffer)])
            .map_err(|_| Cc1101Error::Spi)
    }

    /// Issues a single-byte command strobe.
    fn strobe(&mut self, strobe: u8) -> Result<(), Cc1101Error> {
        self.spi
            .transaction(&mut [Operation::Write(&[strobe])])
            .map_err(|_| Cc1101Error::Spi)
    }

    /// Reads one status register.
    ///
    /// Status registers share their addresses with the command strobes, and the
    /// burst bit is the only thing distinguishing the two — so a status read
    /// that forgot it would silently execute a strobe instead. Hence
    /// `HEADER_READ | HEADER_BURST`.
    ///
    /// The chip returns its status byte while the header goes out, and the
    /// register contents on the second byte.
    fn read_status(&mut self, address: u8) -> Result<u8, Cc1101Error> {
        let mut buffer = [HEADER_READ | HEADER_BURST | address, 0x00];
        self.spi
            .transaction(&mut [Operation::TransferInPlace(&mut buffer)])
            .map_err(|_| Cc1101Error::Spi)?;
        Ok(buffer[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::spi::{ErrorKind, ErrorType, Operation, SpiDevice};
    use embedded_hal_mock::eh1::spi::{Mock as SpiMock, Transaction as SpiTransaction};

    /// The header bytes and payloads `init` is expected to put on the wire,
    /// after the reset strobe and the two identity reads.
    ///
    /// Spelled out as literals on purpose: this is the contract the derivations
    /// in the source are supposed to produce, so a change to any derivation has
    /// to be acknowledged here too.
    const EXPECTED_CONFIG_WRITES: &[&[u8]] = &[
        &[0x00, 0x0D],       // IOCFG2   — GDO2 = serial data out
        &[0x02, 0x2E],       // IOCFG0   — GDO0 driver high-impedance
        &[0x07, 0x00],       // PKTCTRL1 — address check off
        &[0x08, 0x32],       // PKTCTRL0 — async serial, CRC off, infinite length
        &[0x09, 0x00],       // ADDR     — inert while address check is off
        &[0x0D, 0x10],       // FREQ2    | 433.42 MHz
        &[0x0E, 0xAB],       // FREQ1    |
        &[0x0F, 0x85],       // FREQ0    |
        &[0x10, 0xC5],       // MDMCFG4  — RX BW 101.5625 kHz, DRATE_E = 5
        &[0x11, 0xF8],       // MDMCFG3  — DRATE_M = 248
        &[0x12, 0x34],       // MDMCFG2  — ASK/OOK, sync mode 4
        &[0x15, 0x47],       // DEVIATN  — 47.61 kHz
        &[0x18, 0x14],       // MCSM0    — autocalibrate on IDLE -> TX/RX
        &[0x1B, 0xC7],       // AGCCTRL2 — DVGA gain capped, AGC target 42 dB
        &[0x1C, 0x00],       // AGCCTRL1 — LNA2 turned down first; carrier sense inert
        &[0x1D, 0xB2],       // AGCCTRL0 — slowest AGC wait, 12 dB OOK decision boundary
        &[0x22, 0x11],       // FREND0   — PA_POWER = 1 (OOK needs two levels)
        &[0x7E, 0x00, 0xC0], // PATABLE burst — off level, then +10 dBm
    ];

    /// Wraps one logical SPI exchange in the start/end markers the mock expects.
    fn txn(inner: SpiTransaction<u8>) -> [SpiTransaction<u8>; 3] {
        [
            SpiTransaction::transaction_start(),
            inner,
            SpiTransaction::transaction_end(),
        ]
    }

    /// The reset strobe holds CS low across the settle delay, so it is one
    /// transaction containing two operations.
    fn reset_expectations() -> Vec<SpiTransaction<u8>> {
        vec![
            SpiTransaction::transaction_start(),
            SpiTransaction::write_vec(vec![STROBE_SRES]),
            SpiTransaction::delay(RESET_SETTLE_NS),
            SpiTransaction::transaction_end(),
        ]
    }

    /// Identity reads: PARTNUM then VERSION, each a two-byte burst read.
    fn identity_expectations(partnum: u8, version: u8) -> Vec<SpiTransaction<u8>> {
        let mut v = Vec::new();
        v.extend(txn(SpiTransaction::transfer_in_place(
            vec![0xF0, 0x00],
            vec![0x00, partnum],
        )));
        v.extend(txn(SpiTransaction::transfer_in_place(
            vec![0xF1, 0x00],
            vec![0x00, version],
        )));
        v
    }

    /// VERSION (0x31) is a burst-read status register: address | 0xC0 = 0xF1.
    #[test]
    fn read_version_issues_a_burst_read_and_returns_the_status_byte() {
        let expectations = [
            SpiTransaction::transaction_start(),
            SpiTransaction::transfer_in_place(vec![0xF1, 0x00], vec![0x00, 0x99]),
            SpiTransaction::transaction_end(),
        ];
        let mut spi = SpiMock::new(&expectations);
        let mut radio = Cc1101::new(&mut spi);
        assert!(matches!(radio.read_version(), Ok(0x99)));
        spi.done();
    }

    #[test]
    fn init_rejects_an_unexpected_version() {
        let mut expectations = reset_expectations();
        expectations.extend(identity_expectations(PARTNUM_CC1101, 0x99));

        let mut spi = SpiMock::new(&expectations);
        let mut radio = Cc1101::new(&mut spi);

        assert_eq!(radio.init(), Err(Cc1101Error::BadVersion(0x99)));
        spi.done();
    }

    #[test]
    fn init_rejects_an_unexpected_part_number() {
        let mut expectations = reset_expectations();
        // Only PARTNUM is read: init must bail before asking for VERSION.
        expectations.extend(txn(SpiTransaction::transfer_in_place(
            vec![0xF0, 0x00],
            vec![0x00, 0x42],
        )));

        let mut spi = SpiMock::new(&expectations);
        let mut radio = Cc1101::new(&mut spi);

        assert_eq!(radio.init(), Err(Cc1101Error::BadPartNumber(0x42)));
        spi.done();
    }

    #[test]
    fn init_writes_the_ook_async_serial_register_set() {
        let mut expectations = reset_expectations();
        expectations.extend(identity_expectations(PARTNUM_CC1101, 0x14));
        for write in EXPECTED_CONFIG_WRITES {
            expectations.extend(txn(SpiTransaction::write_vec(write.to_vec())));
        }

        let mut spi = SpiMock::new(&expectations);
        let mut radio = Cc1101::new(&mut spi);

        assert_eq!(radio.init(), Ok(()));
        spi.done();
    }

    /// Both silicon revisions the datasheet accounts for must pass `init`.
    #[test]
    fn init_accepts_every_known_silicon_version() {
        for version in KNOWN_VERSIONS {
            let mut expectations = reset_expectations();
            expectations.extend(identity_expectations(PARTNUM_CC1101, *version));
            for write in EXPECTED_CONFIG_WRITES {
                expectations.extend(txn(SpiTransaction::write_vec(write.to_vec())));
            }

            let mut spi = SpiMock::new(&expectations);
            let mut radio = Cc1101::new(&mut spi);

            assert_eq!(radio.init(), Ok(()), "version {version:#04x} was rejected");
            spi.done();
        }
    }

    /// The three mode strobes, pinned as literals. A transposed pair here would
    /// put the radio in the opposite mode to the one asked for — a transmitter
    /// that hears and a receiver that radiates — and nothing in a build would
    /// say so.
    #[test]
    fn the_mode_strobes_are_srx_stx_and_sidle() {
        let mut expectations = Vec::new();
        expectations.extend(txn(SpiTransaction::write_vec(vec![0x34])));
        expectations.extend(txn(SpiTransaction::write_vec(vec![0x35])));
        expectations.extend(txn(SpiTransaction::write_vec(vec![0x36])));

        let mut spi = SpiMock::new(&expectations);
        let mut radio = Cc1101::new(&mut spi);

        assert_eq!(radio.set_rx(), Ok(()));
        assert_eq!(radio.set_tx(), Ok(()));
        assert_eq!(radio.set_idle(), Ok(()));
        spi.done();
    }

    /// An `SpiDevice` whose every transaction fails, to pin the error mapping.
    struct FailingSpi;

    impl ErrorType for FailingSpi {
        type Error = ErrorKind;
    }

    impl SpiDevice for FailingSpi {
        fn transaction(&mut self, _ops: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
            Err(ErrorKind::Other)
        }
    }

    #[test]
    fn bus_failures_surface_as_the_spi_error() {
        let mut radio = Cc1101::new(FailingSpi);
        assert_eq!(radio.read_version(), Err(Cc1101Error::Spi));
        assert_eq!(radio.init(), Err(Cc1101Error::Spi));
        assert_eq!(radio.set_tx(), Err(Cc1101Error::Spi));
        assert_eq!(radio.set_rx(), Err(Cc1101Error::Spi));
        assert_eq!(radio.set_idle(), Err(Cc1101Error::Spi));
    }
}
