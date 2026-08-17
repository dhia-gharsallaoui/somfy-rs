//! Wi-Fi and the TCP/IP stack — the part of the controller that is allowed to
//! fail.
//!
//! Spec R9 and the design spec's degraded-operation rule say it plainly: **the
//! network is a degradable service.** A network that is absent, out of range,
//! or rejecting credentials must not affect radio control. Everything in this
//! module is arranged around that one sentence.
//!
//! ## How the separation is actually kept
//!
//! It is not kept by care. Four structural things keep it:
//!
//! 1. **No shared state.** Nothing here touches [`crate::store::FlashStore`],
//!    the transmit queue, the frame channel or the command channel. The radio
//!    and state tasks are constructed from hardware and channels alone, and
//!    adding a network resource to either would be a change to their types.
//! 2. **The radio tasks are spawned first, and unconditionally.** [`start`]
//!    returns a `Result`, and `main` prints its error and carries on. There is
//!    no path on which a Wi-Fi failure prevents the controller starting — the
//!    ordinary state of a freshly flashed board is *no credentials at all*,
//!    and that board still receives and decodes.
//! 3. **Nothing here waits without awaiting, and nothing here logs freely.**
//!    Every wait in these three tasks is an `await` on a timer, a Wi-Fi event
//!    or a socket; a single-threaded cooperative executor gives no protection
//!    against a task that spins, so that is a property to state rather than
//!    assume. Logging needed the same care and did not get it at first:
//!    `esp_println` writes each line inside a critical section, byte by byte,
//!    spinning on the UART FIFO with interrupts disabled — so a log line is
//!    *not* an await, and `tasks`' own docs put the cost of one at most of the
//!    ~5 ms available to re-arm the receiver between a frame and its repeat.
//!    An access point that is absent or flapping would otherwise produce two
//!    such lines a second, forever. [`RETRY_LOG_INTERVAL`] is the answer, and
//!    it is the same discipline `tasks::worth_reporting` already applies to
//!    the receive-side anomalies.
//! 4. **The radio's timing does not live on the CPU at all.** `esp-radio` runs
//!    its driver on preemptive `esp-rtos` threads that will interrupt the
//!    Embassy executor — but a transmission is clocked out of RMT RAM by the
//!    RMT peripheral (`radio::rmt_tx` asserts that a worst-case frame fits its
//!    reserved blocks, so no refill is ever needed mid-burst), and a reception
//!    is timestamped by the same peripheral. Preemption can delay the software
//!    that *starts* a burst or *drains* a reception; it cannot stretch a pulse.
//!
//! The one interaction that does exist runs the other way, and it is worth
//! naming: the state task's flash commits disable interrupts for as long as an
//! erase takes — tens of milliseconds, one commit in sixteen. Wi-Fi is deaf
//! for that window too. That is the correct direction for this device to lose
//! things in: a Wi-Fi link recovers from a 30 ms gap without anyone noticing,
//! and a rolling code that did not reach flash costs a re-pairing procedure at
//! the shade.
//!
//! ## The heap, and the one caveat to all of the above
//!
//! `esp-radio` is the only reason this firmware has one. [`crate::heap`] owns
//! it, carries the size and the argument for it, and is where to look for why
//! nothing on the frame path can be starved by it.
//!
//! It is also the **one** way a Wi-Fi decision can stop the controller
//! starting, and it is worth being exact about how: the heap is a static, so
//! it comes out of the DRAM the main stack is carved from, and an over-large
//! one would take the stack below what the transmit path needs.
//! `main::check_stack_headroom` refuses to start in that case, with a number.
//!
//! That is not a runtime dependency on the network — the size is a compile-time
//! constant, so a board that boots at all has already passed the check, and no
//! network condition can move it afterwards. And the alternative is worse: a
//! stack too small for `RmtTx::transmit_frame` corrupts pulse trains silently.
//! But it is the one place where "Wi-Fi cannot affect the radio" needs the
//! qualifier *at run time*, so it is said here rather than left to be found.

use core::cell::Cell;

