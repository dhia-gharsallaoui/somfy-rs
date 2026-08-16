//! Register addresses and the derived value of every byte [`init`] writes.
//!
//! [`init`]: crate::Cc1101::init
//!
//! Naming convention: `REG_*` is a register **address**; the bare register name
//! is the **value** this driver writes to it.
//!
//! Every value below is either built from named bit-field constants whose
//! arithmetic is shown beside them, or — where the datasheet offers no formula
//! — explicitly flagged as a table lookup, a vendor recommendation, or a value
//! measured on this project's own hardware. There are no unexplained bytes here
//! by design.

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

/// Enable RX.
pub(crate) const STROBE_SRX: u8 = 0x34;

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
pub(crate) const REG_AGCCTRL2: u8 = 0x1B;
pub(crate) const REG_AGCCTRL1: u8 = 0x1C;
pub(crate) const REG_AGCCTRL0: u8 = 0x1D;
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
///
/// **The carrier-sense half of this setting does nothing here, and an earlier
/// version of this comment claimed otherwise.** It said that requiring energy
/// above the carrier-sense threshold kept the receiver from presenting noise as
/// data in a quiet band. It does not: sync-word qualification is a packet-engine
/// feature, and the datasheet is explicit that asynchronous serial mode disables
/// the packet handling hardware and that "no data decision is done on-chip" —
/// the raw demodulated level goes to the pin either way. Hardware agreed
/// emphatically: with this setting in place and the AGC unconfigured, GDO2
/// carried roughly 750 noise edges per second in a silent band. What actually
/// quiets the receiver is the AGC, further down.
///
/// The value is left at 4 regardless. It is part of a register byte that is
/// hardware-proven on the transmit path, settings 0 and 4 are identical in
/// every respect this mode can observe, and changing a proven byte to no effect
/// is not worth the risk. Only the false claim about it is removed.
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
// AGCCTRL2 / AGCCTRL1 / AGCCTRL0 — receiver gain control
// ---------------------------------------------------------------------------
//
// These three registers are the receive-side gain loop, and until they were
// added nothing here configured them at all. That is invisible while the radio
// only ever transmits — the AGC has no transmit-side role — and fatal the
// moment it has to listen.
//
// Left at their reset values the gain is free to wind all the way up, so a
// quiet 433 MHz band gets amplified until the demodulator is slicing its own
// noise floor, and GDO2 carries a continuous stream of demodulated noise.
// Measured on this hardware with nothing transmitting: **roughly 750 edges per
// second**, with the line high about 70% of the time. Anything downstream that
// waits for a gap in the data to decide a transmission has ended never gets
// one, so a real frame arriving inside that noise is discarded along with it.
//
// **The datasheet derives none of this, and says so.** It states that the
// settings its own design tool produces for FSK/MSK "are not optimum" under
// ASK/OOK and hands the question to a separate vendor application note (DN022,
// "CC110x/CC111x OOK/ASK Register Settings"), which answers with measured
// recommendations rather than formulas:
//
// ```text
// AGCCTRL2 = 0x03 to 0x07
// AGCCTRL1 = 0x00
// AGCCTRL0 = 0x91 or 0x92
// ```
//
// That recommendation is not the answer either, for a reason worth stating
// plainly: **two of those three values are already what a reset leaves
// behind.** AGCCTRL2 resets to 0x03 and AGCCTRL0 to 0x91, so writing the note's
// advice verbatim would change one bit of one register — and the 750 edges per
// second measured above is precisely the behaviour of 0x03/0x91. The note
// optimises packet error rate against a signal generator driving a real signal;
// it never asks what the receiver does in an empty band.
//
// The values below come from a fourth source, and it is the one that settled
// it: the **defaults of a widely deployed third-party CC1101 Arduino driver
// library** (`ELECHOUSE_CC1101_SRC_DRV`, v2.5.7), which is what OOK remote
// projects on this exact hardware actually run. They are field-proven for this
// application rather than derived, and this project then re-measured them
// against a real transmitter before adopting them. There is no arithmetic to
// show because the datasheet publishes none for the AGC; what there is, is a
// measurement, recorded per field below and in `docs/provenance.md`.
//
// **None of the three changes what the radio transmits.** Every field is a
// receive-path quantity — LNA and DVGA gain, the amplitude the channel filter
// is driven to, the OOK slicing boundary. There is exactly one path by which
// they could reach the transmitter and it is worth naming, because it is closed
// by a detail elsewhere rather than by anything here: `AGCCTRL1`'s carrier-sense
// thresholds feed Clear Channel Assessment, and `MCSM1.CCA_MODE` — which this
// driver leaves at its reset value 3, i.e. armed — makes an `STX` strobe
// *issued from the RX state* conditional on the channel being clear. The
// firmware never does that; it strobes `SIDLE` and then `STX`, and from IDLE
// the datasheet's strobe table makes `STX` unconditional. The coupling is
// therefore latent, not live — and it points the safe way in any case, since
// both [`MAX_DVGA_GAIN`] and [`MAGN_TARGET`] raise the carrier-sense threshold,
// which makes CCA report "clear" more readily, never less.

