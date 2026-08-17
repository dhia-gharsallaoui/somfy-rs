//! The controller: the Embassy tasks, four static channels, and the hardware
//! they own.
//!
//! Boot brings up the flash-backed rolling-code store, the CC1101, and both RMT
//! channels, then hands them to the two loops `somfy-tasks` defines and gets out
//! of the way. Everything either loop *does* is over there, host-tested; this
//! file is wiring, and it is deliberately the only place where an `esp-hal`
//! peripheral and a task body meet.
//!
//! ## The network is optional, and the wiring is what makes it so
//!
//! Wi-Fi and the broker session are brought up **after** the radio and state
//! tasks are spawned, by [`start_network`] and [`start_mqtt`], neither of which
//! returns anything. There is no path on which a missing network, a wrong
//! passphrase, an unreachable broker or an unreadable configuration region
//! stops the controller — not because that path is avoided, but because those
//! two functions give their caller nothing to propagate. A board with nothing
//! provisioned at all is the ordinary state of a freshly flashed device, and it
//! receives and decodes exactly as it always did. See [`net`]'s and `mqtt`'s
//! module docs for the other things that keep the halves apart.
//!
//! ## This image transmits only what a broker tells it to
//!
//! The state task transmits only what it is commanded to, and the command
//! channel's one producer is the MQTT session. So what this image can move is
//! exactly what the `shades` region names: a board with that region erased has
//! an empty registry, no entity to command, and keys the transmitter never,
//! which is the ordinary state of a freshly flashed device.
//!
//! Two things follow, and both are deliberate. A shade cannot be commanded
//! before somebody has flashed a shade table with `provision_shades` — the
//! firmware has no path that writes one. And a shade that *is* provisioned
//! still cannot transmit until its rolling code exists in the store, which
//! [`provision_shades`] does once, from the record, and never again: a code
//! re-seeded at every boot would walk backwards and desync the motor. See
//! `somfy_store::seed_if_absent`.
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

// For `esp-radio` and for nothing this firmware writes. Its `StationConfig`
// holds an `alloc::string::String`, so the one allocation on our side of the
// boundary is the passphrase being handed to the driver — see `net::start`.
// `crates/firmware/.cargo/config.toml` explains why `alloc` is in `build-std`
// at all, and `heap` explains what the heap is for.
extern crate alloc;

mod chip;
mod config;
mod heap;
mod inventory;
mod mqtt;
mod net;
mod radio;
mod shades;
mod store;
mod tasks;

use embassy_executor::{SpawnError, Spawner};
use embassy_futures::yield_now;
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