use embassy_executor::{SpawnError, Spawner};
use embassy_futures::select::{select, Either};
use embassy_net::{Config as NetConfig, Runner, Stack, StackResources};
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    sta::StationConfig, AuthenticationMethod, Config as WifiConfig, Interface, WifiController,
    WifiError,
};
use somfy_config::WifiCredentials;
use somfy_tasks::Backoff;
use static_cell::StaticCell;

/// Shortest wait between association attempts.
///
/// An access point that refuses immediately — a wrong passphrase, a MAC filter
/// — would otherwise be retried as fast as the driver can scan, which is a
/// radio transmitting continuously in the 2.4 GHz band for no purpose. One
/// second is long enough that the retry is not a load and short enough that a
/// transient failure costs nothing anyone would notice.
const RETRY_MIN_MS: u32 = 1_000;

/// Longest wait between association attempts.
///
/// The bound R9 asks for. A router that reboots must be rejoined without a
/// human power-cycling this device, so the delay has to stop growing
/// somewhere; a minute is short enough that a reboot is invisible by the time
/// anyone looks, and long enough that a network which is simply gone costs one
/// association attempt a minute rather than a continuous scan.
const RETRY_MAX_MS: u32 = 60_000;

/// How long an association must last before it counts as a working link.
///
/// [`Backoff::succeed_after`] carries the argument: an access point that
/// associates and *then* drops the station — a captive portal, a MAC policy
/// check, a network with no DHCP server — reports success on every attempt, so
/// resetting the delay on association alone would pin the retry at
/// [`RETRY_MIN_MS`] forever and defeat R9's bound in the one case that most
/// needs it.
///
/// Ten seconds because that is comfortably longer than DHCP takes on any
/// network that has a server — the ordinary case this must not punish — and
/// far shorter than any outage a person would notice.
const STABLE_LINK_MS: u32 = 10_000;

/// One consecutive association failure in this many is logged, after the
/// first and after any change in the retry delay.
///
/// A log line is not free and it is not an await; see this module's docs. The
/// interesting lines are the first failure and each step of the backoff, which
/// are always printed — this only bounds the cost of the steady state, where
/// the delay has reached its ceiling and the message is the same every time.
/// At [`RETRY_MAX_MS`] it works out at one line every ten minutes.
const RETRY_LOG_INTERVAL: u32 = 10;

/// Socket slots the stack is built with.
///
/// DHCP takes one for as long as it is configured. The other two are for
/// whatever connects — Task 3's MQTT session is one of them — and a slot is a
/// few dozen bytes of the `StackResources` static, so the spare one costs
/// nothing worth economising on.
const SOCKETS: usize = 3;

/// How often the station's signal strength is re-read while the link is up.
///
/// Thirty seconds because that is the timescale a person watching a
/// signal-strength graph works on, and because each sample costs a fresh
/// subscription to `esp-radio`'s event channel — see [`hold_link`], which is
/// where that cost comes from and why it is bounded.
const RSSI_POLL_S: u64 = 30;

/// The station's last measured signal strength, in dBm, or `None` if the link
/// has never come up.
///
/// **A `blocking_mutex` rather than an atomic, and the matrix is why.**
/// `riscv32imc` — the ESP32-C3's target — has no atomic read-modify-write
/// instruction, so the natural `AtomicI32` shape is not available across all
/// four chips. A critical-section mutex around a `Cell` is, costs a handful of
/// instructions, and is held for a single load or store.
///
/// It exists because [`WifiController::rssi`] takes `&self` on a controller the
/// [`hold_link`] task owns for the life of the program — dropping it
/// deinitialises Wi-Fi — so the broker session cannot read it directly. This is
/// the seam, and it carries a value rather than the controller: nothing outside
/// this module can reach the radio through it.
///
/// `None` rather than a placeholder for the same reason a shade with no
/// observed position publishes nothing: the reading would go out **retained**,
/// so a made-up figure would outlive the boot that produced it and be handed to
/// every later subscriber.
static SIGNAL_DBM: Mutex<CriticalSectionRawMutex, Cell<Option<i32>>> = Mutex::new(Cell::new(None));

