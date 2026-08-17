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
//! A shade that is in the table still cannot transmit until its rolling code
//! exists in the store, which [`provision_shades`] does once, from the record,
//! and never again: a code re-seeded at every boot would walk backwards and
//! desync the motor. See `somfy_store::seed_if_absent`.
//!
//! **What has changed is where the table comes from.** It used to be flashed by
//! `provision_shades` and nothing else — the firmware had no path that wrote
//! one — and now it does: `shades::ShadeStore::store`, driven by the state task
//! on a debounce. `provision_shades` remains how an installation is imported
//! from another controller, and it is still the only way to get a table onto a
//! board that has never had one; adding, removing and linking are the device's
//! own from here.
//!
//! ## What a boot proves
//!
//! The lines this prints are the ones that cannot be established anywhere else:
//! the store's survey (a device that has never stored a code versus one whose
//! codes are gone), how much stack the boot path actually spent against how much
//! it was budgeted, and every frame the receiver decodes. None of it is the
//! radio reporting on its own transmissions — a transmitter's account of itself
//! is wrong in the same way its output is.
//!
//! **The stack pair is two lines and the point is the gap between them.**
//! `stack: … available, … required` compares two written-down numbers and can
//! only catch a bad *division* of DRAM; `stack: … used at the deepest point of
//! boot` is the one that was measured, and it is the only thing here that can
//! catch the requirement itself having gone stale — which it had, by 23,688
//! bytes, and the symptom was a board that passed its own check and then wrote
//! through its stack guard. See [`report_stack_use`] and `heap`.

#![no_std]
#![no_main]
// An `#[embassy_executor::task]` allocates a static sized to one concrete
// future, so it cannot be generic — and `picoserve::Router::route` returns
// `Router<impl PathRouter>`, a type that exists and has no spelling. This
// feature is what lets `api::routes::App` give it one, and it is the only
// unstable language feature this crate uses. It is `picoserve`'s own documented
// pattern for the same reason; the alternative is boxing a trait object, which
// needs both an allocator on the request path and object safety that an
// `async fn` trait does not have.
#![feature(impl_trait_in_assoc_type)]
// `picoserve`'s router is a type per route, each wrapping the previous one as
// its fallback, so a ten-route table is a type ten layers deep and every trait
// obligation on it recurses to the bottom. The default limit of 128 is reached
// while resolving them, and the failure is `error: queries overflow the depth
// limit!` with no span — nothing points at the router. This is the fix, and it
// costs compile time rather than image size.
#![recursion_limit = "512"]

// For `esp-radio` and for nothing this firmware writes. Its `StationConfig`
// holds an `alloc::string::String`, so the one allocation on our side of the
// boundary is the passphrase being handed to the driver — see `net::start`.
// `crates/firmware/.cargo/config.toml` explains why `alloc` is in `build-std`
// at all, and `heap` explains what the heap is for.
extern crate alloc;

// The three transports, gated at their module declarations and nowhere else.
// That restriction is the point rather than a tidiness rule: a `#[cfg]` that
// had to appear inside `tasks`, `edits` or `somfy-domain` would mean transport
// logic had leaked into the core, and turning these off is how that gets
// noticed. `Cargo.toml` carries the argument in full.
#[cfg(feature = "http")]
mod api;
mod chip;
mod config;
mod edits;
mod heap;
#[cfg(feature = "mqtt")]
mod inventory;
#[cfg(feature = "mqtt")]
mod mqtt;
mod net;
mod radio;
mod rpc;
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
use edits::{AckChannel, EditChannel, EventChannel};
use heapless::Vec;
#[cfg(feature = "mqtt")]
use inventory::Inventory;
use radio::air::{Air, AirError};
use radio::rmt_rx::{rx_channel_config, RmtPulseSource};
use radio::rmt_tx::{tx_channel_config, RmtTx};
use shades::ShadeStore;
use somfy_config::{Announced, Catalog, LinkedRemote, StoredShade, WifiCredentials};
use somfy_config::{MqttSettings, Namespaces};
#[cfg(feature = "mqtt")]
use somfy_domain::ShadeId;
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

/// How long the panic handler waits for the serial line to drain before it
/// resets the board. See the handler for why this is not optional.
const PANIC_DRAIN_MS: u32 = 100;

