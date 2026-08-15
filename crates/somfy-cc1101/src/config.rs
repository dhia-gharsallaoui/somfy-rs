//! Register addresses and the derived value of every byte [`init`] writes.
//!
//! [`init`]: crate::Cc1101::init
//!
//! Naming convention: `REG_*` is a register **address**; the bare register name
//! is the **value** this driver writes to it.
//!
//! Every value below is either built from named bit-field constants whose
//! arithmetic is shown beside them, or — in the one case where the datasheet
//! offers no formula — explicitly flagged as a table lookup. There are no
//! unexplained bytes here by design.

/// Crystal frequency, in hertz.
///
/// Every frequency-domain register is a function of this. The part accepts a
/// 26 MHz or 27 MHz crystal; the modules used here are fitted with 26 MHz, and
/// all the arithmetic below assumes it.
pub const F_XOSC_HZ: u32 = 26_000_000;

/// Target carrier, in hertz.
pub const CARRIER_HZ: u32 = 433_420_000;

// ---------------------------------------------------------------------------
// SPI header bits
// ---------------------------------------------------------------------------

/// Header bit 7: read rather than write.
pub(crate) const HEADER_READ: u8 = 0x80;

/// Header bit 6: burst access (consecutive addresses) rather than single.
pub(crate) const HEADER_BURST: u8 = 0x40;

// ---------------------------------------------------------------------------
// Command strobes
// ---------------------------------------------------------------------------

/// Reset the chip.
pub(crate) const STROBE_SRES: u8 = 0x30;

/// Enable TX.
pub(crate) const STROBE_STX: u8 = 0x35;

/// Return to IDLE, switching the synthesiser and any active mode off.
pub(crate) const STROBE_SIDLE: u8 = 0x36;

/// How long to hold chip-select low after `SRES` before talking to the chip.
///
/// **This number is not derived and the datasheet does not contain it.** The
/// reset is not instantaneous — the crystal has to restart and the digital core
/// has to come out of reset — but the only completion criterion given is a
/// handshake: the chip drives its MISO line low when it is ready, and chip
/// select must stay asserted until it does. No microsecond bound is published.
///
/// That handshake cannot be observed through [`embedded_hal::spi::SpiDevice`],
/// which owns the chip-select line and gives no way to watch MISO between
/// operations. So the driver substitutes a fixed wait, taken with chip select
/// still asserted, which is the condition the reset sequence requires. One
/// millisecond is a margin choice: it is more than an order of magnitude beyond
/// the longest oscillator-settling figure the datasheet quotes anywhere
/// (roughly 600 µs), and it is paid once per boot.
pub(crate) const RESET_SETTLE_NS: u32 = 1_000_000;

// ---------------------------------------------------------------------------
// Identity (status) registers
// ---------------------------------------------------------------------------

/// Part number. Shares its address with the `SRES` strobe.
///
/// Status registers and command strobes occupy the same addresses; the burst
/// bit is what tells them apart, so these are only reachable as burst reads.
/// Burst *sequences* are not available for status registers though — they have
/// to be read one at a time, which is why the identity check is two separate
/// transactions rather than one three-byte read.
pub(crate) const REG_PARTNUM: u8 = 0x30;

/// Silicon revision. Shares its address with the `SFSTXON` strobe.
pub(crate) const REG_VERSION: u8 = 0x31;

/// `PARTNUM` as reported by a CC1101.
///
/// Zero, which on its own is a weak check — a bus reading back all-zeros would
/// pass it. It is paired with [`KNOWN_VERSIONS`] for that reason: a dead bus
/// returns either all-zeros or all-ones, and neither is a valid version.
pub(crate) const PARTNUM_CC1101: u8 = 0x00;

/// `VERSION` values that identify CC1101 silicon this driver knows.
///
/// `0x14` is what current parts report. `0x04` is what the datasheet published
/// before revision I and what older silicon answers; both are accepted.
///
/// **This list is deliberately an allowlist, and that is a trade-off.** The
/// datasheet annotates `VERSION` "subject to change without notice", so a
/// genuine future part could report a third value and be turned away. The
/// alternative — accepting anything that is not `0x00` or `0xFF` — would let a
/// miswired or floating bus that happens to return a plausible byte pass as a
/// working radio. Rejecting an unknown chip is the louder failure, and widening
/// this list is a one-line, reviewable change; silently configuring an
/// unidentified part is not.
pub(crate) const KNOWN_VERSIONS: &[u8] = &[0x04, 0x14];

