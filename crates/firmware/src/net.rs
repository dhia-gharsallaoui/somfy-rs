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
//!
//!    **That argument is careful about the wrong order of magnitude, and it is
//!    worth saying so here rather than leaving the imbalance to be found.** A
//!    log line holds a critical section once; the Wi-Fi driver takes one per
//!    allocation and one per free, and an *associated link with no application
//!    traffic at all* churns about 30,000 bytes a second through `esp-alloc` —
//!    measured, 2026-08-17. Nothing on the frame path allocates, which is the
//!    claim [`crate::heap`] makes and CI checks; something else does, several
//!    hundred times a second, with interrupts masked. See that module for the
//!    measurement and for why it has not been observed to matter.
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
use embassy_futures::select::{select, select3, Either3};
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
/// **A sum of what is actually in the image, not a round number.** Exceeding it
/// is not a degraded service: `embassy_net::tcp::TcpSocket::new` panics on a
/// full socket set, and this firmware's panic handler reboots the board — so a
/// stack one slot short is a device that reboots the moment the last client
/// connects, which is the failure mode this arithmetic exists to make
/// impossible.
///
/// It is one of the two places a `#[cfg]` on a transport appears outside that
/// transport's own module, and it earns it: how many sockets a network needs is
/// a property of who connects to it, and the alternative — sizing for the
/// largest configuration always — costs the ESP32 about 1.5 KB of its Wi-Fi
/// heap for sockets an image without a web server can never open.
const SOCKETS: usize = DHCP_SOCKETS + BROKER_SOCKETS + SERVER_SOCKETS + MDNS_SOCKETS + SNTP_SOCKETS;

/// DHCP holds one for as long as the address is configured.
const DHCP_SOCKETS: usize = 1;

/// The broker session's, when there is one. It reconnects on the same slot.
#[cfg(feature = "mqtt")]
const BROKER_SOCKETS: usize = 1;
/// See the `mqtt` definition above.
#[cfg(not(feature = "mqtt"))]
const BROKER_SOCKETS: usize = 0;

/// One per web-server connection task, because each accepts on its own socket.
#[cfg(feature = "http")]
const SERVER_SOCKETS: usize = crate::api::HTTP_TASKS;
/// See the `http` definition above.
#[cfg(not(feature = "http"))]
const SERVER_SOCKETS: usize = 0;

/// The responder's, one per DHCP lease. It is closed and reopened when the
/// address changes, on the same slot.
#[cfg(feature = "mdns")]
const MDNS_SOCKETS: usize = crate::mdns::SOCKETS;
/// See the `mdns` definition above.
#[cfg(not(feature = "mdns"))]
const MDNS_SOCKETS: usize = 0;