/// The word painted over unused stack so the depth reached can be read back.
///
/// Any value that is not plausibly a pointer, a small integer or ASCII, so that
/// live data being mistaken for paint needs a coincidence rather than a common
/// case. A mistake in that direction under-reports by one word and then stops,
/// because the scan ends at the first word that differs.
const STACK_PAINT: u32 = 0xA5A5_5A5A;

/// How much stack immediately below the painting frame is left alone.
///
/// The paint runs with interrupts live, and an interrupt lands on this stack
/// just below the running frame — at most `heap::INTERRUPT_FRAMES_BYTES`,
/// 1,712, at the worst nesting this firmware bounds. 4 KiB is that rounded up
/// past another full nest, and it is the difference between an instrument and a
/// way to corrupt the frame that is running it.
///
/// The cost is that the shallowest 4 KiB is never painted, so [`stack_used`]
/// reports at least this much even on a boot that used nothing. That is the
/// harmless direction and the shallow end is the one nothing is ever near.
const PAINT_HEADROOM_BYTES: usize = 4 * 1024;

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

/// Changes to the shade table, into the state task.
///
/// The producer is whatever the device exposes to a person. There is none in
/// this image yet — the API surface is a separate task — so today this channel
/// exists to be the seam rather than to carry traffic, and the state task is
/// the only thing that may touch the registry either way.
static EDITS: EditChannel = EditChannel::new();

/// What the state task did to the table, out to the broker session.
static SHADE_EVENTS: EventChannel = EventChannel::new();

/// What the broker session did about it, back to the state task.
///
/// The return leg is not bookkeeping: the persisted `announced` bit may only be
/// cleared once the tombstones have landed, or a power cut between the two
/// loses the only record that a removed shade's entities are still on the
/// broker. See `crate::edits`.
static SHADE_ACKS: AckChannel = AckChannel::new();

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
    /// The main stack is smaller than the deepest chain in this image needs.
    /// See [`heap::REQUIRED_STACK_BYTES`].
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
    /// Everything the broker session needs and nothing else does.
    ///
    /// One field rather than five, so that the whole of MQTT's presence in this
    /// struct is a single `#[cfg]` at a module boundary. That is the same rule
    /// the module declarations above follow, and it is what stops a build with
    /// no broker in it carrying five unused fields and their imports.
    #[cfg(feature = "mqtt")]
    broker: MqttBoot,
}

/// Everything the broker session is handed at boot.
///
/// Assembled here because boot is where it can be read — the store and the
/// registry both belong to the state task from the moment they are handed over
/// — and carried rather than re-read for exactly that reason.
#[cfg(feature = "mqtt")]
struct MqttBoot {
    /// The broker to talk to, if one is provisioned. `None` is a supported
    /// configuration, not a failure: the controller receives and decodes with
    /// no broker at all.
    settings: Option<MqttSettings>,
    /// Namespace pairs the ring shows this device has published under before
    /// and is not publishing under now. Their retained configs have to be
    /// cleared before the current ones go out — see spec R5, and
    /// `somfy_mqtt::reconfigure`, which is the only way to ask for the two in
    /// that order.
    superseded: Vec<Namespaces, { config::MAX_SUPERSEDED }>,
    /// The shades to announce, copied before the state task owns the registry.
    inventory: Inventory,
    /// Ids that were announced and no longer exist. Their retained entities are
    /// on the broker with nothing behind them, and this is the only thing that
    /// can name them — see `somfy_config::Catalog`.
    orphans: Vec<ShadeId, MAX_SHADES>,
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
    // **Before anything else, because everything else is what is being
    // measured.** This is the shallowest point the firmware ever reaches after
    // the executor starts, so painting from here covers every frame that
    // follows. See [`report_stack_use`] for what is done with it, and
    // `heap::REQUIRED_STACK_BYTES` for the constant it exists to keep honest.
    paint_stack();

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
    report_stack_use();
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

    // Mounted here rather than inside the state task: `mount` wants roughly
    // 5 KB of stack for the partition table and `esp-storage`'s sector buffer,
    // and doing it before anything is spawned keeps that spike away from the
    // radio task's own stack needs. Every later operation is far cheaper.
    //
    // **Before the shade table now, where it used to be after it.** The shade
    // region is written at runtime, so it can no longer read through a
    // temporary reborrow that ends at boot: it borrows the flash from this
    // store, which owns the peripheral for the life of the program. See
    // `FlashStore::with_flash`.
    let mut store = FlashStore::mount(FlashStorage::new(flash)).map_err(StartError::Store)?;
    let survey = report_store(&mut store)?;