use config::ConfigStore;
use heapless::Vec;
use inventory::Inventory;
use radio::air::{Air, AirError};
use radio::rmt_rx::{rx_channel_config, RmtPulseSource};
use radio::rmt_tx::{tx_channel_config, RmtTx};
use shades::ShadeStore;
use somfy_config::{MqttSettings, Namespaces, StoredShade, WifiCredentials};
use somfy_domain::{Registry, RemoteIdentity, MAX_SHADES};
use somfy_rts::RollingCode;
use somfy_store::{seed_if_absent, RegionState, Seeded};
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
/// **This was 8,192, and it was wrong by a factor of six.** It was reasoned from
/// `RmtTx::transmit_frame`, which needs about 6.5 KB and is the deepest thing on
/// the *frame* path — but the frame path is nowhere near the deepest thing this
/// firmware does. The boot path is, and it is nearly six times larger.
///
/// The old figure never fired, and that is the point rather than a defence: at
/// the 56 KB heap every chip in the matrix had at least 71,004 bytes of stack,
/// so a check set at 8,192 passed on all of them for a reason unrelated to
/// whether it was right. It would have gone on passing until a heap change took
/// the stack below the real requirement — which is exactly what
/// [`heap::RADIO_HEAP_BYTES`] now does deliberately, and why this had to be
/// derived before that could be.
///
/// ## Where the number comes from
///
/// Every frame below is read out of the linked ELF, from the `.stack_sizes`
/// section `-Zemit-stack-sizes` emits, and they are summed along one call chain
/// rather than guessed at. On Xtensa and RISC-V alike a frame is allocated whole
/// in the prologue, so a chain's cost is the sum of its frames.
///
/// The deepest chain is the boot path, and it is one straight line — no branch,
/// no recursion, nothing conditional:
///
/// | | ESP32 | ESP32-S3 | ESP32-C3 |
/// |---|---|---|---|
/// | executor poll above the main task | 152 | 152 | 152 |
/// | `TaskStorage<__embassy_main_task>::poll` | 12,272 | 12,272 | 16 |
/// | [`start`] | 13,952 | 13,744 | *inlined* |
/// | `entry`'s body, where `start` inlined into it | — | — | 16,688 |
/// | [`tasks::state`], building the task token | 7,184 | 7,184 | 7,168 |
/// | `UninitCell::write`, moving the future into its static | 14,320 | 14,320 | 14,320 |
/// | **total** | **47,880** | **47,672** | **38,344** |
///
/// The last row is the state task's 14 KB future being materialised on the stack
/// and then copied into the static `#[embassy_executor::task]` declares for it.
/// It is the largest frame in two of the three images, it is unavoidable at this
/// Embassy version, and it lands *below* `start`'s own 13.7 KB frame rather than
/// after it has been given back.
///
/// **The ESP32-S3 column was checked against the silicon.** A throwaway build
/// (not committed, like the `rx_raw` diagnostics before it) painted the free
/// stack with a known word at the top of `entry` and read back how far down the
/// pattern had been destroyed, on a board associating with a real access point
/// and announcing to a real broker: **47,672 bytes**, against the 47,672 the
/// table computes. Not close — equal. The mark is reached during boot and never
/// moves again, which is what makes the boot path, and not the frame path, the
/// thing this constant has to cover. The other two columns are computed and not
/// measured.
///
/// ## What is added on top of it
///
/// **1,712 bytes of interrupt frames.** An interrupt lands on whatever stack was
/// running, and here that is this one. `xtensa-lx-rt` allocates `XT_STK_FRMSZ` =
/// 256 bytes per entry (`xtensa-lx-rt-0.22.0/src/exception/asm.rs:81`) and then
/// calls a handler with its own frame; the five entries that can nest —
/// `__user_exception`, `__level_1`, `__level_2`, `__level_3` and
/// `__default_double_exception` — cost 5 × 256 plus 432 of handler frames on the
/// worst chip. All five stacked at once is not a scenario anyone has seen; it is
/// the bound.
///
/// So **47,880 + 1,712 = 49,592**, and that is this constant. It is the worst
/// chip's figure rather than a per-chip one, because a boot check that differs
/// per chip is three numbers to keep true instead of one — and the worst chip is
/// the ESP32, which is also the one whose interrupt frames are counted above.
/// The ESP32-C3 is RISC-V and enters interrupts differently, but it arrives with
/// 9,536 bytes of slack against the ESP32's boot path, which is more than the
/// whole Xtensa allowance.
///
/// ## What it does *not* cover, said plainly
///
/// The bodies interrupt handlers dispatch into. `esp_radio`'s `Handler::dispatch`
/// calls straight into the closed Wi-Fi driver from interrupt context
/// (`esp-radio-0.18.0/src/interrupt_dispatch.rs:24`), so those frames land here,
/// and neither that blob nor masked ROM carries stack-size metadata — no sum
/// over them is available. The hardware measurement above is what stands in for
/// it, since it was taken with the driver's interrupts live, and
/// [`heap::STACK_BUDGET_BYTES`] carries the margin that covers being wrong about
/// it.
///
/// Checked at run time rather than asserted at compile time because it cannot be
/// a constant: esp-hal's linker script gives the stack **whatever DRAM is left
/// after the statics**, so the figure moves every time a static is added — and
/// now that [`heap::RADIO_HEAP_BYTES`] is the largest static in the image, it
/// moves every time that changes too. When it fires it names both numbers, which
/// is worth more than a corrupted pulse train.
const REQUIRED_STACK_BYTES: usize = 49_592;

/// How long the panic handler waits for the serial line to drain before it
/// resets the board. See the handler for why this is not optional.
const PANIC_DRAIN_MS: u32 = 100;

/// Transmissions from the state task to the radio task.
///
/// The producer end is only ever reachable as a `somfy_store::TransmitQueue`,
/// which is what makes "the rolling code is in flash before the frame is on the
/// air" a property of the type system rather than of review. See
/// `somfy_tasks::queue`.
static TRANSMIT: TransmitChannel<Mutex> = TransmitChannel::new();