/// `AGCCTRL2.MAX_DVGA_GAIN` = 3: the three highest DVGA gain settings are
/// barred.
///
/// **Not derived — a field-proven library default, re-measured here.** The
/// vendor note's `AGCCTRL2` span of 0x03–0x07 varies only [`MAGN_TARGET`] and
/// leaves this field at 0, "all gain settings can be used", which is the reset
/// behaviour that makes the receiver slice its own noise floor.
///
/// Each setting removes another of the digital variable-gain amplifier's top
/// steps, which raises the weakest signal the receiver will respond to. Swept
/// on this hardware in a quiet band, counting edges on GDO2 over a two-second
/// window with nothing transmitting, holding `MAX_LNA_GAIN` at 0 and
/// `MAGN_TARGET` at 3:
///
/// ```text
/// MAX_DVGA_GAIN = 0  ->  1461, 1598, 1572 edges   (the reset behaviour)
/// MAX_DVGA_GAIN = 1  ->    58,  104,   78 edges
/// MAX_DVGA_GAIN = 2  ->     0,    2,    0 edges
/// MAX_DVGA_GAIN = 3  ->     0,    0,    0 edges
/// ```
///
/// **An earlier revision of this file read 2, on the argument that it was the
/// smallest value that silenced the band and that every further step costs
/// sensitivity. That argument was measuring the wrong thing.** Silence in an
/// empty band says nothing about whether a frame survives, and this field does
/// not act alone: paired with [`MAGN_TARGET`] at 7 rather than 3, setting 3
/// receives *further*, not less far. Measured against a real transmitter with
/// the signal deliberately walked out of the channel filter to simulate
/// distance, this pairing decoded 12 of 12 frames where the old one decoded 0
/// of 6 — see [`MAGN_TARGET`] for the table.
///
/// The nominal price is still quantifiable, from a datasheet table rather than
/// a formula — see [`CS_THRESHOLD_DBM_TENTHS`] — and it is now about 18 dB of
/// carrier-sense threshold. That figure did not predict the measurement and is
/// recorded as indicative only.
const MAX_DVGA_GAIN: u8 = 3;

/// `AGCCTRL2.MAX_LNA_GAIN` = 0: the low-noise amplifier keeps its full range.
///
/// Its reset value, kept deliberately. The datasheet offers this field as the
/// *first* knob to reach for when raising the detection threshold — ahead of
/// [`MAX_DVGA_GAIN`] — but for a stated reason that is not this one: doing it
/// in that order "will reduce power consumption in the receiver front end".
///
/// Optimising for sensitivity instead points the other way. The LNA sets the
/// receiver's noise figure, so gain taken out of it is lost before the signal
/// is ever amplified, while the DVGA sits after the channel filter where the
/// noise figure is already fixed. The measurement agrees: limiting this field
/// alone squelched far less per dB spent than limiting the DVGA did — 152 to
/// 330 edges per second still at setting 3, and 8 to 16 at setting 7, against
/// silence from capping the DVGA.
///
/// It is also the field [`CS_THRESHOLD_DBM_TENTHS`] is pinned against: the
/// datasheet's carrier-sense table is read along the `MAX_LNA_GAIN` = 0 row, so
/// moving this would leave that figure describing nothing. The assertion beside
/// it is what enforces the pairing.
const MAX_LNA_GAIN: u8 = 0;