// ---------------------------------------------------------------------------
// Register addresses
// ---------------------------------------------------------------------------

pub(crate) const REG_IOCFG2: u8 = 0x00;
pub(crate) const REG_IOCFG0: u8 = 0x02;
pub(crate) const REG_PKTCTRL1: u8 = 0x07;
pub(crate) const REG_PKTCTRL0: u8 = 0x08;
pub(crate) const REG_ADDR: u8 = 0x09;
pub(crate) const REG_FREQ2: u8 = 0x0D;
pub(crate) const REG_FREQ1: u8 = 0x0E;
pub(crate) const REG_FREQ0: u8 = 0x0F;
pub(crate) const REG_MDMCFG4: u8 = 0x10;
pub(crate) const REG_MDMCFG3: u8 = 0x11;
pub(crate) const REG_MDMCFG2: u8 = 0x12;
pub(crate) const REG_DEVIATN: u8 = 0x15;
pub(crate) const REG_MCSM0: u8 = 0x18;
pub(crate) const REG_FREND0: u8 = 0x22;
pub(crate) const REG_PATABLE: u8 = 0x3E;

// ---------------------------------------------------------------------------
// IOCFG2 / IOCFG0 — the two data-path pins
// ---------------------------------------------------------------------------

/// `GDOx_CFG` = 0x0D: the pin outputs demodulated serial data.
const GDO_CFG_SERIAL_DATA_OUT: u8 = 0x0D;

/// `GDOx_CFG` = 0x2E: the pin's output driver is high impedance.
const GDO_CFG_HIGH_IMPEDANCE: u8 = 0x2E;

/// GDO2 carries received data out of the chip, for the MCU to sample.
pub(crate) const IOCFG2: u8 = GDO_CFG_SERIAL_DATA_OUT;

/// GDO0 carries transmit data *into* the chip.
///
/// That direction comes from the mode, not from this register: asynchronous
/// serial mode is defined as data in on GDO0 and data out on one of the GDO
/// pins, so while transmitting the modulator reads this pin. What this register
/// still decides is what the CC1101 drives onto the same wire the rest of the
/// time — when idle or receiving. The MCU owns that wire as an output, so
/// anything but high impedance risks two drivers on one net.
///
/// Choosing high impedance also means the setting is safe whether or not the
/// mode overrides it: the datasheet describes the TX-input role as a
/// consequence of the mode but never states that it ignores this field, and a
/// three-stated output is correct under either reading.
pub(crate) const IOCFG0: u8 = GDO_CFG_HIGH_IMPEDANCE;

// ---------------------------------------------------------------------------
// FREQ2 / FREQ1 / FREQ0 — carrier frequency
// ---------------------------------------------------------------------------

/// The 24-bit `FREQ` word.
///
/// Datasheet: `f_carrier = (f_xosc / 2^16) * (FREQ + CHAN * channel spacing)`.
/// The driver never writes `CHANNR`, and the reset that opens
/// [`init`](crate::Cc1101::init) leaves `CHAN` at zero, so the channel term
/// vanishes and the whole thing reduces to
/// `FREQ = f_carrier * 2^16 / f_xosc`:
///
/// ```text
/// FREQ = 433_420_000 * 65_536 / 26_000_000
///      = 28_404_613_120_000 / 26_000_000
///      = 1_092_485.12       ->  1_092_485  =  0x10_AB_85
/// ```
///
/// One `FREQ` step is `26 MHz / 2^16` = 396.73 Hz, so discarding that 0.12 of a
/// step costs 47.6 Hz: the synthesiser actually lands on **433_419_952 Hz**,
/// 0.11 ppm below target. The crystal's own tolerance is tens of ppm — several
/// kHz — so the rounding is two orders of magnitude smaller than the error the
/// hardware contributes anyway, and far inside any receiver's capture range.
const FREQ: u32 = {
    let scaled = CARRIER_HZ as u64 * 65_536;
    // Adding half the divisor before dividing rounds to nearest.
    ((scaled + (F_XOSC_HZ as u64) / 2) / F_XOSC_HZ as u64) as u32
};

pub(crate) const FREQ2: u8 = (FREQ >> 16) as u8;
pub(crate) const FREQ1: u8 = (FREQ >> 8) as u8;
pub(crate) const FREQ0: u8 = FREQ as u8;

// ---------------------------------------------------------------------------
// MDMCFG4 / MDMCFG3 — channel filter bandwidth and symbol rate
// ---------------------------------------------------------------------------