/// Decoded frames from the radio task to the state task.
static FRAMES: FrameChannel<Mutex> = FrameChannel::new();

/// Commands into the state task. The MQTT session is its one producer, and it
/// uses `try_send`: a full queue drops the newest command rather than parking
/// the sender, because a queue of shade commands is a queue of intentions and
/// acting on a stale one is worse than dropping it.
static COMMANDS: CommandChannel<Mutex> = CommandChannel::new();

/// State deltas out of the state task. The MQTT session subscribes; publishing
/// with no subscriber discards immediately, which is what a board with no
/// broker provisioned does.
static DELTAS: DeltaChannel<Mutex> = DeltaChannel::new();

/// Report the panic, then **reboot** — which is the degradable answer, and it
/// is the network that made it necessary.
///
/// Halting was right while this image was radio and flash only: every panic
/// reachable then was this project's own code failing deterministically, a
/// board frozen with its message on the serial line is the best thing to hand
/// a person debugging it, and rebooting into the same panic would only scroll
/// it away.
///
/// Wi-Fi changes that, because it adds panics this firmware neither writes nor
/// can catch. `esp-radio` panics on a Wi-Fi status code it does not recognise,
/// and its event-channel subscriber slots are `expect`ed rather than handled;
/// an allocation that fails anywhere reaches `handle_alloc_error`, which
/// panics too. None of those is reachable through [`net::start`]'s `Result`,
/// so with a halting handler a transient failure in a *degradable* service
/// would take the radio off the air until somebody physically power-cycled the
/// board. That is precisely the outcome spec R9 exists to forbid, and no
/// amount of care in this crate can close it, because the panic is not in this
/// crate.
///
/// So: print first — the message still reaches an attached monitor — then
/// reset. A board that reboots receives again seconds later. A board that
/// halts never does. Where the panic is deterministic at boot the cost is a
/// reboot loop, which is noisy, visible on the serial line, and still strictly
/// better than a dead radio.
///
/// **The bring-up harnesses deliberately keep the halting handler.** They are
/// run with a person watching the serial line, they contain no network, and
/// there the frozen state is worth more than the recovery.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    // **Not optional, and measured.** `esp_println` spins until the UART has
    // room for each byte, not until the line has left the shift register, so
    // resetting immediately truncates the message — observed on an ESP32-S3,
    // where a deliberate panic produced a clean reboot with no `PANIC:` line
    // anywhere on the serial output. A panic you cannot read is most of the
    // cost of a panic. 100 ms is roughly three times what a long message needs
    // at 115200 baud, and it doubles as a floor on how fast a deterministic
    // panic can cycle the board.
    Delay::new().delay_millis(PANIC_DRAIN_MS);
    esp_hal::system::software_reset()
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

/// What [`start`] leaves for the network, once the radio is genuinely running.
///
/// It exists only so that a `yield_now` can sit between the two — see [`entry`]
/// — which is a real ordering requirement and not a stylistic one.
struct Pending {
    wifi: esp_hal::peripherals::WIFI<'static>,
    credentials: Option<WifiCredentials>,
    /// The broker to talk to, if one is provisioned. `None` is a supported
    /// configuration, not a failure: the controller receives and decodes with
    /// no broker at all.
    broker: Option<MqttSettings>,
    /// Namespace pairs the ring shows this device has published under before
    /// and is not publishing under now. Their retained configs have to be
    /// cleared before the current ones go out — see spec R5, and
    /// `somfy_mqtt::reconfigure`, which is the only way to ask for the two in
    /// that order.
    superseded: Vec<Namespaces, { config::MAX_SUPERSEDED }>,
    /// The shades to announce, copied before the state task owns the registry.
    inventory: Inventory,
    /// What the rolling-code region held at boot.
    ///
    /// Carried to the broker session so that `damaged` — the single most
    /// operationally important thing this device knows about itself — reaches
    /// Home Assistant rather than only a serial cable. It is a snapshot, and it
    /// has to be: the store belongs to the state task from the moment it is
    /// handed over, and re-surveying it from the network path would cross the
    /// boundary that keeps a broker from being able to affect radio control.
    survey: store::Survey,
}

