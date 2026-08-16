//! The controller: two Embassy tasks, four static channels, and the hardware
//! they own.
//!
//! Boot brings up the flash-backed rolling-code store, the CC1101, and both RMT
//! channels, then hands them to the two loops `somfy-tasks` defines and gets out
//! of the way. Everything either loop *does* is over there, host-tested; this
//! file is wiring, and it is deliberately the only place where an `esp-hal`
//! peripheral and a task body meet.
//!
//! ## This image transmits nothing on its own
//!
//! The state task transmits only what it is commanded to, and Plan 4 ships no
//! command source — the command channel has no producer until Plan 5's API
//! layer arrives. Nor does it ship a config store, so the shade registry starts
//! and stays empty. Flashing this therefore produces a controller that listens,
//! decodes, logs what it hears, and keys the transmitter never.
//!
//! That is intended, and it is also a safety property worth being explicit
//! about: no boot of this image can move a shade. `tx-check` is the binary that
//! keys the radio, at a synthetic address, on purpose.
//!
//! ## What a boot proves
//!
//! The lines this prints are the ones that cannot be established anywhere else:
//! the store's survey (a device that has never stored a code versus one whose
//! codes are gone), the stack headroom the transmit path needs, and every frame
//! the receiver decodes. None of it is the radio reporting on its own
//! transmissions — a transmitter's account of itself is wrong in the same way
//! its output is.

#![no_std]
#![no_main]

mod chip;
mod radio;
mod store;
mod tasks;