/// `MDMCFG4.CHANBW_E`, with [`CHANBW_M`]: receiver channel filter bandwidth.
///
/// Datasheet: `BW = f_xosc / (8 * (4 + CHANBW_M) * 2^CHANBW_E)`. Both fields
/// are two bits, so at 26 MHz the entire achievable set is sixteen values. The
/// narrowest four:
///
/// ```text
/// E = 3, M = 0  ->  26e6 / (8 * 4 * 8)  =  101.5625 kHz
/// E = 3, M = 1  ->  26e6 / (8 * 5 * 8)  =   81.25   kHz
/// E = 3, M = 2  ->  26e6 / (8 * 6 * 8)  =   67.71   kHz
/// E = 3, M = 3  ->  26e6 / (8 * 7 * 8)  =   58.04   kHz
/// ```
///
/// This radio is specified to run at 99.97 kHz, which **is not one of them**.
/// No `(E, M)` pair yields it at 26 MHz and the register has no finer step, so
/// the figure cannot be met exactly. 101.5625 kHz is 1.6% high; the next
/// setting down, 81.25 kHz, is 18.7% low and would narrow the filter enough to
/// start attenuating the signal. The nearest value wins: E = 3, M = 0.
const CHANBW_E: u8 = 3;

/// `MDMCFG4.CHANBW_M`; see [`CHANBW_E`] for the derivation.
const CHANBW_M: u8 = 0;

/// The Somfy RTS half-symbol period, in microseconds.
///
/// RTS keys the carrier in 640 µs half-symbols, so the on-air chip rate is
/// `1 / 640 µs` = 1562.5 baud. The period is the hardware-verified quantity, so
/// it is what gets written down; the rate is computed from it.
const SOMFY_HALF_SYMBOL_US: u32 = 640;

/// `MDMCFG4.DRATE_E`, with [`DRATE_M`]: symbol rate.
///
/// Transmission here is asynchronous serial, where the modulator follows the
/// GDO0 pin directly and this rate goes unused. It matters on receive, where it
/// sets the demodulator's decimation and post-filter bandwidth — so it is
/// tuned to the signal actually expected on air: one chip per
/// [`SOMFY_HALF_SYMBOL_US`].
///
/// Datasheet: `R = ((256 + DRATE_M) * 2^DRATE_E / 2^28) * f_xosc`, with
/// `DRATE_M` eight bits (so `256 + M` spans 256..=511) and `DRATE_E` four bits:
///
/// ```text
/// (256 + M) * 2^E = 1562.5 * 2^28 / 26e6 = 16_131.94
///   E = 4  ->  256 + M = 1008.2   above 511, rejected
///   E = 6  ->  256 + M =  252.1   below 256, rejected
///   E = 5  ->  256 + M =  504.1   ->  M = 248, the only fit
/// R = 504 * 2^5 / 2^28 * 26e6 = 1562.1 baud   (0.024% under target)
/// ```
const DRATE_E: u8 = 5;

/// `MDMCFG3.DRATE_M`; see [`DRATE_E`] for the derivation.
const DRATE_M: u8 = 248;

/// `[7:6]` CHANBW_E, `[5:4]` CHANBW_M, `[3:0]` DRATE_E.
pub(crate) const MDMCFG4: u8 = (CHANBW_E << 6) | (CHANBW_M << 4) | DRATE_E;

/// `[7:0]` DRATE_M.
pub(crate) const MDMCFG3: u8 = DRATE_M;

// ---------------------------------------------------------------------------
// MDMCFG2 — modulation and sync
// ---------------------------------------------------------------------------

/// `MDMCFG2.MOD_FORMAT` = 3: ASK/OOK.
///
/// The carrier is switched between two power levels rather than two
/// frequencies. That is what Somfy motors listen for.
const MOD_FORMAT_ASK_OOK: u8 = 3;

/// `MDMCFG2.SYNC_MODE` = 4: no preamble or sync word, qualified by carrier
/// sense.
///
/// There is no sync word to match. A Somfy frame's preamble is a run of
/// hardware-sync pulses that the packet engine never sees, because in
/// asynchronous serial mode the framing rides on the data pin instead.
/// Requiring received energy above the carrier-sense threshold keeps the
/// receiver from presenting noise as data while the band is quiet.
const SYNC_MODE_NONE_CARRIER_SENSE: u8 = 4;