#[esp_rtos::main]
async fn entry(spawner: Spawner) {
    let pending = match start(spawner) {
        Ok(pending) => pending,
        Err(error) => {
            // A failure leaves nothing spawned and this message is the whole
            // report. No network is attempted, because there is no radio for
            // it to be independent of.
            esp_println::println!("controller: failed to start: {:?}", error);
            return;
        }
    };

    // **The one await that makes "the radio is already receiving" true.**
    //
    // `#[esp_rtos::main]` makes this function an Embassy task like any other,
    // and `Spawner::spawn` only enqueues — a spawned task is not polled until
    // whatever is running yields. So without this line every spawn below would
    // be a promise rather than a fact, and the whole of Wi-Fi bring-up
    // (`esp_radio::wifi::new` powers the PHY, initialises the blob and creates
    // the driver's threads) would run before the radio task's *first* poll.
    // The RMT receive channel is armed inside `RadioLoop::step`, so the
    // controller would be deaf for all of it — with `air.listen()` already
    // having put the CC1101 into receive, which is exactly the combination
    // that looks like working hardware and hears nothing.
    //
    // One yield is enough: the radio and state tasks were enqueued before this
    // task re-enqueued itself, so both are polled — and the receiver armed —
    // before control returns here.
    yield_now().await;

    start_network(spawner, pending);
    heap::report("controller started");
    esp_println::println!("controller: running");
    // Returning is correct: the executor outlives this function and keeps
    // polling the tasks that were spawned.
}