/// The station's last measured signal strength, for whoever reports it.
#[cfg_attr(
    not(feature = "mqtt"),
    allow(dead_code, reason = "read only by the broker session")
)]
pub fn signal_dbm() -> Option<i32> {
    SIGNAL_DBM.lock(Cell::get)
}

/// Record a sample, or clear it when the link goes away.
fn record_signal(dbm: Option<i32>) {
    SIGNAL_DBM.lock(|cell| cell.set(dbm));
}

/// `embassy-net`'s buffers and socket table, which the stack borrows for
/// `'static`.
///
/// A `StaticCell` rather than a `static mut`: it hands out the `&'static mut`
/// exactly once and panics on a second attempt, so the aliasing rule is
/// enforced at runtime instead of being asserted in an `unsafe` block. [`start`]
/// can only be called once anyway — it consumes the `WIFI` peripheral
/// singleton — and this makes that a fact rather than a convention.
static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

/// Why the network could not be brought up.
///
/// Every variant is reported and then ignored: `main` prints it and the
/// controller carries on without a network. That is the whole point of the
/// module — see its docs.
#[allow(dead_code)]
#[derive(Debug)]
pub enum NetError {
    /// The Wi-Fi driver refused to initialise, or refused the station config.
    Wifi(WifiError),
    /// A network task could not be spawned.
    Spawn(SpawnError),
}

/// Bring up station-mode Wi-Fi and the TCP/IP stack, and hand back the stack.
///
/// Failure here is not fatal and must not be made fatal: the caller reports it
/// and leaves the radio running. The returned [`Stack`] is what Task 3's MQTT
/// session will connect through; nothing in Plan 5 Task 2 uses it beyond
/// [`address_watch`] reporting what DHCP handed out.
pub fn start(
    spawner: Spawner,
    wifi: WIFI<'static>,
    credentials: &WifiCredentials,
) -> Result<Stack<'static>, NetError> {
    // Everything the driver needs to know about the network, set once here.
    // The task below never sees the credentials at all — it drives the
    // connection, not the configuration — which keeps the passphrase in one
    // place instead of inside a task's statically allocated future.
    let station = StationConfig::default()
        .with_ssid(credentials.ssid())
        .with_password(alloc::string::String::from(credentials.psk()))
        .with_auth_method(if credentials.is_open() {
            AuthenticationMethod::None
        } else {
            // Not the strictest method the access point might offer: this is
            // the *minimum* the station will accept, and raising it would
            // refuse a WPA2 network that a WPA3 setting would have rejected
            // outright. The driver negotiates upward from here.
            AuthenticationMethod::Wpa2Personal
        });

    let (mut controller, interfaces) =
        esp_radio::wifi::new(wifi, Default::default()).map_err(NetError::Wifi)?;
    controller
        .set_config(&WifiConfig::Station(station))
        .map_err(NetError::Wifi)?;

    // Seeded from the hardware RNG rather than from a constant: this seeds
    // smoltcp's TCP initial sequence numbers and its ephemeral port choice,
    // and a fixed seed makes every board on the network pick the same ports in
    // the same order. `esp_hal::rng::Rng` is backed by the true RNG once the
    // radio is running, which it now is.
    let rng = esp_hal::rng::Rng::new();
    let seed = ((rng.random() as u64) << 32) | rng.random() as u64;

    let (stack, runner) = embassy_net::new(
        interfaces.station,
        // DHCP, not a static address: this device is one of many on a home
        // network and has no business claiming an address of its own.
        NetConfig::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    // All three tokens first, then all three spawns — the same discipline
    // `main` uses for the radio and state tasks, and for the same reason: a
    // spawn failure part-way through would leave a half-started network, which
    // is harder to diagnose than none at all.
    let link = wifi_link(controller).map_err(NetError::Spawn)?;
    let stack_runner = net_stack(runner).map_err(NetError::Spawn)?;
    let watch = address_watch(stack).map_err(NetError::Spawn)?;
    spawner.spawn(link);
    spawner.spawn(stack_runner);
    spawner.spawn(watch);

    esp_println::println!("wifi: joining '{}'", credentials.ssid());
    Ok(stack)
}