/// `AGCCTRL2.MAGN_TARGET` = 7: the AGC drives the channel filter output to a
/// 42 dB amplitude.
///
/// A **table lookup** for the decibel figure — the datasheet's eight settings
/// map to 24, 27, 30, 33, 36, 38, 40 and 42 dB, in steps that are not uniform
/// and are therefore not computable; see [`MAGN_TARGET_DB`] — and an
/// **empirical choice** for which of the eight to use. It is the top of the
/// vendor note's recommended `AGCCTRL2` range (the note's 0x03 through 0x07
/// *is* this field taking the values 3 through 7) and the value the field-proven
/// library default carries.
///
/// **Raising this is what lets a weak frame through at all, and it only became
/// visible once the receiver was measured against a weak signal rather than
/// against an empty band.** With a strong transmitter every sane setting
/// decodes, which is why a sweep in a quiet room cannot rank them at all.
/// Walking the received signal out of the 101.6 kHz channel filter with a
/// synthesiser offset — a calibrated stand-in for distance — separates them
/// immediately. Complete, checksum-valid 56-bit frames decoded out of confirmed
/// transmissions, three attempts per cell:
///
/// ```text
/// offset                            38 kHz  51 kHz  64 kHz  76 kHz
/// MAX_DVGA_GAIN 2, MAGN_TARGET 3       0/3     0/3       -       -
/// MAX_DVGA_GAIN 3, MAGN_TARGET 7       3/3     3/3     3/3     3/3
/// MAX_DVGA_GAIN 1, MAGN_TARGET 7       2/3     3/3     2/3     3/3
/// MAX_DVGA_GAIN 2, MAGN_TARGET 7         -       -     1/3     2/3
/// ```
///
/// Read it as a pair, not as one field: at `MAGN_TARGET` 3 nothing gets through
/// at any gain cap, and once it is 7 the cap decides how much margin is left,
/// with 3 the best of the three measured. Neither field alone explains the
/// result, which is why both moved together and why the set is adopted as a set.
///
/// The datasheet's warning against raising it stands and is not dismissed: it
/// calls the field "a compromise between blocker tolerance/selectivity and
/// sensitivity" and says increasing it "reduces the headroom for blockers, and
/// therefore close-in selectivity". **That cost has not been measured**, because
/// measuring it needs a blocker this project has not set up. What has been
/// measured is that the cautious setting loses the frame.
const MAGN_TARGET: u8 = 7;

/// `[7:6]` MAX_DVGA_GAIN, `[5:3]` MAX_LNA_GAIN, `[2:0]` MAGN_TARGET.
pub(crate) const AGCCTRL2: u8 = (MAX_DVGA_GAIN << 6) | (MAX_LNA_GAIN << 3) | MAGN_TARGET;

/// `AGCCTRL1.AGC_LNA_PRIORITY` = 0: LNA2 is turned down to its minimum before
/// the LNA is touched.
///
/// **A vendor recommendation with no derivation offered**, and the only bit in
/// which the note's `AGCCTRL1 = 0x00` differs from the reset value 0x40. The
/// datasheet presents the two strategies as alternatives and gives no rule for
/// choosing; the note picks this one for ASK/OOK, so this driver does too.
///
/// It made no measurable difference to the noise on this hardware — the sweep
/// that produced the numbers in [`MAX_DVGA_GAIN`] ran with this field at 0
/// throughout, and the reset-value baseline it is compared against had it at 1
/// with everything else identical. It is followed because it is the vendor's
/// answer for this modulation, not because anything here demonstrates it.
const AGC_LNA_PRIORITY_LNA2_FIRST: u8 = 0;

/// `AGCCTRL1.CARRIER_SENSE_REL_THR` = 0: relative carrier sense disabled — the
/// reset value, kept.
///
/// It asserts carrier sense on a sudden jump in RSSI rather than on an absolute
/// level, which is aimed at bands with a time-varying noise floor. Nothing in
/// this driver consumes carrier sense: the packet handler that would use it as
/// a sync-word qualifier is bypassed by asynchronous serial mode, and the one
/// remaining consumer is the CCA gate on an `STX` issued from RX, which the
/// firmware never issues. Kept at zero so it stays that way.
const CARRIER_SENSE_REL_THR_DISABLED: u8 = 0;

/// `AGCCTRL1.CARRIER_SENSE_ABS_THR` = 0: carrier sense asserts at the
/// [`MAGN_TARGET`] amplitude — the reset value, kept.
///
/// A signed 4-bit offset in 1 dB steps relative to [`MAGN_TARGET`], and zero
/// means no offset. Same reasoning as the relative threshold above: it is
/// written to a known value rather than tuned, because nothing reads it.
const CARRIER_SENSE_ABS_THR_AT_MAGN_TARGET: u8 = 0;