fn start(spawner: Spawner) -> Result<Pending, StartError> {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Before anything else can allocate. `esp-rtos` is built with `alloc`
    // support so that the Wi-Fi driver's task stacks have somewhere to come
    // from, and that is true on a board with no credentials as much as on one
    // with them. See `heap::RADIO_HEAP_BYTES` for the size and where it came
    // from — it is a static, so it comes out of the same DRAM as the stack
    // `check_stack_headroom` measures below.
    heap::install_for_radio();
    // After the heap is installed and before the stack check below: a chip
    // can pass the stack check and still have too little heap left for the
    // radio, and those are two different failures with two different
    // symptoms. See `heap::warn_if_undersized`.
    heap::warn_if_undersized();

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

    // One flash peripheral, two regions, read one after the other. The config
    // store is mounted through a reborrow and dropped before the rolling-code
    // store takes the singleton for good — the configuration is read once at
    // boot and never again, so nothing needs to hold it open, and the store
    // that *is* held open is the one a running controller writes to.
    let mut flash = peripherals.FLASH;
    let (credentials, broker, superseded) = report_config(FlashStorage::new(flash.reborrow()));

    // Read **before** the rolling-code store takes the flash singleton for
    // good, because that store owns it for the life of the program and this
    // region cannot be reached afterwards. The shades are therefore carried
    // across the two calls below — 2,312 bytes for a full table, which is the
    // smallest form that survives; see `provision_shades` for what is not held.
    let shades = report_shades(FlashStorage::new(flash.reborrow()));

    // Mounted here rather than inside the state task: `mount` wants roughly
    // 5 KB of stack for the partition table and `esp-storage`'s sector buffer,
    // and doing it before anything is spawned keeps that spike away from the
    // radio task's own stack needs. Every later operation is far cheaper.
    let mut store = FlashStore::mount(FlashStorage::new(flash)).map_err(StartError::Store)?;
    let survey = report_store(&mut store)?;

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
    let device =
        ExclusiveDevice::new(bus, chip_select, Delay::new()).map_err(|_| StartError::ChipSelect)?;

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

    // The controller, and then both halves of provisioning in one place and in
    // this order: a shade reaches the registry, and its rolling code reaches
    // the store only if the store has none.
    //
    // **Before anything is spawned**, because a commit runs with interrupts
    // disabled on this core and the receiver is not armed yet, so nothing on
    // the air can be missed by it — and **after** the survey, which is what
    // tells the seeding whether an empty read can be believed. It is also after
    // `FlashStore::mount`, deliberately: a `StateMachine` is 8,016 bytes and
    // mounting wants about 5 KB of stack, and on the tightest chip this builds
    // for the whole stack is 14,588.
    let mut machine = StateMachine::new(TxProfile::default());
    provision_shades(machine.registry_mut(), &mut store, shades, survey.damaged);

    // Copied **here**, before the state task takes ownership of the machine.
    // The MQTT session works from this copy rather than from the registry, so
    // there is no shared state between the broker and the radio at all — see
    // `mqtt`'s module docs for the other three things that keep them apart.
    let inventory = Inventory::snapshot(machine.registry());
    if inventory.len() == 0 {
        // Said out loud, because a controller with nothing provisioned and a
        // broken one look identical from the serial line and from Home
        // Assistant, where both announce availability and no entity.
        esp_println::println!(
            "controller: no shades provisioned — receiving and tracking only, and \
             nothing can be commanded until a shade table is flashed. Build one with \
             `cargo run -p somfy-config --example provision_shades`."
        );
    } else {
        esp_println::println!("controller: {} shades provisioned", inventory.len());
    }

    // This controller's own virtual-remote identity, printed so an operator can
    // check the provisioning tool against the board rather than against a
    // derivation done twice. It is a *diagnostic*: nothing here allocates from
    // it, because the shade table is read-only to this firmware — the addresses
    // in it were chosen when the table was built.
    //
    // What it does NOT prove is that two boards differ. The derivation folds 24
    // bits of MAC into 20, so two arbitrary boards share a base about one time
    // in a million — a coincidence, not the OUI defect, and the remedy is a
    // hand-picked address for one of them rather than a bug report. See
    // `somfy_domain::RemoteIdentity::from_mac`.
    esp_println::println!(
        "pairing: this controller's remote addresses start at {:#08X}",
        RemoteIdentity::from_mac(base_mac()).base(),
    );

    // Both tokens first, then both spawns. `#[task]` hands back a token or a
    // `SpawnError` and `Spawner::spawn` is infallible once it has one, so
    // spawning as each token is obtained would leave a half-started
    // controller on a failure — a live radio task with nothing draining
    // `FRAMES`, which fills and then reports a dropped frame for every
    // reception, forever. Taking both first makes that unrepresentable.
    let radio = tasks::radio(RadioLoop::new(
        RmtPulseSource::new(receive),
        air,
        TRANSMIT.requests(),
        FRAMES.sender(),
    ))
    .map_err(StartError::Spawn)?;
    let state = tasks::state(
        machine,
        store,
        TRANSMIT.queue(),
        FRAMES.receiver(),
        COMMANDS.receiver(),
        DELTAS.immediate_publisher(),
    )
    .map_err(StartError::Spawn)?;

    spawner.spawn(radio);
    spawner.spawn(state);

    // The network is brought up by the caller, **after** it has yielded to the
    // tasks just spawned. That ordering is the degradability requirement
    // expressed as control flow, and it is why this returns the pieces instead
    // of finishing the job: see [`entry`] for what the yield buys, and `net`'s
    // module docs for the other three things that keep the two halves apart.
    Ok(Pending {
        wifi: peripherals.WIFI,
        credentials,
        broker,
        superseded,
        inventory,
        survey,
    })
}