/// Keep the station associated, with bounded backoff.
///
/// The loop is deliberately dull: attempt, report, wait for the link to drop,
/// wait out the backoff, repeat. Neither `esp-radio` nor the MQTT client chosen
/// for Task 3 provides reconnection, so this is where R9's "reconnect with
/// bounded backoff" is implemented, and [`Backoff`] is host-tested because the
/// three ways to get it wrong are all silent.
///
/// ## Why the wait is outside the match
///
/// So there is no path around it. `wait_for_disconnect_async` returns at once
/// when the station is not connected — its documented behaviour, and the right
/// one, since otherwise it would wait forever for an event that has already
/// happened — which means a link that comes up and drops immediately can walk
/// this loop with nothing in it that takes time. One wait, reached however the
/// attempt ended, is what makes that a 1-second cycle instead of a spin. A
/// clean reconnection after a brief outage pays the minimum, because
/// [`Backoff::succeed`] has just reset it.
///
/// It holds the [`WifiController`] for the whole life of the program on
/// purpose: dropping it deinitialises Wi-Fi.
#[embassy_executor::task]
async fn wifi_link(mut controller: WifiController<'static>) -> ! {
    let mut backoff = Backoff::new(RETRY_MIN_MS, RETRY_MAX_MS);
    let mut consecutive = 0u32;
    let mut previous_delay = 0u32;
    loop {
        match controller.connect_async().await {
            Ok(info) => {
                // Sampled here rather than only in the loop below, so the first
                // figure exists before the broker session can announce anything
                // — otherwise the signal-strength entity would appear with no
                // reading for up to `RSSI_POLL_S`, which is the "appears and
                // does nothing" shape this project avoids everywhere else.
                sample_signal(&controller);
                esp_println::println!(
                    "wifi: associated on channel {} ({:?} dBm)",
                    info.channel,
                    signal_dbm(),
                );

                // Timed, because the reset below depends on it having lasted.
                let joined = Instant::now();
                match hold_link(&controller).await {
                    Ok(info) => esp_println::println!("wifi: disconnected — {:?}", info.reason),
                    Err(error) => esp_println::println!("wifi: link lost — {:?}", error),
                }
                // The link is gone, so the last sample is no longer a fact about
                // anything. Cleared rather than left: a stale signal strength is
                // a confidently wrong retained value, which is the failure class
                // the whole MQTT integration is written around.
                record_signal(None);
                let lasted = joined.elapsed().as_millis().min(u32::MAX as u64) as u32;

                if backoff.succeed_after(lasted, STABLE_LINK_MS) {
                    consecutive = 0;
                } else {
                    esp_println::println!(
                        "wifi: the link lasted {} ms, under the {} ms it takes to count \
                         as working — backing off rather than retrying at full rate",
                        lasted,
                        STABLE_LINK_MS,
                    );
                }
            }
            Err(error) => {
                consecutive = consecutive.saturating_add(1);
                // Printing the error is safe: it carries the SSID and the
                // access point's reason code, never the passphrase — and
                // `WifiCredentials` redacts that from `Debug` in any case.
                //
                // Rate-limited because a log line is not an await. Always the
                // first, always a step of the backoff, and otherwise one in
                // `RETRY_LOG_INTERVAL`; see this module's docs.
                if consecutive == 1
                    || backoff.delay_ms() != previous_delay
                    || consecutive.is_multiple_of(RETRY_LOG_INTERVAL)
                {
                    esp_println::println!(
                        "wifi: association failed ({} in a row) — {:?}",
                        consecutive,
                        error,
                    );
                }
            }
        }

        let waiting = backoff.fail();
        if waiting != previous_delay {
            esp_println::println!("wifi: retrying in {} ms", waiting);
        }
        previous_delay = waiting;
        Timer::after(Duration::from_millis(waiting as u64)).await;
    }
}