/// `[7]` DEM_DCFILT_OFF = 0, `[6:4]` MOD_FORMAT, `[3]` MANCHESTER_EN = 0,
/// `[2:0]` SYNC_MODE.
///
/// The DC blocking filter stays enabled (bit 7 clear), which is what the
/// demodulator wants.
///
/// Manchester encoding stays **off**. Somfy is Manchester-coded, but at the
/// 640 µs half-symbol level, in a pulse train this firmware builds itself; the
/// chip's own bit-level Manchester coder would encode that a second time. It is
/// unavailable in asynchronous serial mode in any case.
pub(crate) const MDMCFG2: u8 = (MOD_FORMAT_ASK_OOK << 4) | SYNC_MODE_NONE_CARRIER_SENSE;

// ---------------------------------------------------------------------------
// PKTCTRL0 / PKTCTRL1 / ADDR — packet engine, bypassed
// ---------------------------------------------------------------------------

/// `PKTCTRL0.PKT_FORMAT` = 3: asynchronous serial mode.
///
/// The mode this whole driver exists to select. It reduces the CC1101 to a
/// modulator that follows a pin.
const PKT_FORMAT_ASYNC_SERIAL: u8 = 3;

/// `PKTCTRL0.LENGTH_CONFIG` = 2: infinite packet length.
///
/// A frame lasts exactly as long as the MCU drives the data pin. There is no
/// length for the packet engine to enforce, and "infinite" is how the register
/// says so.
const LENGTH_CONFIG_INFINITE: u8 = 2;

/// `[6]` WHITE_DATA = 0, `[5:4]` PKT_FORMAT, `[2]` CRC_EN = 0, `[1:0]`
/// LENGTH_CONFIG.
///
/// Whitening and CRC are both off. Both are packet-engine features that
/// asynchronous serial mode bypasses, and a CRC appended to a Somfy frame would
/// be so many extra microseconds of carrier that no motor is listening for.
pub(crate) const PKTCTRL0: u8 = (PKT_FORMAT_ASYNC_SERIAL << 4) | LENGTH_CONFIG_INFINITE;

/// `[7:5]` PQT = 0, `[3]` CRC_AUTOFLUSH = 0, `[2]` APPEND_STATUS = 0,
/// `[1:0]` ADR_CHK = 0.
///
/// Address filtering off: Somfy addresses live inside the frame payload and
/// mean nothing to the CC1101's own address matcher. The preamble-quality
/// threshold, the auto-flush and the appended RSSI/LQI status bytes are all
/// packet-engine features this mode never reaches, so the register is zero
/// throughout.
pub(crate) const PKTCTRL1: u8 = 0x00;

/// Device address. Never compared, because address checking is off above.
///
/// Written anyway so the value is one this driver chose rather than whatever a
/// reset happened to leave, and so enabling address checking later cannot
/// silently inherit a stale address.
pub(crate) const ADDR: u8 = 0x00;

// ---------------------------------------------------------------------------
// MCSM0 — automatic calibration
// ---------------------------------------------------------------------------

/// `MCSM0.FS_AUTOCAL` = 1: calibrate the synthesiser on every transition out of
/// IDLE into RX or TX.
///
/// This one is load-bearing, and it is the only register here that the target
/// configuration did not call for.
///
/// The driver parks the chip in IDLE between frames and strobes `STX` for each
/// transmission, and the synthesiser's VCO must be calibrated before it will
/// actually sit on the programmed frequency. The datasheet makes the `STX`
/// strobe's behaviour conditional on exactly this field: from IDLE it enables
/// TX, and performs a calibration first *if* `MCSM0.FS_AUTOCAL` is 1. Its reset
/// value is 0, "never — manually calibrate using the SCAL strobe".
///
/// So leaving this register alone would mean transmitting on an uncalibrated
/// VCO forever, off the programmed frequency, while every register read still
/// came back perfectly healthy. Nothing else in the system would notice.
const FS_AUTOCAL_IDLE_TO_RX_TX: u8 = 1;

/// `MCSM0.PO_TIMEOUT` = 1: 16 crystal counts, roughly 37–39 µs — the reset
/// setting, kept.
///
/// It sets how long the chip waits for the crystal to stabilise when the
/// oscillator is started. This driver never powers the chip down, so the value
/// is inert here; it is named and carried explicitly because writing a register
/// at all means choosing every field in it, including the ones you are keeping.
const PO_TIMEOUT_RESET_DEFAULT: u8 = 1;