/// Read the persisted configuration and say what was found.
///
/// Returns `None` — meaning "run without a network" — for every outcome that
/// is not a usable credential, and prints which one it was. That is a
/// deliberate difference from [`FlashStore`], which refuses to run at all on a
/// region it cannot read: losing a rolling code costs a re-pairing procedure
/// at every shade, while losing this costs a Wi-Fi connection and one
/// re-provisioning step. `config`'s module docs carry the argument.
///
/// **A board with nothing provisioned is the ordinary case**, not an error
/// path. It is what a freshly flashed device looks like, and it is also the
/// cleanest demonstration that the network is optional: such a board still
/// receives and decodes.
#[allow(
    clippy::type_complexity,
    reason = "three independent halves of one \
    read, and a struct for them would be a type used exactly once between two \
    adjacent lines"
)]
fn report_config(
    flash: FlashStorage<'_>,
) -> (
    Option<WifiCredentials>,
    Option<MqttSettings>,
    Vec<Namespaces, { config::MAX_SUPERSEDED }>,
) {
    let nothing = || (None, None, Vec::new());

    let mut store = match ConfigStore::mount(flash) {
        Ok(store) => store,
        Err(error) => {
            esp_println::println!("config: region unavailable ({:?})", error);
            return nothing();
        }
    };

    let (base, slots, slot_len) = store.geometry();
    esp_println::println!(
        "config: partition '{}' at {:#010X}, {} slots of {} bytes",
        config::PARTITION_LABEL,
        base,
        slots,
        slot_len,
    );

    let (record, survey) = match store.load() {
        Ok(found) => found,
        Err(error) => {
            esp_println::println!("config: unreadable ({:?})", error);
            return nothing();
        }
    };
    esp_println::println!(
        "config: survey slots={} valid={} blank={} damaged={} newest_seq={:?}",
        survey.slots,
        survey.valid,
        survey.blank,
        survey.damaged,
        survey.newest_seq,
    );
    for stale in &survey.superseded {
        // Printed because it is about to change what the device publishes: the
        // retained configs under these namespaces are cleared before the
        // current ones go out. See spec R5.
        esp_println::println!(
            "config: superseded namespaces discovery_prefix='{}' state_root='{}' \
             — their retained topics will be cleared on the next fresh broker session",
            stale.discovery_prefix(),
            stale.state_root(),
        );
    }
    if survey.superseded_truncated {
        esp_println::println!(
            "config: more superseded namespaces than this build tracks ({}); \
             the oldest will not be cleared",
            config::MAX_SUPERSEDED,
        );
    }

    // `Debug` on both halves redacts its secret, and only the SSID and the
    // broker's address are printed here in any case. The SSID is broadcast by
    // the access point several times a second; neither secret leaves flash
    // except into the driver or the broker.
    let Some(record) = record else {
        return (None, None, survey.superseded);
    };
    if let Some(mqtt) = &record.mqtt {
        esp_println::println!(
            "config: broker {}:{} ({}), discovery_prefix='{}' state_root='{}'",
            mqtt.address(),
            mqtt.port(),
            if mqtt.is_anonymous() {
                "anonymous"
            } else {
                "authenticated"
            },
            mqtt.discovery_prefix(),
            mqtt.state_root(),
        );
    }
    (record.wifi, record.mqtt, survey.superseded)
}

/// The shades a boot found, in the order their registry ids will follow.
///
/// This is what is carried from [`report_shades`] across `FlashStore::mount`
/// to [`provision_shades`], and it is the smallest form that can be: 2,312
/// bytes for a full table. What is deliberately *not* carried is a decoded
/// `somfy_config::ShadeRecord` per slot — see `shades::ShadeStore::load_with`.
type Shades = Vec<StoredShade, MAX_SHADES>;

/// Read the persisted shade table and say what was found.
///
/// Answers "no shades" for every outcome that is not a readable table, and
/// prints which one it was — the same posture as [`report_config`], and for the
/// same reason: a board with nothing provisioned is the ordinary state of a
/// freshly flashed device, and it still receives and decodes.
///
/// Nothing is placed anywhere here. The registry does not exist yet, and that
/// is the point: reading this region has to happen before the rolling-code
/// store takes the flash, and a `StateMachine` alive across that mount is 8,016
/// bytes standing next to a 5 KB spike.
fn report_shades(flash: FlashStorage<'_>) -> Shades {
    let mut shades = Shades::new();

    let mut store = match ShadeStore::mount(flash) {
        Ok(store) => store,
        Err(error) => {
            esp_println::println!(
                "shades: region unavailable ({:?}) — no shades. A board flashed with an \
                 older partition table has no '{}' partition; reflash it with this \
                 crate's partitions.csv.",
                error,
                shades::PARTITION_LABEL,
            );
            return shades;
        }
    };

    let (base, slots, slot_len) = store.geometry();
    esp_println::println!(
        "shades: partition '{}' at {:#010X}, {} slots of {} bytes",
        shades::PARTITION_LABEL,
        base,
        slots,
        slot_len,
    );

    // The closure is where the shades are collected. `push` cannot fail — the
    // record's own capacity is `MAX_SHADES`, the same bound as this vector —
    // and a failure is ignored rather than `expect`ed because a panic here
    // would take the radio off the air over a shade table.
    let survey = match store.load_with(|_, shade| {
        let _ = shades.push(shade);
    }) {
        Ok(survey) => survey,
        Err(error) => {
            esp_println::println!("shades: unreadable ({:?}) — no shades", error);
            return Shades::new();
        }
    };
    esp_println::println!(
        "shades: survey slots={} valid={} blank={} damaged={} newest_seq={:?}",
        survey.slots,
        survey.valid,
        survey.blank,
        survey.damaged,
        survey.newest_seq,
    );
    if let Some(error) = survey.first_error {
        // Printed with the entry index it carries, because that is the shade to
        // correct — a bare damaged count leaves an operator guessing which one
        // the record refused. A refused table places **no** shades at all, on
        // purpose: see `somfy_config::ShadeRecord::for_each`.
        esp_println::println!(
            "shades: a table did not decode ({:?}). If it was the newest one, no shade \
             from it was loaded — re-provision it.",
            error,
        );
    }
    if shades.is_empty() {
        esp_println::println!(
            "shades: none provisioned — the controller receives, decodes and tracks, \
             and has nothing to command"
        );
    }
    shades
}