/// The SNTP exchange's, **and the resolver's**.
///
/// Two rather than one, and the second is the one that is easy to miss: turning
/// on `embassy-net`'s `dns` feature makes `embassy_net::new` add a DNS socket to
/// the set unconditionally, before any of this firmware's code runs. It is
/// counted here because `sntp` is the feature that turns `dns` on — see this
/// crate's `Cargo.toml` — and [`resolve`] is the only thing that uses it.
#[cfg(feature = "sntp")]
const SNTP_SOCKETS: usize = crate::sntp::SOCKETS;
/// See the `sntp` definition above.
#[cfg(not(feature = "sntp"))]
const SNTP_SOCKETS: usize = 0;

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
/// **A `blocking_mutex` rather than an atomic, and this is the note the other
/// four sites point at.** The reason was the build matrix: `riscv32imc` — the
/// ESP32-C3's target — has no atomic read-modify-write instruction, so the
/// natural `AtomicI32` shape was not available on every chip this crate built
/// for. A critical-section mutex around a `Cell` is, costs a handful of
/// instructions, and is held for a single load or store.
///
/// **The ESP32-C3 was dropped on 2026-08-19 and this shape is kept**, here and
/// at the four other sites that cite it (`api::events::WS_HELD`,
/// `sntp::WALL_CLOCK`, `trial::Slot`, `mqtt::Rare`). That is a decision, and it
/// is recorded rather than left as a stale reason. Changing five of these to
/// atomics would be five edits to working, shipped, unmeasured-cost code in
/// exchange for a handful of instructions on paths that run at most once a
/// second — and it would throw away the one property that survives the chip:
/// this shape compiles for *any* Espressif part, including the RISC-V ones a
/// future contributor may want back. `docs/provenance.md` records that the
/// RISC-V row is what caught the original fault, and that nothing in CI catches
/// the next one.
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
    let station = station_config(credentials);

    // `ControllerConfig::default()`, taken whole and deliberately. Two of the
    // defaults it carries are worth naming, because both look wrong to a reader
    // who does not know why they are there.
    //
    // **The country code is `CN`, and it stays** (owner's decision, 2026-08-17).
    // It is inherited from the default rather than chosen by us, and what
    // reaches the driver is `esp_wifi_set_country` with `schan: 1, nchan: 13,
    // max_tx_power: 20` and `WIFI_COUNTRY_POLICY_MANUAL` — channels 1 to 13 at
    // 20 dBm, always, which is exactly the EU allocation this device is deployed
    // under. Right, then, and right by accident, which is the whole reason this
    // paragraph exists.
    //
    // **The two-letter code does not select that channel range.** Read
    // `CountryInfo::into_blob` (`esp-radio-0.18.0/src/wifi/mod.rs:2019`): only
    // `cc` comes from the code, while `schan`, `nchan` and `max_tx_power` are
    // literals with a `TODO` over them saying they ought to be configurable. So
    // `.with_country_info(*b"DE")` would change the string in the beacon and
    // **not** the channels or the power, and `esp-radio` 0.18 offers no way to
    // change them at all. Anyone "correcting" `CN` to a European code would
    // therefore be making a cosmetic change while believing they had made a
    // regulatory one — which is the failure this note is here to prevent, and
    // the reason the code is left alone rather than quietly improved.
    //
    // It follows that this is a **deployment-dependent** default rather than a
    // universally safe one: under FCC rules channels 12 and 13 are not
    // permitted, and no setting exposed here would exclude them. A board for
    // that domain needs a change in `esp-radio`, not a change in this file.
    //
    // **Frame aggregation (AMPDU) is left on**, which is the default too. It was
    // measured as a way to recover heap headroom and it does not: `crate::heap`
    // carries fourteen boots with it and seventeen without, and the answer is
    // 316 bytes of steady use against a worst case that does not move outside
    // the noise. Turning it off needs `esp-radio/unstable`, which is a real cost
    // for that; the note there has the figures so this is not re-investigated.
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
    // The credential trial's clock. Gated on `http` because the settings screen
    // is the only thing that can start a trial, and a build with no web server
    // would be running a timer over a slot nothing can ever fill.
    #[cfg(feature = "http")]
    let trial = crate::trial::watch(stack).map_err(NetError::Spawn)?;
    spawner.spawn(link);
    spawner.spawn(stack_runner);
    spawner.spawn(watch);
    #[cfg(feature = "http")]
    spawner.spawn(trial);

    crate::logln!("wifi: joining '{}'", credentials.ssid());
    Ok(stack)
}