/// `[5:4]` FS_AUTOCAL, `[3:2]` PO_TIMEOUT, `[1]` PIN_CTRL_EN = 0,
/// `[0]` XOSC_FORCE_ON = 0.
///
/// Pin radio control is off — this driver drives the chip over SPI strobes, not
/// through the GDO pins — and the crystal is allowed to stop in the SLEEP state.
pub(crate) const MCSM0: u8 = (FS_AUTOCAL_IDLE_TO_RX_TX << 4) | (PO_TIMEOUT_RESET_DEFAULT << 2);

// ---------------------------------------------------------------------------
// FREND0 / PATABLE — output power
// ---------------------------------------------------------------------------

/// `FREND0.PA_POWER` = 1: which `PATABLE` entry supplies the "on" level.
///
/// On-off keying needs two power levels, not one. The modulator takes the "0"
/// level from `PATABLE[0]` and the "1" level from `PATABLE[PA_POWER]`, so
/// `PA_POWER` must be non-zero for OOK to key anything at all: at 0 both levels
/// would come from the same entry and the carrier would sit there unmodulated.
/// The datasheet is direct about the value — for OOK, the logic-0 and logic-1
/// power levels are to be programmed at indices 0 and 1 respectively.
///
/// (There is a separate case, ASK *shaping*, where the transmitter ramps
/// through the whole table and `PA_POWER` should be 7. That is not this: Somfy
/// wants a hard-keyed carrier, not a shaped envelope.)
const PA_POWER: u8 = 1;

/// `FREND0.LODIV_BUF_CURRENT_TX` = 1, its reset value.
///
/// A front-end bias setting with no published formula and no reason to move it.
const LODIV_BUF_CURRENT_TX: u8 = 1;

/// `[5:4]` LODIV_BUF_CURRENT_TX, `[2:0]` PA_POWER.
pub(crate) const FREND0: u8 = (LODIV_BUF_CURRENT_TX << 4) | PA_POWER;

/// The PA power table: `[0]` is the OOK "0" level, `[1]` the OOK "1" level.
///
/// **Neither byte is derived. This is the one lookup in the file.**
///
/// `0x00` switches the power amplifier off, which is the "0" of on-off keying.
///
/// `0xC0` is the tabulated setting for **+10 dBm in the 433 MHz band**. The PA
/// register is a group of bias and ramp fields with no published closed-form
/// relation to output power — the datasheet gives a table of one byte per band
/// per power level and nothing to compute — so there is genuinely nothing to
/// derive here.
///
/// It is also not an exact match for what was asked. This radio is specified to
/// run at a transmit power of 11, and there is no +11 dBm setting for 433 MHz:
/// the only table covering this band runs −30, −20, −15, −10, 0, +5, +7, +10
/// dBm and stops. +10 dBm is its maximum, so that is what this is — the top of
/// the table standing in for a level that cannot be expressed. A +11 dBm row
/// does exist elsewhere in the datasheet, in the table for 868/915 MHz parts
/// built with wire-wound inductors, and its byte is also `0xC0`; whichever of
/// the two the figure came from, `0xC0` is the answer. See
/// `docs/provenance.md`.
///
/// Two operational notes from the datasheet, both relevant to callers: writing
/// any entry other than index 0 *requires* a burst write, which is why
/// `write_patable` sends both bytes in one transaction; and everything except
/// index 0 is lost if the chip enters its SLEEP state, so a driver that ever
/// powers the radio down has to write this table again on the way back up.
pub(crate) const PATABLE_OOK: [u8; 2] = [0x00, 0xC0];

// ---------------------------------------------------------------------------
// The write order
// ---------------------------------------------------------------------------

/// Every `(address, value)` pair [`init`] writes, in address order.
///
/// `PATABLE` is not here: it lives outside the configuration register space and
/// is written as a burst afterwards.
///
/// [`init`]: crate::Cc1101::init
pub(crate) const CONFIG: &[(u8, u8)] = &[
    (REG_IOCFG2, IOCFG2),
    (REG_IOCFG0, IOCFG0),
    (REG_PKTCTRL1, PKTCTRL1),
    (REG_PKTCTRL0, PKTCTRL0),
    (REG_ADDR, ADDR),
    (REG_FREQ2, FREQ2),
    (REG_FREQ1, FREQ1),
    (REG_FREQ0, FREQ0),
    (REG_MDMCFG4, MDMCFG4),
    (REG_MDMCFG3, MDMCFG3),
    (REG_MDMCFG2, MDMCFG2),
    (REG_DEVIATN, DEVIATN),
    (REG_MCSM0, MCSM0),
    (REG_FREND0, FREND0),
];

// ---------------------------------------------------------------------------
// DEVIATN — nominal frequency deviation
// ---------------------------------------------------------------------------