/// Put every shade in the registry, and give each one a rolling code if and
/// only if the store has none for it.
///
/// **The seeding rule is the one that costs a re-pairing when it is broken.**
/// The record is read at every boot and names a starting code; writing it every
/// boot would move the counter backwards, and a motor rejects any code at or
/// below the last one it accepted. `somfy_store::seed_if_absent` is where that
/// is enforced — the commit is inside the branch where the read found nothing —
/// and this function only reports what it decided.
///
/// A shade the registry refuses is reported and skipped rather than stopping
/// the rest. `add_shade` refuses a duplicate address and a full registry, and
/// the record's own decode has already refused both — so reaching either here
/// means the two disagree, which is worth a line rather than a silent gap in
/// the ids.
/// The factory MAC as an array.
///
/// `esp-hal` wraps a `[u8; 6]` but hands it back as a `&[u8]`, and this
/// firmware must not panic in `main` — a boot loop over a diagnostic line is
/// worse than the diagnostic is worth. So the copy is a `zip` rather than a
/// `copy_from_slice`, which panics on a length this can never actually be
/// given.
fn base_mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    let source = esp_hal::efuse::base_mac_address();
    for (slot, byte) in mac.iter_mut().zip(source.as_bytes()) {
        *slot = *byte;
    }
    mac
}

fn provision_shades(
    registry: &mut Registry,
    store: &mut FlashStore<'_>,
    shades: Shades,
    damaged: usize,
) {
    // The rolling-code region's own state, as the survey found it a moment ago.
    // A missing code in a region with damaged slots may be a lost code rather
    // than a new shade, so it is refused rather than planted.
    let region = RegionState::from_damaged(damaged);

    for (index, shade) in shades.into_iter().enumerate() {
        let address = shade.config.address;
        let id = match registry.add_shade(shade.config) {
            Ok(id) => id,
            Err(error) => {
                esp_println::println!(
                    "shades: entry {} at {:#08X} refused by the registry ({:?}) — it is not \
                     announced and cannot be commanded",
                    index,
                    address,
                    error,
                );
                continue;
            }
        };
        esp_println::println!(
            "shades: ShadeId({}) address {:#08X} — entry {}",
            id.0,
            address,
            index,
        );
        seed(store, address, shade.initial_code, region);
    }
}

/// Give one address its first rolling code, and say which of the three things
/// happened.
fn seed(store: &mut FlashStore<'_>, address: u32, code: RollingCode, region: RegionState) {
    match seed_if_absent(store, address, code, region) {
        Ok(Seeded::Kept(stored)) => esp_println::println!(
            "shades: {:#08X} keeps its stored rolling code {} — the provisioned starting \
             value {} is ignored, which is what every boot after the first looks like",
            address,
            stored.0,
            code.0,
        ),
        Ok(Seeded::Planted(planted)) => esp_println::println!(
            "shades: {:#08X} had no stored rolling code; seeded {} from the shade record. \
             This happens once.",
            address,
            planted.0,
        ),
        Ok(Seeded::Refused { damaged }) => esp_println::println!(
            "shades: {:#08X} has no stored rolling code and the rolling-code region reports \
             {} damaged slot(s) — NOT seeding, because an empty read there may be a lost \
             code rather than a new shade. This shade will refuse to transmit until the \
             region is repaired or deliberately erased.",
            address,
            damaged,
        ),
        Err(error) => esp_println::println!(
            "shades: {:#08X} could not be seeded ({:?}) — it will refuse to transmit",
            address,
            error,
        ),
    }
}