/// What the driver needs to know about one network.
///
/// Factored out of [`start`] because a credential trial builds one too, and the
/// two must be built the same way: a trial that negotiated authentication
/// differently from the boot path would prove a credential under conditions the
/// device would not reproduce after a restart.
fn station_config(credentials: &WifiCredentials) -> StationConfig {
    StationConfig::default()
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
        })
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
///
/// ## Why the candidate is applied here and nowhere else
///
/// [`WifiController::set_config`] needs `&mut`, and this task holds the only
/// controller there is. So a credential trial cannot apply itself; it leaves a
/// candidate in `crate::trial`'s slot and raises a signal, and this loop picks
/// it up at the top of its next pass. The signal is what makes "next pass"
/// prompt: without it a candidate requested during a sixty-second backoff would
/// sit unapplied until that wait ran out, and the trial's own deadline would
/// have started counting against a radio that had not moved.
#[embassy_executor::task]
async fn wifi_link(mut controller: WifiController<'static>) -> ! {
    let mut backoff = Backoff::new(RETRY_MIN_MS, RETRY_MAX_MS);
    let mut consecutive = 0u32;
    let mut previous_delay = 0u32;
    loop {
        if let Some(candidate) = crate::trial::take_requested() {
            // **Before touching the radio**, so the `202` that started this has
            // left the socket the candidate is about to take down. See
            // `crate::trial`'s settle constant for why that is a socket
            // question rather than a radio one.
            Timer::after(crate::trial::settle()).await;
            apply_candidate(&mut controller, candidate).await;
            // Straight back to the top: `connect_async` below is what joins the
            // candidate network, and the backoff is reset so the first attempt
            // on a new network is immediate rather than inheriting whatever
            // delay the previous one had reached.
            backoff.succeed();
            consecutive = 0;
            previous_delay = 0;
        }

        match controller.connect_async().await {
            Ok(info) => {
                // Sampled here rather than only in the loop below, so the first
                // figure exists before the broker session can announce anything
                // — otherwise the signal-strength entity would appear with no
                // reading for up to `RSSI_POLL_S`, which is the "appears and
                // does nothing" shape this project avoids everywhere else.
                sample_signal(&controller);
                crate::logln!(
                    "wifi: associated on channel {} ({:?} dBm)",
                    info.channel,
                    signal_dbm(),
                );

                // Timed, because the reset below depends on it having lasted.
                let joined = Instant::now();
                match hold_link(&controller).await {
                    Ok(info) => crate::logln!("wifi: disconnected — {:?}", info.reason),
                    Err(error) => crate::logln!("wifi: link lost — {:?}", error),
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
                    crate::logln!(
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
                    crate::logln!(
                        "wifi: association failed ({} in a row) — {:?}",
                        consecutive,
                        error,
                    );
                }
            }
        }

        let waiting = backoff.fail();
        if waiting != previous_delay {
            crate::logln!("wifi: retrying in {} ms", waiting);
        }
        previous_delay = waiting;
        // **`select`, not a bare `Timer`.** The wait itself is unchanged and
        // still unavoidable — see the note above on why it is outside the match
        // — but a candidate arriving during it must not have to sit out up to
        // `RETRY_MAX_MS` first. Losing the remainder of a backoff to apply a
        // candidate is exactly right: the network being backed off from is the
        // one the operator is replacing.
        let _ = select(
            Timer::after(Duration::from_millis(waiting as u64)),
            crate::trial::requested(),
        )
        .await;
    }
}

/// Put a candidate credential on the radio.
///
/// Nothing here writes flash and nothing here can: what makes a trial safe is
/// that the stored credential is untouched until somebody has come back through
/// the candidate and said so. See `crate::trial`.
///
/// A driver that refuses the configuration is reported and the trial dropped
/// rather than started — the radio is still on the previous network and
/// working, so running a deadline against it would revert a device that had
/// nothing wrong with it.
async fn apply_candidate(controller: &mut WifiController<'static>, candidate: WifiCredentials) {
    crate::logln!(
        "wifi: trying '{}' — the stored credential is untouched and comes back \
         unless somebody confirms from the new network",
        candidate.ssid(),
    );
    if let Err(error) = controller.set_config(&WifiConfig::Station(station_config(&candidate))) {
        crate::logln!(
            "wifi: the driver refused the candidate configuration ({:?}) — staying on \
             the stored credential",
            error,
        );
        crate::trial::not_applied();
        return;
    }
    // The old association has to go before the new one can be made. An error
    // here is the ordinary "there was nothing to disconnect" case, which is
    // what a board whose network was already down looks like.
    let _ = controller.disconnect_async().await;
    // The last sample belonged to the previous network.
    record_signal(None);
    crate::trial::applied(candidate, Instant::now().as_millis());
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
///
/// ## The third arm, and why it does not spend the last subscriber slot
///
/// A candidate credential arriving while the link is up has to be applied, and
/// applying it needs the caller's `&mut`. So this returns instead — with
/// `NotConnected`, which the caller already treats as a lost link and which is
/// about to be true. `crate::trial::requested` waits on a `Signal`, not on
/// `esp-radio`'s event channel, so it costs no subscriber slot at all and the
/// margin of exactly one above is untouched.
async fn hold_link(
    controller: &WifiController<'static>,
) -> Result<esp_radio::wifi::DisconnectedStationInfo, WifiError> {
    loop {
        match select3(
            controller.wait_for_disconnect_async(),
            Timer::after(Duration::from_secs(RSSI_POLL_S)),
            crate::trial::requested(),
        )
        .await
        {
            Either3::First(outcome) => return outcome,
            Either3::Second(()) => sample_signal(controller),
            Either3::Third(()) => return Err(WifiError::NotConnected),
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

/// Turn a name or an address literal into an IPv4 address.
///
/// # Why this is here and not in its caller
///
/// [`crate::sntp`] is the only caller today, and it needs this because the only
/// sane default NTP server is a *name* — there is no stable address for one, and
/// hard-coding a pool member's current IP would be a constant with no derivation
/// and a shelf life.
///
/// It is written as a general resolver rather than folded into that module
/// because of what is coming next: `somfy_config::MqttSettings` holds the broker
/// as an `Ipv4Addr`, so a broker on a home network cannot be named. Closing that
/// needs a field change in `somfy-config` and **one call to this function** —
/// the resolver, the DNS socket and the servers DHCP hands out are all in place
/// now, and the remaining work is entirely in the record.
///
/// # It answers with one address, not the set
///
/// `dns_query` returns everything the server sent, and `pool.ntp.org` returns
/// four. Taking the first is right for this device: they are interchangeable by
/// construction, and a client that tried them in turn would be implementing
/// server selection — which is NTP's job, not SNTP's, and is what a client
/// asking one question an hour has no use for.
///
/// # Timeouts
///
/// Bounded here rather than by the caller, because smoltcp's resolver retries on
/// its own schedule and "no DNS server answered" is otherwise a future that
/// never completes. [`RESOLVE_TIMEOUT_S`] is the whole budget including those
/// retries.
///
/// A literal costs none of this: `dns_query` parses `name` as an address first
/// and returns it without a packet, so a configuration that names an address
/// still works on a network with no working resolver.
#[cfg(feature = "sntp")]
pub async fn resolve(stack: Stack<'static>, name: &str) -> Option<core::net::Ipv4Addr> {
    use embassy_net::dns::DnsQueryType;
    use embassy_time::with_timeout;

    let answers = with_timeout(
        Duration::from_secs(RESOLVE_TIMEOUT_S),
        stack.dns_query(name, DnsQueryType::A),
    )
    .await;

    let answers = match answers {
        Ok(Ok(answers)) => answers,
        Ok(Err(error)) => {
            crate::logln!("net: could not resolve '{}' ({:?})", name, error);
            return None;
        }
        Err(_) => {
            crate::logln!(
                "net: no answer resolving '{}' within {} s",
                name,
                RESOLVE_TIMEOUT_S,
            );
            return None;
        }
    };

    // Only `DnsQueryType::A` was asked for, so only V4 can come back — and this
    // is written as a filter rather than an `expect` because it is also the
    // guard that keeps an IPv6 address out of `sntpc-net-embassy`, whose
    // address conversion answers one with `unreachable!()` in a build without
    // its `ipv6` feature. A panic there would reset the board over a DNS reply.
    answers.iter().find_map(|address| match address {
        embassy_net::IpAddress::Ipv4(v4) => Some(*v4),
        #[allow(
            unreachable_patterns,
            reason = "only V4 exists while `proto-ipv6` is off, and this arm is \
                      what keeps that true if it is ever turned on"
        )]
        _ => None,
    })
}

/// The whole budget for one name lookup, in seconds.
///
/// A **policy figure**, and it is a ceiling rather than an expectation: a home
/// resolver answers in single-digit milliseconds, and smoltcp retries a lost
/// query on its own before this fires. Ten seconds is chosen to match
/// [`STABLE_LINK_MS`] — the same "comfortably longer than anything a working
/// network does" reasoning — so that a resolver which is simply absent costs one
/// bounded wait rather than a task that never returns.
#[cfg(feature = "sntp")]
const RESOLVE_TIMEOUT_S: u64 = 10;

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
        // The one thing an over-the-air update's self-test wants from this
        // task, and it is deliberately the *strong* predicate: `config_up`
        // means an address, not merely an association. It is **reported and
        // never a trigger** — a release is not refused for failing to find an
        // access point — so this is one bit for a console line rather than a
        // vote. See `somfy_ota::selftest`.
        crate::ota::associated();
        match stack.config_v4() {
            Some(config) => crate::logln!(
                "net: address {} gateway {:?}",
                config.address,
                config.gateway,
            ),
            // Unreachable in practice — `wait_config_up` returned — but a
            // panic here would take the radio off the air over a log line.
            None => crate::logln!("net: configured, but no IPv4 address to report"),
        }
        crate::heap::report("network up");
        stack.wait_config_down().await;
        crate::logln!("net: address lost");
    }
}