/// `[7]` not used, `[6]` AGC_LNA_PRIORITY, `[5:4]` CARRIER_SENSE_REL_THR,
/// `[3:0]` CARRIER_SENSE_ABS_THR.
pub(crate) const AGCCTRL1: u8 = (AGC_LNA_PRIORITY_LNA2_FIRST << 6)
    | (CARRIER_SENSE_REL_THR_DISABLED << 4)
    | CARRIER_SENSE_ABS_THR_AT_MAGN_TARGET;

/// `AGCCTRL0.HYST_LEVEL` = 2: medium hysteresis on the AGC's own gain-change
/// decision — the reset value, the vendor note's, and the library default's,
/// all agreeing. Kept.
const HYST_LEVEL_MEDIUM: u8 = 2;

/// `AGCCTRL0.WAIT_TIME` = 3: 32 channel-filter samples after a gain change
/// before the AGC starts accumulating again — the slowest the two-bit field
/// offers, against a reset value of 1 (16 samples).
///
/// **Not derived; the field-proven library default.** It is the one field here
/// that acts on the AGC's *speed* rather than its thresholds, and slowing the
/// loop is the right direction for this modulation for a reason the datasheet
/// states plainly elsewhere: under OOK the AGC cannot tell a keyed-off carrier
/// from an absent one, so a fast loop chases the modulation itself. The
/// measurement does not isolate this field from the two threshold changes it
/// arrived with, so it is adopted as part of a proven set rather than on
/// evidence of its own.
const WAIT_TIME_32_SAMPLES: u8 = 3;

/// `AGCCTRL0.AGC_FREEZE` = 0: normal operation, gain adjusted whenever needed.
///
/// The reset value, and the only setting that makes sense here. The three
/// alternatives freeze the gain — one of them on sync-word detection, which in
/// asynchronous serial mode never happens because the packet handler is
/// bypassed, and the other two under manual control this driver does not
/// exercise.
const AGC_FREEZE_NORMAL: u8 = 0;

/// `AGCCTRL0.FILTER_LENGTH` = 2: a 12 dB OOK decision boundary.
///
/// This field is overloaded by modulation: for FSK and MSK it is an averaging
/// length in channel-filter samples, and for ASK/OOK it is the decision
/// boundary — how far below the peak the demodulator puts the line between a
/// keyed carrier and no carrier. The datasheet tabulates 4, 8, 12 and 16 dB for
/// the four settings, which is uniform enough to check; see
/// [`OOK_DECISION_BOUNDARY_DB`].
///
/// The vendor note offers `AGCCTRL0` as "0x91 or 0x92", which is exactly this
/// field being 1 (8 dB) or 2 (12 dB), and does not choose. The field-proven
/// library default takes 12 dB, and so does this driver.
///
/// **This field is not independent of [`MAX_DVGA_GAIN`], and an earlier
/// revision of this file recorded the dependence as a property of the field
/// itself.** It said 12 dB was a trap: that GDO2 stopped producing edges but sat
/// *high* 99–100% of the time, a receiver stuck asserting a carrier that is not
/// there and indistinguishable from success to an edge count alone. That
/// measurement was real, and it was taken with `MAX_DVGA_GAIN` at 2. A wider
/// boundary sits lower relative to the peak, so it reads more as "carrier" —
/// which is exactly what pins the line high if the gain in front of it is too
/// high for the band. The two have to move together:
///
/// ```text
///                                    idle edges/s   line high
/// MAX_DVGA_GAIN 2, boundary 12 dB       54-101       2.2-3.9%
/// MAX_DVGA_GAIN 3, boundary 12 dB            0             0%
/// ```
///
/// At the gain this driver now sets, the wider boundary costs nothing in a
/// quiet band and buys the sensitivity recorded under [`MAGN_TARGET`]. The
/// original warning survives as the reason the pairing is checked rather than
/// assumed: **the idle line resting low is asserted by measurement on every
/// change to either field, not inferred from an edge count.**
const FILTER_LENGTH_OOK_12DB: u8 = 2;

/// `[7:6]` HYST_LEVEL, `[5:4]` WAIT_TIME, `[3:2]` AGC_FREEZE,
/// `[1:0]` FILTER_LENGTH.
pub(crate) const AGCCTRL0: u8 = (HYST_LEVEL_MEDIUM << 6)
    | (WAIT_TIME_32_SAMPLES << 4)
    | (AGC_FREEZE_NORMAL << 2)
    | FILTER_LENGTH_OOK_12DB;

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
    (REG_AGCCTRL2, AGCCTRL2),
    (REG_AGCCTRL1, AGCCTRL1),
    (REG_AGCCTRL0, AGCCTRL0),
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