/// Start the network if there is one to start, and never fail.
///
/// No `Result`, on purpose. A caller cannot propagate what it is not given, so
/// the "network failure stops the controller" path is not something to avoid
/// writing — it is not expressible from here.
fn start_network(spawner: Spawner, pending: Pending) {
    let Some(credentials) = pending.credentials else {
        esp_println::println!(
            "network: no credentials provisioned — running radio-only. \
             This board still receives and decodes; see docs/hardware-checklist.md \
             to provision one."
        );
        return;
    };
    let stack = match net::start(spawner, pending.wifi, &credentials) {
        Ok(stack) => stack,
        Err(error) => {
            esp_println::println!(
                "network: failed to start ({:?}) — running radio-only, which is unaffected",
                error,
            );
            return;
        }
    };
    start_mqtt(
        spawner,
        stack,
        pending.broker,
        pending.superseded,
        pending.inventory,
        pending.survey,
    );
}

/// Start the broker session if one is configured, and never fail.
///
/// Same shape and same reason as [`start_network`]: no `Result`, so the "MQTT
/// failure stops the controller" path is not expressible here. Spec R9 is
/// explicit that a broker which is down, unreachable, or rejecting credentials
/// must not affect radio control, and a board with no broker at all is the
/// ordinary state of one provisioned before a broker existed.
fn start_mqtt(
    spawner: Spawner,
    stack: embassy_net::Stack<'static>,
    broker: Option<MqttSettings>,
    superseded: Vec<Namespaces, { config::MAX_SUPERSEDED }>,
    inventory: Inventory,
    survey: store::Survey,
) {
    let Some(settings) = broker else {
        esp_println::println!(
            "mqtt: no broker provisioned — the controller runs without one. \
             It still receives, decodes and tracks."
        );
        // **A gap R5 names and this configuration model cannot close.** R5 asks
        // that disabling discovery clear every entity it owns. Clearing the
        // broker is not the same act — it removes the only route to the
        // retained topics, so there is nothing to connect to and nothing this
        // device can do about the orphans it left. There is no separate
        // "discovery off" switch to attach the obligation to instead.
        //
        // What is left is to name them, because a silent orphan is the thing
        // the requirements were written from. Anyone reading this line has the
        // two commands that clear it by hand.
        for stale in &superseded {
            esp_println::println!(
                "mqtt: retained topics under discovery_prefix='{}' state_root='{}' \
                 CANNOT be cleared without a broker — they will outlive this device. \
                 Clear them with: mosquitto_sub -t '{}/+/+/+/config' -v --retained-only, \
                 then mosquitto_pub -r -n -t <each topic>; and likewise under '{}/#'.",
                stale.discovery_prefix(),
                stale.state_root(),
                stale.discovery_prefix(),
                stale.state_root(),
            );
        }
        return;
    };

    let deltas = match DELTAS.subscriber() {
        Ok(deltas) => deltas,
        Err(error) => {
            esp_println::println!("mqtt: no delta subscription available ({:?})", error);
            return;
        }
    };

    if let Err(error) = mqtt::start(
        spawner,
        stack,
        settings,
        superseded,
        inventory,
        survey,
        COMMANDS.sender(),
        deltas,
    ) {
        esp_println::println!(
            "mqtt: failed to start ({:?}) — running without a broker, \
             which leaves the radio unaffected",
            error,
        );
    }
}

/// Print what the rolling-code region holds before anything writes to it.
///
/// The difference between "this device has never stored a code" and "this
/// device's codes are gone" is exactly what
/// `docs/specs/2026-08-15-config-integrity-requirements.md` R1 requires be
/// distinguishable, and no amount of "the store mounted OK" can tell you which
/// one this is. `damaged` above zero on a device nobody power-cut deserves a
/// look.
///
/// The survey is returned as well as printed, because "deserves a look" is a
/// weak guarantee when the only place to look is a serial console. The broker
/// session publishes `damaged` as a diagnostic sensor, so the same fact reaches
/// Home Assistant.
fn report_store(store: &mut FlashStore<'_>) -> Result<store::Survey, StartError> {
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
    Ok(survey)
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