/// Wait out one association, sampling the signal strength as it goes.
///
/// ## Why this is a loop around a `select` and not just a wait
///
/// [`WifiController::rssi`] needs the controller, and the controller lives here
/// for the life of the program. So the only place a periodic sample can be
/// taken is inside the wait that would otherwise occupy this task for hours.
///
/// ## Why dropping the disconnect future cannot lose a disconnect
///
/// `wait_for_disconnect_async` subscribes to `esp-radio`'s event channel on
/// **each call** and drops the subscription with its future, so a timer that
/// wins the race does miss any event published in the gap. It is not lost, and
/// the reason is an ordering inside the driver rather than a hope about timing:
/// `event_post` calls `state::update_state(event)` — which stores
/// `WifiStationState::Disconnected` — **before** it publishes to
/// `EVENT_CHANNEL` (`esp-radio-0.18.0/src/wifi/os_adapter/mod.rs:674`, then
/// `:677`). And `wait_for_disconnect_async`'s first act is
/// `if !self.is_connected() { return Err(WifiError::NotConnected) }`
/// (`src/wifi/mod.rs:2967`), reading that same state.
///
/// So the flag an unsubscribed caller reads is set strictly earlier than the
/// message it could have missed. A disconnect that lands in the gap is reported
/// on the next iteration — one `select` later at worst — as `NotConnected`,
/// which the caller already treats as a lost link.
///
/// ## The subscriber budget, with the number
///
/// One subscription exists at a time and it is dropped before the next is
/// created, which matters because `esp-radio` `expect`s its subscriber slots
/// rather than handling exhaustion — and this firmware's panic handler resets
/// the board. The budget is **two**
/// (`esp-radio-0.18.0/esp_config.yml`'s `event_channel_subscribers` default),
/// `connect_async` holds one and never overlaps with this function, and this
/// holds at most one. The margin is exactly one slot, so a second concurrent
/// waiter added to this task would be the thing that spends it.
async fn hold_link(
    controller: &WifiController<'static>,
) -> Result<esp_radio::wifi::DisconnectedStationInfo, WifiError> {
    loop {
        match select(
            controller.wait_for_disconnect_async(),
            Timer::after(Duration::from_secs(RSSI_POLL_S)),
        )
        .await
        {
            Either::First(outcome) => return outcome,
            Either::Second(()) => sample_signal(controller),
        }
    }
}

/// Read the station's signal strength, or record that there is none to read.
///
/// An error means the driver could not answer — the station is not in station
/// mode, or the call failed — and that is recorded as "no reading" rather than
/// as a number, for the reason on [`SIGNAL_DBM`].
fn sample_signal(controller: &WifiController<'static>) {
    record_signal(controller.rssi().ok());
}

/// `embassy-net`'s own runner: polls smoltcp, drives DHCP, moves frames.
///
/// Nothing but a wrapper. It exists as a task because `Runner::run` never
/// returns and has to be polled by something.
#[embassy_executor::task]
async fn net_stack(mut runner: Runner<'static, Interface<'static>>) -> ! {
    runner.run().await
}

/// Say what DHCP handed out, and say when it goes away again.
///
/// The only observable output of this task in Plan 5 Task 2, and the one that
/// distinguishes "associated" from "on the network": a station can be
/// associated and still have no address, which looks identical from the Wi-Fi
/// side and is the state in which nothing works.
#[embassy_executor::task]
async fn address_watch(stack: Stack<'static>) -> ! {
    loop {
        stack.wait_config_up().await;
        match stack.config_v4() {
            Some(config) => esp_println::println!(
                "net: address {} gateway {:?}",
                config.address,
                config.gateway,
            ),
            // Unreachable in practice — `wait_config_up` returned — but a
            // panic here would take the radio off the air over a log line.
            None => esp_println::println!("net: configured, but no IPv4 address to report"),
        }
        crate::heap::report("network up");
        stack.wait_config_down().await;
        esp_println::println!("net: address lost");
    }
}