/// The OOK/ASK decision boundary actually selected, in decibels.
///
/// The AGC fields have no datasheet formulas — this is the one exception, and
/// only because the published table happens to be uniform: settings 0 through 3
/// map to 4, 8, 12 and 16 dB, which is `4 * (setting + 1)`. Reproducing it
/// arithmetically is what lets a mistyped [`FILTER_LENGTH_OOK_12DB`] fail the
/// build instead of quietly moving the slicing threshold.
pub const OOK_DECISION_BOUNDARY_DB: u8 = 4 * (FILTER_LENGTH_OOK_12DB + 1);

/// Target amplitude out of the channel filter, in decibels.
///
/// **A table lookup with no formula behind it** — the datasheet's eight
/// settings step 24, 27, 30, 33, 36, 38, 40, 42 dB, which is 3 dB per step and
/// then 2 dB, so it cannot be computed. It is written out as the table it is.
pub const MAGN_TARGET_DB: u8 = [24, 27, 30, 33, 36, 38, 40, 42][MAGN_TARGET as usize];

/// The carrier-sense threshold the chosen gain limits imply, in **tenths of a
/// dBm**, so the half-decibel steps survive integer arithmetic.
///
/// **Also a table lookup, and an indicative one** — this is the closest the
/// datasheet comes to putting a number on what [`MAX_DVGA_GAIN`] costs. Its
/// table of typical RSSI at the carrier-sense threshold, along the
/// `MAX_LNA_GAIN` = 0 row, reads −97.5, −91.5, −85.5 and −79.5 dBm for the four
/// `MAX_DVGA_GAIN` settings: a uniform 6 dB per step, which is what is
/// reproduced here.
///
/// Three caveats, none of them small. That table is quoted for 868 MHz at
/// 2.4 kBaud, and this radio runs at 433 MHz at 1562 baud — the datasheet says
/// outright that "for other data rates, the user must generate similar tables".
/// It is the *carrier-sense* threshold, not the OOK slicing threshold, and the
/// two are only related through the same four gain fields. And it is quoted as
/// typical, not guaranteed. So the absolute figure is indicative; what it is
/// used for here is the **difference** — the 18.0 dB the DVGA cap moves the
/// carrier-sense threshold by — and that is the number the assertion pins.
///
/// **It is not a sensitivity figure, and reading it as one is what this
/// constant now exists to warn against.** An earlier revision of this file
/// treated the same difference as the cost of capping the gain and chose the
/// smallest cap that silenced the band to avoid paying it. Measured against a
/// real transmitter, the receiver with the *larger* number here decodes frames
/// the one with the smaller number loses entirely — see [`MAGN_TARGET`]. The
/// table predicts the carrier-sense threshold, which nothing in this driver
/// consumes; it does not predict where the OOK slicer ends up.
pub const CS_THRESHOLD_DBM_TENTHS: i32 = -975 + 60 * MAX_DVGA_GAIN as i32;

const _: () = {
    // The AGC bytes, so a mis-shifted field cannot slip through. 0x00 is the
    // vendor note's recommendation exactly; 0xC7 and 0xB2 are the field-proven
    // library defaults, and 0xC7 is deliberately outside the note's
    // recommended 0x03..=0x07 range for AGCCTRL2.
    assert!(AGCCTRL2 == 0xC7 && AGCCTRL1 == 0x00 && AGCCTRL0 == 0xB2);

    // A 12 dB OOK decision boundary, the wider of the two the note offers.
    // Safe only at this gain — see FILTER_LENGTH_OOK_12DB, which records the
    // idle measurement that pairs the two fields.
    assert!(OOK_DECISION_BOUNDARY_DB == 12);

    // 42 dB: the top of the field, and of the note's range.
    assert!(MAGN_TARGET_DB == 42);

    // The carrier-sense row this reads along is only the quoted one while the
    // LNA keeps its full gain. If that ever changes, the figure below stops
    // meaning anything and this assertion is the warning.
    assert!(MAX_LNA_GAIN == 0);

    // −79.5 dBm indicative, 18.0 dB above what an unlimited DVGA reaches.
    // Recorded, not spent: see CS_THRESHOLD_DBM_TENTHS for why this difference
    // is not the sensitivity cost it looks like.
    assert!(CS_THRESHOLD_DBM_TENTHS == -795);
    assert!(CS_THRESHOLD_DBM_TENTHS - (-975) == 180);
};

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