/// `DEVIATN.DEVIATION_E`, with [`DEVIATION_M`]: nominal deviation 47.60 kHz.
///
/// Datasheet: `f_dev = (f_xosc / 2^17) * (8 + DEVIATION_M) * 2^DEVIATION_E`,
/// with `DEVIATION_M` three bits (so `8 + M` spans 8..=15) and `DEVIATION_E`
/// three bits:
///
/// ```text
/// f_xosc / 2^17 = 26_000_000 / 131_072 = 198.3643 Hz
/// (8 + M) * 2^E = 47_600 / 198.3643    = 239.96
///   E = 3  ->  8 + M = 30.0   above 15, rejected
///   E = 5  ->  8 + M =  7.5   below  8, rejected
///   E = 4  ->  8 + M = 15.0   ->  M = 7, the only fit
/// f_dev = 198.3643 * 15 * 2^4 = 47_607.4 Hz   (7.4 Hz over target)
/// ```
///
/// The register only affects the FSK, GFSK and MSK modulators, so in ASK/OOK it
/// does nothing at all. It is written regardless, so the radio's state is
/// entirely this driver's choice rather than a mixture of chosen values and
/// whatever a reset left behind.
const DEVIATION_E: u8 = 4;

/// `DEVIATN.DEVIATION_M`; see [`DEVIATION_E`] for the derivation.
const DEVIATION_M: u8 = 7;

/// `[7]` reserved, `[6:4]` DEVIATION_E, `[3]` reserved, `[2:0]` DEVIATION_M.
pub(crate) const DEVIATN: u8 = (DEVIATION_E << 4) | DEVIATION_M;

// ---------------------------------------------------------------------------
// The derivations, run forwards and checked
// ---------------------------------------------------------------------------
//
// Each mantissa/exponent pair above was solved by hand and the working written
// beside it. These constants push those choices back through the datasheet
// formulas and assert the results, so the prose cannot drift away from the
// bytes: change a field and the crate stops compiling until the comment's
// stated outcome is corrected too.

/// Carrier the synthesiser actually lands on, in hertz.
pub const ACHIEVED_CARRIER_HZ: u32 = ((FREQ as u64 * F_XOSC_HZ as u64) / 65_536) as u32;

/// Channel filter bandwidth actually selected, in hertz (truncated).
pub const ACHIEVED_CHANBW_HZ: u32 = F_XOSC_HZ / (8 * (4 + CHANBW_M as u32) * (1 << CHANBW_E));

/// Nominal deviation actually selected, in hertz (truncated).
pub const ACHIEVED_DEVIATION_HZ: u32 =
    (((F_XOSC_HZ as u64 * (8 + DEVIATION_M as u64)) << DEVIATION_E) / 131_072) as u32;

/// Symbol rate actually selected, in millibaud.
pub const ACHIEVED_RATE_MBAUD: u32 =
    ((((256 + DRATE_M as u64) << DRATE_E) * F_XOSC_HZ as u64 * 1_000) >> 28) as u32;

/// One chip per Somfy half-symbol, in millibaud: 1 562 500, i.e. 1562.5 baud.
const TARGET_RATE_MBAUD: u32 = 1_000_000_000 / SOMFY_HALF_SYMBOL_US;

const _: () = {
    // 433.42 MHz requested; 47.6 Hz low, which is 0.11 ppm.
    assert!(ACHIEVED_CARRIER_HZ == 433_419_952);
    assert!(CARRIER_HZ - ACHIEVED_CARRIER_HZ == 48);

    // 99.97 kHz requested, and unreachable — see CHANBW_E. This is the nearest
    // setting the register can express.
    assert!(ACHIEVED_CHANBW_HZ == 101_562);

    // 47.60 kHz requested; 7.4 Hz high.
    assert!(ACHIEVED_DEVIATION_HZ == 47_607);

    // Within 0.1% of the Somfy chip rate.
    let error = TARGET_RATE_MBAUD - ACHIEVED_RATE_MBAUD;
    assert!(error * 1_000 < TARGET_RATE_MBAUD);

    // The composed bytes, so a mis-shifted bit field cannot slip through.
    assert!(FREQ2 == 0x10 && FREQ1 == 0xAB && FREQ0 == 0x85);
    assert!(MDMCFG4 == 0xC5 && MDMCFG3 == 0xF8 && MDMCFG2 == 0x34);
    assert!(DEVIATN == 0x47 && PKTCTRL0 == 0x32 && MCSM0 == 0x14 && FREND0 == 0x11);
};