use embassy_executor::{SpawnError, Spawner};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output, OutputConfig, Pin},
    interrupt::software::SoftwareInterruptControl,
    rmt::{Rmt, RxChannelCreator, TxChannelCreator},
    spi::{
        master::{Config as SpiConfig, Spi},
        Mode,
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_storage::FlashStorage;
use somfy_cc1101::{Cc1101, Cc1101Error};
use somfy_tasks::{
    CommandChannel, DeltaChannel, FrameChannel, RadioLoop, StateMachine, TransmitChannel, TxProfile,
};

use radio::air::{Air, AirError};
use radio::rmt_rx::{rx_channel_config, RmtPulseSource};
use radio::rmt_tx::{tx_channel_config, RmtTx};
use store::{FlashStore, StoreError};
use tasks::Mutex;

// Emits the ESP-IDF application descriptor into the image. The second-stage
// bootloader expects it, and espflash refuses to write a binary that lacks one
// — so without this line the firmware builds and links cleanly and then cannot
// be put on a device at all. It contributes no runtime behaviour, which is
// exactly why its absence is invisible until the moment you try to flash.
esp_bootloader_esp_idf::esp_app_desc!();

/// SPI clock for the CC1101. The part accepts up to 10 MHz for single-byte
/// access but only 6.5 MHz for the burst reads this driver uses, so 4 MHz sits
/// clear of both.
const SPI_HZ: u32 = 4_000_000;

/// Main stack this firmware refuses to start without.
///
/// `RmtTx::transmit_frame` needs roughly 6.5 KB, nearly all of it in
/// `somfy_rmt::build_symbols`'s two fixed 320-pulse buffers. Those are locals of
/// a synchronous call, so they sit on the stack of whatever polls the radio
/// task — and Embassy tasks have no stacks of their own, so that is the main
/// stack. 8 KB is that figure plus room for the call frames above and below it.
///
/// Checked rather than asserted because it cannot be a constant: esp-hal's
/// linker script gives the stack **whatever DRAM is left after the statics**,
/// so the figure moves every time a static is added. On a device with plenty
/// spare this check never fires; on one where a future Plan's buffers have
/// eaten the margin, it fires at boot with a number rather than corrupting a
/// pulse train.
const REQUIRED_STACK_BYTES: usize = 8 * 1024;

/// Transmissions from the state task to the radio task.
///
/// The producer end is only ever reachable as a `somfy_store::TransmitQueue`,
/// which is what makes "the rolling code is in flash before the frame is on the
/// air" a property of the type system rather than of review. See
/// `somfy_tasks::queue`.
static TRANSMIT: TransmitChannel<Mutex> = TransmitChannel::new();

/// Decoded frames from the radio task to the state task.
static FRAMES: FrameChannel<Mutex> = FrameChannel::new();

/// Commands into the state task. **No producer exists yet** — this is the seam
/// Plan 5's API layer plugs into.
static COMMANDS: CommandChannel<Mutex> = CommandChannel::new();

/// State deltas out of the state task. **No subscriber exists yet**, for the
/// same reason; publishing with none discards immediately.
static DELTAS: DeltaChannel<Mutex> = DeltaChannel::new();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    // Nothing left to do on a bare-metal panic: halt here rather than spin
    // on real work, so an empty loop is the correct body, not a mistake.
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Anything that can stop the controller starting, reported rather than
/// panicked so the failure names itself over the serial line.
///
/// Each payload exists precisely to be printed. rustc's dead-code analysis
/// deliberately does not count a derived `Debug` as a read, so without the
/// allow it reports every one of them as unused.
#[allow(dead_code)]
#[derive(Debug)]
enum StartError {
    Spi(esp_hal::spi::master::ConfigError),
    ChipSelect,
    Radio(Cc1101Error),
    Rmt(esp_hal::rmt::ConfigError),
    Air(AirError),
    Store(StoreError),
    /// A pin claimed does not match the pin `chip::pins` documents.
    PinMap {
        claimed: u8,
        documented: u8,
    },
    /// The main stack is smaller than the transmit path needs. See
    /// [`REQUIRED_STACK_BYTES`].
    StackTooSmall {
        available: usize,
        required: usize,
    },
    Spawn(SpawnError),
}

#[esp_rtos::main]
async fn entry(spawner: Spawner) {
    match start(spawner) {
        Ok(()) => esp_println::println!("controller: running"),
        Err(error) => esp_println::println!("controller: failed to start: {:?}", error),
    }
    // Returning is correct: the executor outlives this function and keeps
    // polling the two tasks that were spawned. A failure leaves nothing
    // spawned and the message above is the whole report.
}

fn start(spawner: Spawner) -> Result<(), StartError> {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // The scheduler behind the executor needs a timer and a software interrupt.
    //
    // Note what is *not* started: the second core. `esp_rtos::start_second_core`
    // is never called, so the app core stays parked — which is what keeps
    // `esp-storage`'s multi-core write strategy from refusing every commit. See
    // `store`'s module docs for why that refusal is the right default and why
    // parking the other core instead would be worse.
    let timers = TimerGroup::new(peripherals.TIMG0);
    let software = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timers.timer0, software.software_interrupt0);

    check_stack_headroom()?;

    let pins = crate::cc1101_pins!(peripherals);
    check_pin_map(&pins)?;

    // Mounted here rather than inside the state task: `mount` wants roughly
    // 5 KB of stack for the partition table and `esp-storage`'s sector buffer,
    // and doing it before anything is spawned keeps that spike away from the
    // radio task's own stack needs. Every later operation is far cheaper.
    let mut store = FlashStore::mount(FlashStorage::new(peripherals.FLASH))
        .map_err(StartError::Store)?;
    report_store(&mut store)?;

    let bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SPI_HZ))
            .with_mode(Mode::_0),
    )
    .map_err(StartError::Spi)?
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
        .map_err(|_| StartError::ChipSelect)?;

    let mut cc1101 = Cc1101::new(device);
    cc1101.init().map_err(StartError::Radio)?;

    // `into_async` converts the whole peripheral, both channels with it. The
    // receiver has to be asynchronous — a blocking receive busy-polls with no
    // deadline, and a shade may go untouched for hours — and the driver mode is
    // not per channel, so the transmitter is asynchronous too.
    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(chip::RMT_CLOCK_MHZ))
        .map_err(StartError::Rmt)?
        .into_async();
    let (transmit, receive) = crate::rmt_channels!(rmt);
    let transmit = transmit
        .configure_tx(&tx_channel_config())
        .map_err(StartError::Rmt)?
        .with_pin(pins.gdo0_tx);
    let receive = receive
        .configure_rx(&rx_channel_config())
        .map_err(StartError::Rmt)?
        .with_pin(pins.gdo2_rx);

    // Listening from here on. The radio leaves receive only for the length of a
    // burst, and `Air::key_off` puts it back; nothing else in the firmware holds
    // a radio handle, so there is nowhere else a mode change could come from.
    let mut air = Air::new(cc1101, RmtTx::new(transmit));
    air.listen().map_err(StartError::Air)?;

    // `#[task]` hands back a token or a `SpawnError`; `Spawner::spawn` itself is
    // infallible once it has one, so the fallible half is the token.
    let radio = tasks::radio(RadioLoop::new(
        RmtPulseSource::new(receive),
        air,
        TRANSMIT.requests(),
        FRAMES.sender(),
    ))
    .map_err(StartError::Spawn)?;
    spawner.spawn(radio);

    // An empty registry: Plan 4 ships no config store, so no shade is
    // provisioned and nothing can be commanded. Said out loud because a silent
    // empty controller and a broken one look identical from the serial line.
    let machine = StateMachine::new(TxProfile::default());
    esp_println::println!(
        "controller: 0 shades provisioned — receiving and tracking only, \
         nothing will transmit until a config store and a command source exist"
    );

    let state = tasks::state(
        machine,
        store,
        TRANSMIT.queue(),
        FRAMES.receiver(),
        COMMANDS.receiver(),
        DELTAS.immediate_publisher(),
    )
    .map_err(StartError::Spawn)?;
    spawner.spawn(state);

    Ok(())
}