    let (shade_store, shades) = report_shades(&mut store);

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
    let catalog = provision_shades(machine.registry_mut(), &mut store, shades, survey.damaged);

    // Copied **here**, before the state task takes ownership of the machine.
    // The MQTT session works from this copy rather than from the registry, so
    // there is no shared state between the broker and the radio at all — see
    // `mqtt`'s module docs for the other three things that keep them apart.
    #[cfg(feature = "mqtt")]
    let inventory = Inventory::snapshot(machine.registry());
    // Announced and gone: entities on the broker with nothing behind them. Read
    // here, before the state task takes the registry, because this is the last
    // moment both halves of the comparison are in one place — and the ids are
    // the only thing that can name what has to be cleared.
    #[cfg(feature = "mqtt")]
    let orphans: Vec<ShadeId, MAX_SHADES> = catalog.orphans(machine.registry()).collect();
    // Asked of the registry rather than of the broker session's snapshot of it.
    // A controller with no shades is a fact about the controller, and a build
    // with no broker in it still has to be able to say so.
    if machine.registry().shades().next().is_none() {
        // Said out loud, because a controller with nothing provisioned and a
        // broken one look identical from the serial line and from Home
        // Assistant, where both announce availability and no entity.
        esp_println::println!(
            "controller: no shades provisioned — receiving and tracking only, and \
             nothing can be commanded until a shade table is flashed. Build one with \
             `cargo run -p somfy-config --example provision_shades`."
        );
    } else {
        esp_println::println!(
            "controller: {} shades provisioned",
            machine.registry().shades().count(),
        );
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
        tasks::Table {
            shades: shade_store,
            catalog,
            identity: RemoteIdentity::from_mac(base_mac()),
            edits: EDITS.receiver(),
            acks: SHADE_ACKS.receiver(),
            events: SHADE_EVENTS.sender(),
        },
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
    // The configuration region holds the broker's settings whether or not this
    // build talks to one, and `report_config` prints them either way — an
    // operator inspecting a radio-only image still wants to see what is
    // provisioned. Dropping them here is what says "read, reported, and
    // deliberately unused" rather than leaving a warning to be silenced.
    #[cfg(not(feature = "mqtt"))]
    drop((broker, superseded, survey));

    Ok(Pending {
        wifi: peripherals.WIFI,
        credentials,
        #[cfg(feature = "mqtt")]
        broker: MqttBoot {
            settings: broker,
            superseded,
            inventory,
            orphans,
            survey,
        },
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

/// The shades a boot found, in the order their registry ids will follow, plus
/// the wall remotes linked to them and the set the last session announced.
///
/// This is what is carried from [`report_shades`] to [`provision_shades`], and
/// it is the smallest form that can be: 2,312 bytes for a full table. What is
/// deliberately *not* carried is a decoded `somfy_config::ShadeRecord` per slot
/// — see `shades::ShadeStore::load_with`.
struct Shades {
    shades: Vec<StoredShade, MAX_SHADES>,
    links: Vec<LinkedRemote, { somfy_config::MAX_LINKS }>,
    /// What the record said this device had already published entities for.
    /// Empty for a board with no readable table, which is the honest answer:
    /// nothing is known, so nothing is claimed.
    announced: Announced,
}

/// Read the persisted shade table and say what was found.
///
/// Answers "no shades" for every outcome that is not a readable table, and
/// prints which one it was — the same posture as [`report_config`], and for the
/// same reason: a board with nothing provisioned is the ordinary state of a
/// freshly flashed device, and it still receives and decodes.
///
/// Nothing is placed anywhere here. The registry does not exist yet, and that
/// is the point: a `StateMachine` alive across this read is 8,016 bytes
/// standing next to a 5 KB spike.
///
/// **The store is returned rather than dropped**, because this region is now
/// written at runtime: the state task keeps it, and borrows the flash from the
/// rolling-code store whenever it writes. Only the mount's stack cost is paid
/// here, which is why it is paid in `main`.
fn report_shades(store: &mut FlashStore<'static>) -> (Option<ShadeStore>, Shades) {
    let mut found = Shades {
        shades: Vec::new(),
        links: Vec::new(),
        announced: Announced::NONE,
    };

    // `None` rather than a refusal, for the reason `shades`' module docs give:
    // losing this region costs the ability to command shades until somebody
    // re-provisions, while the rolling-code store refuses on damage because
    // losing *that* costs a physical re-pairing at every motor. What is new is
    // that `None` now also means no shade can be added, which is worth saying.
    let mut shade_store = match store.with_flash(ShadeStore::mount) {
        Ok(shade_store) => shade_store,
        Err(error) => {
            esp_println::println!(
                "shades: region unavailable ({:?}) — no shades, and none can be added \
                 either. A board flashed with an older partition table has no '{}' \
                 partition; reflash it with this crate's partitions.csv.",
                error,
                shades::PARTITION_LABEL,
            );
            return (None, found);
        }
    };

    let (base, slots, slot_len) = shade_store.geometry();
    esp_println::println!(
        "shades: partition '{}' at {:#010X}, {} slots of {} bytes",
        shades::PARTITION_LABEL,
        base,
        slots,
        slot_len,
    );

    // The closures are where the table is collected. Neither `push` can fail —
    // the record's own capacities are these vectors' — and a failure is ignored
    // rather than `expect`ed because a panic here would take the radio off the
    // air over a shade table.
    let read = store.with_flash(|flash| {
        shade_store.load_with(
            flash,
            |_, shade| {
                let _ = found.shades.push(shade);
            },
            |link| {
                let _ = found.links.push(link);
            },
        )
    });
    let (survey, header) = match read {
        Ok(read) => read,
        Err(error) => {
            esp_println::println!("shades: unreadable ({:?}) — no shades", error);
            return (
                Some(shade_store),
                Shades {
                    shades: Vec::new(),
                    links: Vec::new(),
                    announced: Announced::NONE,
                },
            );
        }
    };
    if let Some(header) = header {
        found.announced = header.announced;
    }
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
    if found.shades.is_empty() {
        esp_println::println!(
            "shades: none provisioned — the controller receives, decodes and tracks, \
             and has nothing to command until one is added"
        );
    }
    (Some(shade_store), found)
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
    found: Shades,
    damaged: usize,
) -> Catalog {
    // The rolling-code region's own state, as the survey found it a moment ago.
    // A missing code in a region with damaged slots may be a lost code rather
    // than a new shade, so it is refused rather than planted.
    let region = RegionState::from_damaged(damaged);
    let mut catalog = Catalog::new();
    // Adopted before anything else, and never derived from the table: the whole
    // value of this set is the case where the two disagree, which is a shade
    // that was announced and has since been removed.
    catalog.adopt_announced(found.announced);

    for (index, shade) in found.shades.into_iter().enumerate() {
        let address = shade.config.address;
        // What the shade would be driven as, against what this controller
        // transmits. Said out loud because the alternative is a shade that
        // imports looking healthy and never moves — which is exactly what
        // storing the width and the protocol was for.
        if shade.config.frame_width != somfy_domain::FrameWidth::Bits56
            || shade.config.protocol != somfy_domain::RadioProtocol::Rts
        {
            esp_println::println!(
                "shades: entry {} at {:#08X} is a {:?} shade on {:?} — this controller \
                 transmits 56-bit RTS only, so it will accept commands and never move",
                index,
                address,
                shade.config.frame_width,
                shade.config.protocol,
            );
        }
        let seed_code = shade.initial_code;
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
        // The seed is remembered rather than re-derived, because the record is
        // the only place it exists: `seed_if_absent` ignores it from the second
        // boot onward, and a table written later has to carry it forward
        // unchanged or the boot after a lost rolling-code region would plant
        // whatever was invented here.
        catalog.place(id, seed_code);
        seed(store, address, seed_code, region);
    }

    // The wall remotes, after every shade, because a remote cannot be linked to
    // a shade that is not in the registry yet. **This is the only feedback path
    // this controller has**: RTS is one-way, so a shade whose remotes are
    // unknown decodes their frames, matches them against nothing, and lets its
    // position estimate drift with nothing to say why.
    let mut linked = 0usize;
    for link in &found.links {
        match registry.shade_mut(link.shade) {
            Some(shade) => match shade.link_remote(link.address) {
                Ok(()) => linked += 1,
                Err(error) => esp_println::println!(
                    "shades: the remote at {:#08X} could not be linked to ShadeId({}) ({:?})",
                    link.address,
                    link.shade.0,
                    error,
                ),
            },
            // Reachable only if the registry refused the shade above, which it
            // reported on its own line.
            None => esp_println::println!(
                "shades: the remote at {:#08X} names ShadeId({}), which is not in the registry",
                link.address,
                link.shade.0,
            ),
        }
    }
    if linked > 0 {
        esp_println::println!(
            "shades: {} wall remote(s) linked — their presses are what keeps a position \
             estimate honest",
            linked,
        );
    }

    // Announced but not present: entities on the broker with nothing behind
    // them. Named here because this is the last moment anything knows the id,
    // and cleared by the broker session, which then acknowledges it.
    let orphans = catalog.orphans(registry).count();
    if orphans > 0 {
        esp_println::println!(
            "shades: {} shade(s) were announced and no longer exist — their retained \
             entities will be cleared on the next broker session",
            orphans,
        );
    }
    catalog
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
///
/// # Why it must not be inlined
///
/// **`#[inline(never)]` here is worth 18,576 bytes of stack on the ESP32-S3, and
/// without it the default build does not boot.** The mechanism is worth stating
/// because it is invisible in the source and general:
///
/// This function and [`start`] are two sequential calls out of [`entry`]. Only
/// one of them runs at a time — `start` has returned before this begins — so
/// their stack costs look like they should overlap rather than add. Inlining
/// breaks that. An inlined callee's locals become slots in the *caller's* frame,
/// allocated in its prologue and live for as long as the caller is; so with this
/// function inlined into `entry`, the web server's bring-up — dominated by
/// `api::start` handing `BUFFERS.init` an `[Buffers; HTTP_TASKS]` by value,
/// 14,336 bytes — sat underneath the 48,992-byte call to `start` for the whole
/// of it. The two costs added instead of overlapping, the total reached 71,568
/// against 66,724 of stack, and the board wrote through esp-hal's stack guard
/// and rebooted, forever.
///
/// The tell was that enabling `http` deepened the *main task's own frame* by
/// 9,200 bytes, in a build where no HTTP code can have run yet: nothing had
/// connected, and the panic landed before Wi-Fi had even been asked to join.
///
/// Marked here rather than on `api::start` because the boundary that matters is
/// this one — everything the network bring-up allocates should live below
/// `entry`'s frame, not inside it, whichever of these functions grows next.
/// `heap::NETWORK_CHAIN_BYTES` is what this branch costs once separated, and
/// `heap::REQUIRED_STACK_BYTES` takes the larger of it and the boot path.
#[inline(never)]
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
    // **Before the broker, and independent of it.** A device with no broker
    // provisioned — the ordinary state of a freshly flashed board — still needs
    // its own UI, and that UI is how somebody provisions one. Starting it after
    // `start_mqtt` would work too; starting it first says that it does not
    // depend on anything MQTT does.
    #[cfg(feature = "http")]
    if let Err(error) = api::start(spawner, stack) {
        esp_println::println!(
            "api: failed to start ({:?}) — running without a web UI, which leaves the radio \
             and the broker unaffected",
            error,
        );
    }
    #[cfg(feature = "mqtt")]
    start_mqtt(spawner, stack, pending.broker);

    // A build with neither transport still brings the network up, and that is
    // deliberate rather than an oversight: DHCP is what turns "associated" into
    // "on the network", `net::address_watch` prints the address it was given,
    // and an operator commissioning a radio-only board needs both. Nothing
    // connects *through* the stack, so this is where that is said out loud.
    #[cfg(not(any(feature = "http", feature = "mqtt")))]
    let _ = stack;
}

/// Start the broker session if one is configured, and never fail.
///
/// Same shape and same reason as [`start_network`]: no `Result`, so the "MQTT
/// failure stops the controller" path is not expressible here. Spec R9 is
/// explicit that a broker which is down, unreachable, or rejecting credentials
/// must not affect radio control, and a board with no broker at all is the
/// ordinary state of one provisioned before a broker existed.
#[cfg(feature = "mqtt")]
fn start_mqtt(spawner: Spawner, stack: embassy_net::Stack<'static>, boot: MqttBoot) {
    let MqttBoot {
        settings,
        superseded,
        inventory,
        orphans,
        survey,
    } = boot;
    let Some(settings) = settings else {
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
        orphans,
        survey,
        COMMANDS.sender(),
        deltas,
        SHADE_EVENTS.receiver(),
        SHADE_ACKS.sender(),
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

/// Where the main stack is, and where it is safe to paint.
///
/// Returns the lowest address this firmware may write for its own purposes and
/// the top of the stack, or `None` if the image's layout is not the one assumed
/// — in which case nothing is painted and nothing is measured, which is the only
/// honest response to not recognising the ground.
///
/// The symbols are esp-hal's own, read exactly as esp-hal reads them
/// (`soc::ensure_stack_pointer_in_range`), which is `pub(crate)` and so cannot
/// be called from here. **`__stack_chk_guard` is the one that must be respected
/// rather than merely known about**: esp-hal places it
/// `ESP_HAL_CONFIG_STACK_GUARD_OFFSET` (60) bytes above the bottom and puts a
/// hardware data watchpoint on it, so a write there is a panic by design.
/// Painting starts one word past it.
///
/// **There is exactly one such word, and this is why that was checked rather
/// than hoped.** `esp-rtos` keeps a guard per task too, and for the main task it
/// takes the same offset from the same symbol
/// (`esp-rtos-0.3.0/src/lib.rs:331`, `task/mod.rs::set_up_stack_guard`) and
/// deliberately leaves the value alone when it already matches — so its guard
/// *is* `__stack_chk_guard` rather than a second word at some other offset. Its
/// other two overflow checks are unaffected by writes below the frame:
/// `sw-task-overflow-detection` is off by default and reads only that same word,
/// and `stack-pointer-range-check` compares the stack pointer against the
/// region's bounds.
fn stack_region() -> Option<(usize, usize, usize)> {
    unsafe extern "C" {
        static _stack_end_cpu0: u32;
        static _stack_start_cpu0: u32;
        static __stack_chk_guard: u32;
    }
    // None is dereferenced — only the addresses themselves are taken, which is
    // what makes this safe and why no `unsafe` block is needed for it.
    let bottom = (&raw const _stack_end_cpu0) as usize;
    let top = (&raw const _stack_start_cpu0) as usize;
    let guard = (&raw const __stack_chk_guard) as usize;
    // Everything below rests on the guard lying inside the region, on the region
    // being non-empty, and on both ends being word-aligned. All three hold for
    // the linker script this crate builds against; none is assumed.
    let floor = guard.checked_add(4)?;
    if bottom > guard || floor >= top || !floor.is_multiple_of(4) || !top.is_multiple_of(4) {
        return None;
    }
    Some((bottom, floor, top))
}

/// Fill the unused stack with [`STACK_PAINT`] so [`stack_used`] can read back how
/// far down it was destroyed.
///
/// **This is the only thing in this firmware that can tell the truth about the
/// stack**, and the reason it exists is that the constant it stands next to went
/// stale and took the board down with it. Everything else — the boot check, the
/// compile-time gate — compares one written-down number against another.
///
/// Called at the top of [`entry`], so it covers everything except the 144 bytes
/// of executor frames above it, which do not move.
///
/// # Why it cannot corrupt a live frame
///
/// The ceiling is [`PAINT_HEADROOM_BYTES`] below `probe`, a local of *this*
/// function — so it is below this function's own frame, which `#[inline(never)]`
/// guarantees is the deepest live one when the loop runs. Reading the stack
/// pointer out of the register would be the obvious way to establish that and is
/// **not** used: `core::arch::asm!` is unstable on Xtensa, and this crate's
/// `impl_trait_in_assoc_type` is deliberately its only unstable language feature.
/// The address of a local is stable, arch-independent, and a stronger argument
/// anyway — it does not depend on what a register is claimed to hold.
///
/// An interrupt taken while the loop runs lands immediately below the frame and
/// so inside the reserved headroom, above every byte this touches. And the floor
/// is one word above esp-hal's stack guard, which is watched by hardware and
/// must not be written. If any of that cannot be established from the linker's
/// own symbols the function returns having written nothing.
#[inline(never)]
fn paint_stack() {
    let probe = 0u32;
    let frame = (&raw const probe) as usize;
    let Some((_, floor, top)) = stack_region() else {
        return;
    };
    if frame > top || frame <= floor {
        return;
    }
    let ceiling = frame.saturating_sub(PAINT_HEADROOM_BYTES);
    let mut at = floor;
    while at < ceiling {
        // SAFETY: `at` is a 4-aligned address in `floor..ceiling`, which the
        // checks above have established lies strictly inside the linker's stack
        // region, strictly above esp-hal's guard word, and at least
        // PAINT_HEADROOM_BYTES below a local of this frame. Nothing else owns
        // stack memory that far below the running frame; an interrupt arriving
        // mid-loop uses the reserved headroom and has returned before anything
        // reads back what is written here.
        unsafe { (at as *mut u32).write_volatile(STACK_PAINT) };
        at += 4;
    }
}

/// How deep the stack has been since [`paint_stack`] ran.
///
/// Scans up from the bottom for the first word that is no longer
/// [`STACK_PAINT`]: everything below it is still virgin, so the distance from
/// there to the top is what has been used. Reads only.
///
/// It is a **floor**, in two ways worth naming rather than glossing. The
/// shallowest [`PAINT_HEADROOM_BYTES`] were never painted, so a boot that never
/// went deep reports that headroom as though it had been spent; and live data
/// that happens to equal the pattern reads as virgin. Both err toward reporting
/// *less* depth than was reached — which is the direction that matters, because
/// a figure close to the available stack is then certainly close.
fn stack_used() -> usize {
    let Some((_, floor, top)) = stack_region() else {
        return 0;
    };
    let mut at = floor;
    while at < top {
        // SAFETY: `at` is a 4-aligned address strictly inside the linker's stack
        // region, established by `stack_region` and bounded by this loop.
        if unsafe { (at as *const u32).read_volatile() } != STACK_PAINT {
            break;
        }
        at += 4;
    }
    top - at
}

/// Refuse to start if the main stack is smaller than the deepest chain needs.
///
/// See [`heap::REQUIRED_STACK_BYTES`] for why this is a runtime check rather
/// than a `const` assertion, and for what it covers. A stack overflow presents
/// either as a stack-guard panic and a boot loop — which is what it did — or,
/// if the guard word happens not to be written, as random corruption in a pulse
/// train, a shade that responds intermittently with nothing pointing at the
/// cause. Both are worth a number at boot.
fn check_stack_headroom() -> Result<(), StartError> {
    let Some((bottom, _, top)) = stack_region() else {
        // Not a failure to start: the check could not be performed, which is a
        // different thing from failing it, and the controller has no business
        // refusing to receive over a diagnostic it could not take.
        esp_println::println!(
            "stack: cannot locate the main stack region — headroom unchecked. \
             This build's linker layout is not the one crate::stack_region assumes."
        );
        return Ok(());
    };
    let available = top.saturating_sub(bottom);
    esp_println::println!(
        "stack: {} bytes available, {} required",
        available,
        heap::REQUIRED_STACK_BYTES,
    );
    if available < heap::REQUIRED_STACK_BYTES {
        return Err(StartError::StackTooSmall {
            available,
            required: heap::REQUIRED_STACK_BYTES,
        });
    }
    Ok(())
}

/// Print how much of the stack the boot actually used, against what was claimed.
///
/// **The point of this line is the gap between its two numbers.** `available`
/// and `required` are both written down; `used` is the only one that was
/// measured, and it is what would have said — in one line, on the first boot
/// after the web server landed — that the requirement had stopped being true.
///
/// Printed after the network is up, because that is past the deepest chain this
/// firmware has: the boot path, which ends in the state task's future being
/// moved into its static. The request path is about 19 KB shallower, so a later
/// reading would not be a larger one; if that ever stops holding, this figure is
/// what will show it.
fn report_stack_use() {
    let used = stack_used();
    let headroom = heap::REQUIRED_STACK_BYTES.saturating_sub(used);
    esp_println::println!(
        "stack: {} bytes used at the deepest point of boot, of {} required — \
         {} bytes of the requirement unspent",
        used,
        heap::REQUIRED_STACK_BYTES,
        headroom,
    );
    if used > heap::REQUIRED_STACK_BYTES {
        // Not a panic and not a refusal: the board is already past the deepest
        // point and is running. It is a statement that the constants in
        // `crate::heap` are now describing a different program from this one,
        // which is exactly the state that produced a boot loop last time and
        // said nothing.
        esp_println::println!(
            "stack: THE REQUIREMENT IS STALE — this boot used more than \
             heap::REQUIRED_STACK_BYTES claims is needed. Re-read the chains \
             from a linked ELF (the commands are in crates/firmware/src/heap.rs) \
             before this margin runs out."
        );
    }
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