/// Print what the rolling-code region holds before anything writes to it.
///
/// The difference between "this device has never stored a code" and "this
/// device's codes are gone" is exactly what
/// `docs/specs/2026-08-15-config-integrity-requirements.md` R1 requires be
/// distinguishable, and no amount of "the store mounted OK" can tell you which
/// one this is. `damaged` above zero on a device nobody power-cut deserves a
/// look.
fn report_store(store: &mut FlashStore<'_>) -> Result<(), StartError> {
    let (base, slots, slot_len) = store.geometry();
    esp_println::println!(
        "store: partition '{}' at {:#010X}, {} slots of {} bytes",
        store::PARTITION_LABEL,
        base,
        slots,
        slot_len,
    );

    let survey = store.survey().map_err(StartError::Store)?;
    esp_println::println!(
        "store: survey slots={} valid={} blank={} damaged={} newest_seq={:?} addresses={}",
        survey.slots,
        survey.valid,
        survey.blank,
        survey.damaged,
        survey.newest_seq,
        survey.addresses,
    );
    Ok(())
}

/// Refuse to start if the main stack is smaller than the transmit path needs.
///
/// See [`REQUIRED_STACK_BYTES`] for why this is a runtime check rather than a
/// `const` assertion. A stack overflow here would present as random corruption
/// in a pulse train — a shade that responds intermittently, with nothing
/// anywhere pointing at the cause — so it is worth a number at boot.
fn check_stack_headroom() -> Result<(), StartError> {
    // The symbols esp-hal's own linker script defines for the main stack, read
    // exactly as esp-hal itself reads them (`soc::ensure_stack_pointer_in_range`),
    // which is `pub(crate)` and so cannot be called from here.
    unsafe extern "C" {
        static _stack_end_cpu0: u32;
        static _stack_start_cpu0: u32;
    }
    // Neither is dereferenced — only the addresses themselves are taken, which
    // is what makes this safe and why no `unsafe` block is needed for it.
    let bottom = (&raw const _stack_end_cpu0) as usize;
    let top = (&raw const _stack_start_cpu0) as usize;
    let available = top.saturating_sub(bottom);
    esp_println::println!(
        "stack: {} bytes available, {} required",
        available,
        REQUIRED_STACK_BYTES,
    );
    if available < REQUIRED_STACK_BYTES {
        return Err(StartError::StackTooSmall {
            available,
            required: REQUIRED_STACK_BYTES,
        });
    }
    Ok(())
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
fn check_pin_map(pins: &chip::Cc1101Pins<'_>) -> Result<(), StartError> {
    for (claimed, documented) in [
        (pins.sck.number(), chip::pins::SCK),
        (pins.mosi.number(), chip::pins::MOSI),
        (pins.miso.number(), chip::pins::MISO),
        (pins.csn.number(), chip::pins::CSN),
        (pins.gdo0_tx.number(), chip::pins::GDO0_TX),
        (pins.gdo2_rx.number(), chip::pins::GDO2_RX),
    ] {
        if claimed != documented {
            return Err(StartError::PinMap {
                claimed,
                documented,
            });
        }
    }
    Ok(())
}
