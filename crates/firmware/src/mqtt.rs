//! The broker session: a socket, a `minimq` client, and a loop that executes
//! the plans `somfy-mqtt` builds.
//!
//! Everything about *what* to publish and *whether it is retained* is decided
//! in `somfy-mqtt` and host-tested there. What is here is transport: opening a
//! TCP connection, handing it to `minimq`, walking a plan, translating a
//! [`somfy_mqtt::Step`] into a packet, and reconnecting when that fails.
//!
//! ## This task cannot affect the radio, and here is why
//!
//! The same four reasons [`crate::net`] gives, restated because this task is
//! the one that talks to something outside the house:
//!
//! 1. **No shared state.** Its arguments are a [`Stack`], the broker's own
//!    settings, a [`Broker`] (topic configuration, a snapshot of the shades to
//!    announce, and a snapshot of what the rolling-code region held at boot), a
//!    command *sender* and a delta *subscriber*. It holds no flash, no radio, no
//!    transmit queue, and no reference to the registry — the shades and the
//!    survey are both **copies taken before the state task owned anything**.
//!    Giving this task any of those would be a change to its type, not an
//!    oversight in its body. That is also why the rolling-code diagnostic is a
//!    boot figure rather than a live one: reporting it live would mean holding
//!    the store, which is the one thing this task must not do.
//! 2. **It is spawned after the radio tasks, and after the yield that polls
//!    them.** `main` spawns radio and state, yields — which is what actually
//!    runs them and arms the receiver — and only then starts the network.
//!    `Spawner::spawn` enqueues; it does not run.
//! 3. **Every wait is an `await`,** on a socket, a timer, or the delta channel.
//!    A cooperative executor gives no protection against a task that spins, so
//!    that is stated rather than assumed. Log lines are rate-limited by the
//!    same rule `net` uses, because `esp_println` writes inside a critical
//!    section and is therefore *not* an await.
//! 4. **It never blocks on the state task.** Commands go out through
//!    [`embassy_sync::channel::Sender::try_send`], never `send`: a full command
//!    queue drops the newest command and says so, rather than parking this task
//!    until the state task drains it. A queue of shade commands is a queue of
//!    intentions, and `somfy-tasks` already argues that acting on a stale one
//!    is worse than dropping it.
//!
//! The one direction that is *not* symmetric is worth naming: the state task's
//! flash commits disable interrupts for tens of milliseconds, so a commit can
//! stall the network. That is the correct direction for this device to lose
//! things in — a TCP session survives a 30 ms gap, and a rolling code that did
//! not reach flash costs a re-pairing procedure at the shade.
//!
//! ## What a successful connect proves
//!
//! `minimq` is MQTT v5 only: its CONNECT carries protocol version 5, and
//! [`Session::connect`] returns `Ok` only after a CONNACK with a success reason
//! code has been decoded as an MQTT v5 packet. So the line this logs on the
//! first connect is not an inference from the broker's version number — it is
//! the observation that the broker accepted a v5 CONNECT and answered in v5.
//! A 3.1.1-only broker answers `0x01` (unacceptable protocol version) or closes
//! the socket, and both arrive here as an error.
//!
//! ## The buffers, and where they come from
//!
//! All four are locals of the task body, so they live in the statically
//! allocated future `#[embassy_executor::task]` creates rather than on the main
//! stack. They still come out of the same DRAM the main stack is carved from —
//! see [`crate::heap`] — so `main::check_stack_headroom` is what stands between
//! these sizes and a board that will not boot. Their sizes are argued at each
//! constant.
//!
//! ## R7's entity set, and the one rule that shapes it
//!
//! `somfy-mqtt` decides *which* entities exist; this module supplies the values,
//! and [`Diagnostics`] is the table of where each one comes from. The rule is
//! that **an entity backed by nothing is worse than an absent one** — it reads
//! as a device fault rather than as an unimplemented feature — so a reading with
//! no honest source publishes nothing at all rather than a placeholder, exactly
//! as an unreported shade's position does.

use core::fmt::Write as _;
use core::net::SocketAddrV4;

use embassy_futures::select::{select4, Either4};
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_sync::pubsub::WaitResult;
use embassy_time::{Duration, Instant, Ticker, Timer};
use heapless::{String, Vec};
use minimq::{
    Buffers, ConfigBuilder, ConnectEvent, Publication, QoS, RetainHandling, Session,
    SubscriptionOptions, TopicFilter, Will,
};
use somfy_config::{MqttSettings, Namespaces};
use somfy_domain::{Direction, Pos, ShadeCommand, ShadeId, StateDelta, MAX_SHADES};
use somfy_mqtt::{
    reconfigure, Component, ConfigError, DeviceEntity, DeviceId, DiscoveryPrefix, MqttConfig,
    NodeId, Pairing, Payload, PublishedTopic, Retention, ShadeTopic, StateRoot, Step,
    PAYLOAD_CAPACITY,
};
use somfy_tasks::{Backoff, ControlCommand};

use crate::config::MAX_SUPERSEDED;
use crate::edits::{AckSender, EventReceiver, ShadeAck, ShadeEvent};
use crate::inventory::Inventory;
use crate::store::Survey;
use crate::tasks::{CommandSender, DeltaSubscriber};

/// Inbound MQTT packet buffer.
///
/// `minimq` advertises this as MQTT 5's `MaximumPacketSize` in CONNECT, so it
/// is not merely local storage — it is the ceiling the broker is told to obey,
/// and inbound is bounded by construction rather than by hope. Every payload
/// this device subscribes to is a single word or a number: `OPEN`, `CLOSE`,
/// `STOP`, `PRESS`, or a position. 512 bytes is two orders of magnitude beyond
/// that and still small enough to leave the ESP32 — the tightest chip in the
/// matrix — its stack.
const MQTT_RX_BYTES: usize = 512;

/// Outbound MQTT arena: the largest packet plus whatever QoS 1 state is in
/// flight.
///
/// The largest packet this device sends is a retained discovery config: a
/// topic bounded by `somfy-mqtt`'s own capacity proof at 143 bytes, a payload
/// bounded by [`somfy_mqtt::PAYLOAD_CAPACITY`] at 1152, and a fixed header —
/// about 1,310 bytes at the widest configuration `somfy-mqtt` will accept, and
/// under 600 at the one this firmware actually builds.
///
/// **One packet, not several.** [`perform`] settles after every operation, so
/// exactly one is ever in flight and the arena never has to hold two at once.
/// Without that, two of the widest configs would already overrun this figure —
/// which is the tighter of the two ceilings the settle discipline removes, and
/// the reason it is a bound rather than a budget to watch.
const MQTT_TX_BYTES: usize = 1664;

/// TCP receive window. Commands are tiny and arrive one at a time.
const TCP_RX_BYTES: usize = 768;

/// TCP send buffer. Sized to hold one whole discovery config so a publish is
/// handed to the stack in one piece rather than dribbled out.
const TCP_TX_BYTES: usize = 1024;

/// Shortest wait between broker connection attempts. Same figure and same
/// argument as [`crate::net::RETRY_MIN_MS`]: short enough that a transient
/// failure costs nothing anyone notices, long enough that a broker which is
/// refusing is not hammered.
const RETRY_MIN_MS: u32 = 1_000;

/// Longest wait between broker connection attempts. The bound R9 asks for.
const RETRY_MAX_MS: u32 = 60_000;

/// How long a session must last before it counts as a working one.
///
/// The same argument [`somfy_tasks::Backoff::succeed_after`] carries for Wi-Fi,
/// and it applies at least as strongly here: a broker that accepts the
/// connection and *then* closes it — wrong credentials on a broker configured
/// to disconnect rather than refuse, an ACL that rejects the first publish, a
/// second client using the same client id and kicking this one off — reports
/// success on every attempt. Resetting the delay on connect alone would pin the
/// retry at [`RETRY_MIN_MS`] forever, which is the case the bound exists for.
///
/// Thirty seconds because a session that has survived that has completed its
/// announcement and at least one keepalive round trip.
const STABLE_SESSION_MS: u32 = 30_000;

/// One consecutive connection failure in this many is logged, after the first
/// and after any change in the retry delay. See [`crate::net`] for why a log
/// line is not free.
const RETRY_LOG_INTERVAL: u32 = 10;

/// How long a socket operation may stall before the stack calls the peer gone.
///
/// Without it a broker that vanishes without closing its socket — a power cut,
/// a cable pulled — leaves this task waiting on a read that will never
/// complete, and the reconnect loop never runs. `minimq`'s own keepalive
/// detects a silent broker too, but only once it has managed to *write* a
/// PINGREQ; this covers the case where the write itself never completes.
/// It **must exceed [`KEEPALIVE_S`]**, and by enough to cover a late PINGREQ.
/// A socket timeout shorter than the keepalive does not detect a dead broker
/// sooner; it kills a session the protocol considers perfectly healthy, on a
/// timer, forever. Observed against a real broker on 2026-08-17 at 20 s
/// against a 60 s keepalive: CONNACK, then `Mqtt(Transport)` at 20226, 20083
/// and 20083 ms — the reconnect loop working exactly as designed on a fault
/// that was ours.
const SOCKET_TIMEOUT_S: u64 = 90;

/// MQTT keepalive advertised in CONNECT.
///
/// `minimq` drives PINGREQ itself from inside `recv`, so this is the interval
/// at which a dead broker becomes visible. Sixty seconds is the protocol's own
/// common default.
const KEEPALIVE_S: u16 = 60;

// The two constants above are only correct relative to each other, and nothing
// else ties them together: the transport layer and the protocol layer each look
// reasonable read alone. Inverting them costs no build error and no link error
// — the session simply dies on a timer and reconnects forever, which reads as a
// flaky broker rather than as a configuration fault. 1.5x is the same ratio a
// broker applies to its own keepalive before declaring a client gone.
const _: () = assert!(
    SOCKET_TIMEOUT_S >= (KEEPALIVE_S as u64) * 3 / 2,
    "the socket timeout must outlast the keepalive, or the transport kills \
     healthy sessions on a timer"
);

/// How often the controller's own diagnostics are republished.
///
/// They are what an operator watches over hours rather than seconds — a heap
/// that is creeping, a signal that is falling — so the interval is chosen
/// against the cost rather than against any need for immediacy. Each tick is
/// [`DeviceEntity::ALL`] publishes and one round trip each; at a minute that is
/// five packets a minute against a shade in motion's twenty a second.
///
/// It is also what makes uptime mean anything: published only on announcement,
/// the figure would freeze at whatever it was when the session came up.
///
/// It equals [`KEEPALIVE_S`], and the coincidence is harmless in the useful
/// direction: a tick is five publishes each settled with a `poll`, so the
/// session is driven by real traffic within every keepalive window and PINGREQ
/// becomes rare rather than starved. Real traffic is the better liveness proof
/// of the two.
const DIAGNOSTIC_INTERVAL_S: u64 = 60;

/// Bytes a rendered diagnostic reading may occupy.
///
/// The widest is `u64::MAX` at 20 digits; a negative signal strength is 11 at
/// most. 24 covers both and is the size of a pointer pair.
const READING_CAPACITY: usize = 24;

/// Whether this firmware advertises a tilt axis. **It does not, deliberately.**
///
/// `somfy-domain` carries tilt modes without implementing them — no command
/// drives a tilt axis and `Shade::tilt_pos` always reports `Pos::ZERO` — so
/// publishing tilt topics would give Home Assistant a slider that reads 0
/// forever and moves nothing. That is the "appearing is not working" failure
/// the requirements spec's own acceptance criterion calls out, and it is worse
/// than an absent control because it reads as a device fault. Recorded as a
/// deliberate deviation from R8 in `docs/provenance.md`.
const HAS_TILT: bool = false;

/// Everything one broker session works from: what was provisioned, and what it
/// has learned since.
///
/// A struct rather than a handful of arguments because these travel together
/// through every layer of this module, and because the borrow checker is
/// friendlier to disjoint fields of one value than to a long parameter list
/// that has to be re-threaded at each call.
pub struct Broker {
    /// The namespaces in use now.
    config: MqttConfig,
    /// Namespaces this device published under before and does not now. Their
    /// retained topics are cleared before the current ones are published; see
    /// spec R5, and `somfy_mqtt::reconfigure`, which is the only way to ask for
    /// the two halves in that order.
    stale: Vec<MqttConfig, MAX_SUPERSEDED>,
    /// The shades to announce, copied at boot and kept current by
    /// [`ShadeEvent`]s from the state task.
    inventory: Inventory,
    /// Ids that were announced and no longer exist.
    ///
    /// **The only thing that can name them.** A removed shade's retained
    /// discovery config outlives it on the broker, and clearing it needs an id
    /// nothing else in the system remembers — so the id is carried here, from
    /// the persisted `announced` set, until the tombstones have landed and the
    /// state task has been told it may forget.
    orphans: Vec<ShadeId, MAX_SHADES>,
    /// The last state observed for each shade, so a fresh broker session can be
    /// given it without waiting for the next change.
    known: Known,
    /// What the controller reports about itself. See [`Diagnostics`].
    diagnostics: Diagnostics,
    /// The one discovery-payload buffer. One, because only one config is
    /// rendered at a time and a kilobyte is not free on the tightest chip here.
    payload: String<PAYLOAD_CAPACITY>,
    /// Whether the MQTT-version observation has already been logged. It is a
    /// fact about the broker, not about the session, so it is said once.
    version_logged: bool,
    /// How often the two broker-driven log lines have fired. See [`Rare`].
    rare: Rare,
    /// Where "the entities are on/off the broker" goes back to the state task,
    /// which is what persists it. See `crate::edits`.
    acks: AckSender,
}

/// Steps one shade's retirement costs: a discovery config per member of
/// `somfy_mqtt::SHADE_COMPONENTS`, plus one per published topic.
///
/// Collected rather than walked lazily, because the plan borrows the config and
/// each step needs `&mut self` to execute. Ten is comfortably above the seven
/// the current entity set produces, and a plan that outgrew it would silently
/// clear fewer topics than it announced — so the assertion below is the check
/// rather than the constant.
const RETIRE_STEPS: usize = 10;

/// Steps one shade's announcement costs: a discovery config per component it
/// owns, plus one subscription per command topic.
const ANNOUNCE_STEPS: usize = 10;

impl Broker {
    /// Assemble a session from what boot found.
    ///
    /// The broker's own address and credentials are deliberately **not** here.
    /// They are needed once, to open a socket and to build the CONNECT, and
    /// keeping them out means nothing that runs per session holds the password
    /// — nor borrows it for the lifetime of the client, which is what a
    /// `minimq` session does with the value it is given.
    pub fn new(
        config: MqttConfig,
        stale: Vec<MqttConfig, MAX_SUPERSEDED>,
        inventory: Inventory,
        orphans: Vec<ShadeId, MAX_SHADES>,
        survey: Survey,
        acks: AckSender,
    ) -> Broker {
        let known = Known::new(&inventory);
        Broker {
            config,
            stale,
            inventory,
            orphans,
            known,
            diagnostics: Diagnostics {
                rollcode_damaged: survey.damaged,
            },
            payload: String::new(),
            version_logged: false,
            rare: Rare::default(),
            acks,
        }
    }
}

/// What the controller reports about **itself**, and where each figure comes
/// from.
///
/// | entity | source |
/// |---|---|
/// | uptime | [`Instant::now`], which counts from the time driver starting at boot |
/// | Wi-Fi signal | [`crate::net::signal_dbm`], sampled by the link task |
/// | free heap | [`crate::heap::free_bytes`] |
/// | peak heap use | [`crate::heap::peak_bytes`] |
/// | damaged rolling-code slots | the boot survey, carried here |
///
/// Everything but the last is read at the moment it is published, so nothing
/// here has to be kept up to date. The rolling-code figure is the exception and
/// is a **snapshot of the region as it was at boot**: the store belongs to the
/// state task from the moment it is handed over, and re-surveying it would mean
/// reaching across the boundary that keeps a broker from being able to affect
/// radio control. A slot damaged after boot is therefore reported at the next
/// one — which is the same latency an operator reading the serial line has, and
/// `docs/provenance.md` records the condition for improving it.
struct Diagnostics {
    /// Slots in the rolling-code region that were neither valid nor blank at
    /// boot.
    rollcode_damaged: usize,
}

impl Diagnostics {
    /// The current reading for one entity, or `None` when there is nothing
    /// honest to report.
    ///
    /// `None` is not a failure and it is not rendered as one: the caller
    /// publishes nothing. The value would go out **retained**, so a placeholder
    /// would outlive the boot that produced it and be handed to every later
    /// subscriber — the confidently-wrong retained value this whole integration
    /// is written around. Home Assistant shows an entity with no state as
    /// unknown, which is what it is.
    fn reading(&self, entity: DeviceEntity) -> Option<String<READING_CAPACITY>> {
        let mut out: String<READING_CAPACITY> = String::new();
        let written = match entity {
            DeviceEntity::Uptime => write!(&mut out, "{}", Instant::now().as_secs()),
            // The only one that can be absent: the link has not come up yet, or
            // the driver could not answer. See `net::SIGNAL_DBM`.
            DeviceEntity::WifiSignal => write!(&mut out, "{}", crate::net::signal_dbm()?),
            DeviceEntity::HeapFree => write!(&mut out, "{}", crate::heap::free_bytes()),
            DeviceEntity::HeapPeak => write!(&mut out, "{}", crate::heap::peak_bytes()),
            DeviceEntity::RollcodeDamaged => write!(&mut out, "{}", self.rollcode_damaged),
        };
        // Unreachable — `READING_CAPACITY` holds every one of these — and
        // treated as "nothing to report" rather than published half-written,
        // because a truncated number is a plausible wrong number.
        written.ok().map(|()| out)
    }
}

/// Everything one operation needs beyond the connection and the step itself.
///
/// A struct rather than five arguments because [`perform`] is the **only**
/// function in this module that puts anything on the wire, so every path
/// reaches it and every one of them would otherwise repeat the list. Its fields
/// are disjoint borrows of [`Broker`], which is what lets a plan that reads
/// `config` and `inventory` be walked while `payload` and `rare` are written.
struct Wire<'a> {
    config: &'a MqttConfig,
    inventory: &'a Inventory,
    commands: &'a CommandSender,
    payload: &'a mut String<PAYLOAD_CAPACITY>,
    rare: &'a mut Rare,
}

/// What ended a wait in the session loop.
///
/// An enum rather than three branches doing their work in place, because two of
/// them publish and an inbound message holds the connection borrowed while it is
/// read. Deciding first and acting after is what gives the borrow back.
enum Woken {
    /// The state task reported a shade.
    Delta(StateDelta),
    /// The diagnostic interval elapsed.
    Diagnostics,
    /// The state task added or removed a shade.
    Shade(ShadeEvent),
}

/// Bring up the broker session.
///
/// Returns a `SpawnError` and nothing else; the caller reports it and carries
/// on without MQTT, exactly as it does for Wi-Fi. There is no failure here that
/// stops the controller.
#[allow(
    clippy::too_many_arguments,
    reason = "every one is a distinct thing boot found, and a struct for them \
    would be a type constructed at one call site and destructured at the next"
)]
pub fn start(
    spawner: embassy_executor::Spawner,
    stack: Stack<'static>,
    settings: MqttSettings,
    superseded: Vec<Namespaces, MAX_SUPERSEDED>,
    inventory: Inventory,
    orphans: Vec<ShadeId, MAX_SHADES>,
    survey: Survey,
    commands: CommandSender,
    deltas: DeltaSubscriber,
    events: EventReceiver,
    acks: AckSender,
) -> Result<(), embassy_executor::SpawnError> {
    let device_id = device_id();
    let config = match topic_config(
        settings.discovery_prefix(),
        settings.state_root(),
        &device_id,
    ) {
        Ok(config) => config,
        Err(error) => {
            // Unreachable through the provisioning path — `MqttSettings::new`
            // has already refused everything this can report — but reported
            // rather than `expect`ed, because a panic here reboots the board,
            // and it would do so on every boot.
            esp_println::println!(
                "mqtt: stored settings are not a usable topic configuration ({}) \
                 — running without a broker",
                error,
            );
            return Ok(());
        }
    };

    // Every superseded namespace pair becomes a configuration whose retained
    // topics are cleared before the current ones are published. A pair that is
    // no longer a valid configuration is skipped with a line rather than
    // stopping the rest: it cannot be cleared, and saying so is more use than
    // refusing to announce anything.
    let mut stale: Vec<MqttConfig, MAX_SUPERSEDED> = Vec::new();
    for old in &superseded {
        match topic_config(old.discovery_prefix(), old.state_root(), &device_id) {
            Ok(config) => {
                let _ = stale.push(config);
            }
            Err(error) => esp_println::println!(
                "mqtt: cannot clear the retained topics under '{}'/'{}' ({})",
                old.discovery_prefix(),
                old.state_root(),
                error,
            ),
        }
    }

    let broker = Broker::new(config, stale, inventory, orphans, survey, acks);
    spawner.spawn(session(stack, settings, broker, commands, deltas, events)?);
    Ok(())
}

/// The stable identifier every `unique_id` is built from.
///
/// The factory MAC, hex-encoded without separators. It has to survive a reboot,
/// a configuration change and a firmware update: an entity whose `unique_id`
/// changes is a *new* entity to Home Assistant, and the old one stays behind as
/// an orphan with every automation and dashboard card still pointing at it.
///
/// The eFuse MAC is the only value on this board that satisfies all three. It is
/// not a secret — it is in every frame the Wi-Fi radio transmits — and it is not
/// derived from anything a user edits.
fn device_id() -> String<12> {
    use core::fmt::Write as _;

    let mut out = String::new();
    for byte in esp_hal::efuse::base_mac_address().as_bytes() {
        // Cannot fail: six bytes at two hex digits each is exactly the capacity.
        // `write!` rather than a lookup table because a truncated device id is
        // two devices silently sharing an identity.
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Assemble a topic configuration from two namespaces and this device's id.
///
/// The node id is the device id as well: Home Assistant ignores it, but it makes
/// this device's own configs findable with
/// `mosquitto_sub -t 'homeassistant/+/<id>/#'` on a broker shared with other
/// integrations.
fn topic_config(
    discovery_prefix: &str,
    state_root: &str,
    device_id: &str,
) -> Result<MqttConfig, ConfigError> {
    MqttConfig::new(
        DiscoveryPrefix::new(discovery_prefix)?,
        StateRoot::new(state_root)?,
        NodeId::new(device_id)?,
        DeviceId::new(device_id)?,
    )
}

/// Connect, announce, serve, and reconnect — forever.
///
/// The outer loop is R9's bounded backoff. The inner one is the session.
#[embassy_executor::task]
async fn session(
    stack: Stack<'static>,
    settings: MqttSettings,
    mut broker: Broker,
    commands: CommandSender,
    mut deltas: DeltaSubscriber,
    events: EventReceiver,
) -> ! {
    // Declared before the session below, so they outlive the borrows it takes
    // of them. Locals of a `#[task]` body live in the task's statically
    // allocated future, not on the main stack.
    let mut socket_rx = [0u8; TCP_RX_BYTES];
    let mut socket_tx = [0u8; TCP_TX_BYTES];
    let mut mqtt_rx = [0u8; MQTT_RX_BYTES];
    let mut mqtt_tx = [0u8; MQTT_TX_BYTES];

    let will = broker.config.will();
    let endpoint = SocketAddrV4::new(settings.address(), settings.port());

    // The client id is derived from the device id, so it is stable across
    // reboots and firmware updates. A broker that sees two clients with the
    // same id disconnects the older one, so an unstable id would look like a
    // broker repeatedly dropping this device.
    let mut builder = ConfigBuilder::new(Buffers::new(&mut mqtt_rx, &mut mqtt_tx))
        .client_id(broker.config.device_id().as_str())
        .expect("a device id is at most 32 bytes and the client id holds 64")
        .keepalive_interval(KEEPALIVE_S)
        // **A clean session, stated rather than inherited from the default.**
        //
        // A session that survives disconnection is one the broker *queues for*:
        // every QoS 1 command published while this device is away is delivered
        // the moment it comes back. That is R6's failure with a different
        // mechanism — a shade acting on an hour-old instruction after a broker
        // restart — and it is worse than the retained-command version, because
        // several can arrive at once and the device has no way to tell them
        // from fresh ones.
        //
        // The cost is that [`ConnectEvent::Reconnected`] becomes rare: with
        // nothing kept, the broker starts a fresh session and the whole
        // announcement runs again. That is a handful of retained publishes per
        // reconnect, all idempotent. The branch is still handled, because a
        // broker may resume a session for its own reasons and re-announcing
        // when it did not have to is waste, not breakage.
        .session_expiry_interval(0);

    // **R5's will, registered in CONNECT and nowhere else.** This is the only
    // moment a client can hand the broker something to say on its behalf, and
    // the case it exists for is the one where this device is no longer able to
    // say anything.
    builder = builder
        .will(
            Will::new(will.topic().as_str(), will_payload(&will), &[])
                .expect("the availability topic is at most 71 bytes and a will topic holds 128")
                .retained(),
        )
        .expect("the will is set exactly once");

    if !settings.is_anonymous() {
        builder = builder
            .auth(settings.username(), settings.password().as_bytes())
            .expect("auth is set exactly once");
    }

    let mut session = Session::new(builder);

    let mut backoff = Backoff::new(RETRY_MIN_MS, RETRY_MAX_MS);
    let mut consecutive = 0u32;
    let mut previous_delay = 0u32;

    loop {
        // **The guard Task 2 named.** A station can be associated with no DHCP
        // lease, which looks identical from the Wi-Fi side and is the state in
        // which nothing works; connecting a socket then fails immediately and
        // burns a backoff step for a network condition that is about to fix
        // itself.
        stack.wait_config_up().await;

        let mut socket = TcpSocket::new(stack, &mut socket_rx, &mut socket_tx);
        socket.set_timeout(Some(Duration::from_secs(SOCKET_TIMEOUT_S)));

        let started = Instant::now();
        let outcome = match socket.connect(endpoint).await {
            Ok(()) => {
                broker
                    .serve(&mut session, socket, &commands, &mut deltas, &events)
                    .await
            }
            Err(error) => {
                // The endpoint is printed; the credentials never are, and
                // `MqttSettings`' `Debug` redacts the password in any case.
                Err(SessionEnd::Tcp(error))
            }
        };

        let lasted = started.elapsed().as_millis().min(u32::MAX as u64) as u32;
        if backoff.succeed_after(lasted, STABLE_SESSION_MS) {
            consecutive = 0;
        } else {
            consecutive = consecutive.saturating_add(1);
        }

        // Rate-limited for the same reason `net` rate-limits its own: a broker
        // that is switched off would otherwise produce a line a second for as
        // long as the device runs, each one written inside a critical section.
        if consecutive <= 1
            || backoff.delay_ms() != previous_delay
            || consecutive.is_multiple_of(RETRY_LOG_INTERVAL)
        {
            match &outcome {
                Ok(()) => {
                    esp_println::println!("mqtt: session at {} ended after {} ms", endpoint, lasted,)
                }
                Err(end) => esp_println::println!(
                    "mqtt: session at {} ended after {} ms — {:?} ({} in a row)",
                    endpoint,
                    lasted,
                    end,
                    consecutive,
                ),
            }
        }

        let waiting = backoff.fail();
        if waiting != previous_delay {
            esp_println::println!("mqtt: reconnecting in {} ms", waiting);
        }
        previous_delay = waiting;
        Timer::after(Duration::from_millis(waiting as u64)).await;
    }
}

impl Broker {
    /// One broker session, from CONNECT to whatever ends it.
    ///
    /// Fields are reached through `self` one at a time rather than destructured
    /// up front: the plans read `config`, `stale` and `inventory` while
    /// `payload` and `known` are written, and disjoint fields of one value is
    /// exactly the borrow the compiler accepts.
    async fn serve<'buf>(
        &mut self,
        session: &mut Session<'buf>,
        socket: TcpSocket<'_>,
        commands: &CommandSender,
        deltas: &mut DeltaSubscriber,
        events: &EventReceiver,
    ) -> Result<(), SessionEnd> {
        let mut connection = session.connect(socket).await.map_err(SessionEnd::mqtt)?;
        let event = connection.connect_event();

        // The observation, not the inference. See this module's docs. Said once
        // per boot, because it is a fact about the broker rather than about
        // this session.
        if !self.version_logged {
            esp_println::println!(
                "mqtt: broker accepted an MQTT v5 CONNECT and answered CONNACK ({:?})",
                event,
            );
            self.version_logged = true;
        }

        match event {
            // A fresh broker session: the broker kept nothing, so subscriptions
            // are gone and its retained store may have been rebuilt from
            // scratch. That also makes a broker restart indistinguishable from
            // a first connect, which is exactly why the whole announcement runs
            // here and not on every reconnect.
            ConnectEvent::Connected => self.resync(&mut connection, commands).await?,
            // A resumed session: subscriptions and in-flight QoS state survived,
            // so re-announcing the entities would be a broker's worth of
            // retained publishes for no change. Availability still goes out,
            // because the will may have fired while this device was away and
            // left `offline` retained.
            // **The retirement is not conditional on the event.** A superseded
            // namespace still has orphans under it whether or not the broker
            // resumed the session, and R5's obligation is about what is
            // retained on the broker rather than about what this client's
            // session remembers. Skipping it here would make the rule depend on
            // a CONNACK flag — which, with `session_expiry_interval(0)`, is a
            // branch nothing takes today and would silently turn the rule off
            // for whoever raises it.
            ConnectEvent::Reconnected if !self.stale.is_empty() => {
                // **`resync`, not `announce`.** A superseded configuration that
                // shares the state root has its state topics tombstoned by the
                // retirement, and those are the topics the current
                // configuration publishes to. Announcing without republishing
                // would leave every position and every reading cleared on the
                // broker until the next change — which, for a shade nobody
                // touches, may be days. That the two cannot be asked for
                // separately is the point of `resync` existing.
                //
                // `online` is not published separately on this path: it is the
                // first step of the plan `resync` walks, and publishing it here
                // as well would buy nothing for an extra settled round trip.
                self.resync(&mut connection, commands).await?;
            }
            ConnectEvent::Reconnected => {
                let online = Step::Send(self.config.online());
                self.perform_one(&mut connection, &online, commands).await?;
            }
        }

        // **The one place the heap's high-water mark is worth reading**, and
        // the reason it is here rather than anywhere else: the announcement
        // above is the burst of retained discovery configs that produces the
        // peak `crate::heap::RADIO_HEAP_BYTES` is sized against, it is reached
        // within a second of CONNACK, and it never moves again. Reported at
        // `heap::install_for_radio`'s two earlier call sites — "controller
        // started" and "network up" — the figure is the one *before* the load
        // that sizes the constant, which is how a serial console came to
        // disagree with the constant it was supposed to justify.
        //
        // Nowhere near the frame path, and once per session rather than once
        // per reconnect loop iteration.
        crate::heap::report("session announced");

        // Created once, outside the loop, so the interval is a schedule rather
        // than a delay restarted by every delta and every inbound command. A
        // `Timer` inside the loop would push the next diagnostic publish out by
        // a full interval each time a shade moved.
        let mut diagnostics = Ticker::every(Duration::from_secs(DIAGNOSTIC_INTERVAL_S));

        loop {
            // Three inputs in one wait. All three futures are cancel-safe: the
            // delta subscriber advances its cursor only on `Poll::Ready`,
            // `minimq`'s `recv` is documented as cancel-safe, and
            // `Ticker::next` keeps its deadline in the ticker rather than in
            // the future — so whichever loses is simply dropped.
            //
            // The inbound branch is handled *inside* the match and yields
            // nothing borrowed: an `InboundPublish` borrows the connection
            // mutably, and the publishes below need that borrow back.
            let woken = match select4(
                connection.recv(),
                deltas.next_message(),
                diagnostics.next(),
                events.receive(),
            )
            .await
            {
                Either4::First(inbound) => {
                    let inbound = inbound.map_err(SessionEnd::mqtt)?;
                    dispatch(
                        &mut Wire {
                            config: &self.config,
                            inventory: &self.inventory,
                            commands,
                            payload: &mut self.payload,
                            rare: &mut self.rare,
                        },
                        inbound.topic(),
                        inbound.payload(),
                    );
                    None
                }
                Either4::Second(WaitResult::Message(delta)) => Some(Woken::Delta(delta)),
                // The subscriber fell behind and the channel dropped deltas for
                // it. Worth one line: it means this task was blocked long enough
                // for the state task to publish `DELTA_QUEUE_DEPTH` updates, so
                // the position now on the broker is behind the shade.
                Either4::Second(WaitResult::Lagged(missed)) => {
                    esp_println::println!("mqtt: fell behind, {} state updates dropped", missed);
                    None
                }
                Either4::Third(()) => Some(Woken::Diagnostics),
                Either4::Fourth(event) => Some(Woken::Shade(event)),
            };

            match woken {
                Some(Woken::Shade(ShadeEvent::Added { id, name, pairable })) => {
                    self.inventory.insert(
                        id,
                        &name,
                        if pairable {
                            Pairing::Offered
                        } else {
                            Pairing::Withheld
                        },
                    );
                    self.known.track(id);
                    self.announce_one(&mut connection, id, commands).await?;
                }
                // **The entities go before the shade is forgotten.** The state
                // task has already removed it from the registry and written the
                // record with its announced bit still set, so the id survives a
                // power cut here; `retire` acknowledges only once the broker
                // has confirmed every tombstone.
                Some(Woken::Shade(ShadeEvent::Removed { id })) => {
                    self.retire(&mut connection, id, commands).await?;
                    self.inventory.remove(id);
                    self.known.forget(id);
                }
                Some(Woken::Delta(delta)) => {
                    self.known.record(&delta);
                    for state in self.known.of(delta.id) {
                        let publish = Step::Send(self.config.state(
                            state.id,
                            state.topic,
                            state.value.as_bytes(),
                        ));
                        self.perform_one(&mut connection, &publish, commands)
                            .await?;
                    }
                }
                Some(Woken::Diagnostics) => {
                    self.publish_diagnostics(&mut connection, commands).await?;
                }
                None => {}
            }
        }
    }

    /// Everything this device says when it takes ownership of its topics:
    /// clear what a superseded configuration left, announce the current one,
    /// then republish every retained value the announcement did not carry.
    ///
    /// **The three are one method because the third depends on the first.** A
    /// superseded configuration that shares the state root has its state topics
    /// tombstoned by the retirement — and those are the very topics the current
    /// configuration publishes to, so an announcement without a republish
    /// leaves them cleared on the broker until something changes. For a shade
    /// nobody touches that is days. Offering the halves separately is what would
    /// let a caller take one and not the other.
    ///
    /// It is also R9's "republish retained state on reconnect" in its own
    /// right: a fresh broker session may have lost its retained store entirely.
    async fn resync<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        self.announce(connection, commands).await?;
        self.publish_shade_state(connection, commands).await?;
        // A diagnostic whose first publish waited for the next tick would show
        // as unknown in Home Assistant for up to `DIAGNOSTIC_INTERVAL_S` after
        // every reconnect.
        self.publish_diagnostics(connection, commands).await
    }

    /// Every shade's last observed state, republished retained.
    async fn publish_shade_state<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        // Collected first because `known.ids()` borrows `self` and the publish
        // below needs it back. `MAX_SHADES` ids is 32 bytes.
        let ids: Vec<ShadeId, MAX_SHADES> = self.known.ids().collect();
        for id in ids {
            for state in self.known.of(id) {
                let publish = Step::Send(self.config.state(
                    state.id,
                    state.topic,
                    state.value.as_bytes(),
                ));
                self.perform_one(connection, &publish, commands).await?;
            }
        }
        Ok(())
    }

    /// The controller's own readings, retained, one per [`DeviceEntity`].
    ///
    /// An entity with nothing to report publishes **nothing** rather than a
    /// placeholder — see [`Diagnostics::reading`]. Home Assistant shows it as
    /// unknown, which is what it is, and the next tick fills it in as soon as
    /// there is something to say.
    async fn publish_diagnostics<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        for entity in DeviceEntity::ALL {
            let Some(reading) = self.diagnostics.reading(entity) else {
                continue;
            };
            let publish = Step::Send(self.config.device_state(entity, reading.as_bytes()));
            self.perform_one(connection, &publish, commands).await?;
        }
        Ok(())
    }

    /// One step, settled — the shape every caller that is not walking a plan
    /// uses. See [`perform`] for why settling is not optional.
    async fn perform_one<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        step: &Step<'_>,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        perform(
            connection,
            step,
            &mut Wire {
                config: &self.config,
                inventory: &self.inventory,
                commands,
                payload: &mut self.payload,
                rare: &mut self.rare,
            },
        )
        .await
    }

    /// Clear whatever a superseded configuration left behind, then publish the
    /// current one.
    ///
    /// The two halves are never asked for separately: `reconfigure` is the only
    /// way to obtain them together and it emits the tombstones first, which is
    /// the ordering R5 requires. Publishing the new configs first would leave
    /// Home Assistant holding both sets for as long as the retirement took.
    async fn announce<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        if !self.stale.is_empty() {
            esp_println::println!(
                "mqtt: clearing the retained topics of {} superseded configuration(s)",
                self.stale.len(),
            );
        }
        // One call, whatever `stale` holds — an empty slice reduces to a plain
        // announcement. Looping here instead would announce the current
        // configuration once per superseded one, which is a broker's worth of
        // retained publishes repeated for no change.
        // Every step through `perform`, which settles. See it for why that is
        // not optional: an announcement now costs `1 + 5N + k` operations for
        // `N` shades and `k` device entities, and `minimq` holds eight.
        {
            // Captured by value as a bitmap, not borrowed: the plan's closure
            // has to outlive a `&mut Wire` that already borrows the inventory,
            // and a `u32` sidesteps that entirely. One bit per registry slot,
            // which is the same shape the persisted announced set has and for
            // the same reason.
            let pairable = self.pairable_bits();
            let mut wire = Wire {
                config: &self.config,
                inventory: &self.inventory,
                commands,
                payload: &mut self.payload,
                rare: &mut self.rare,
            };
            for step in reconfigure(
                &self.stale,
                &self.config,
                self.inventory.ids(),
                HAS_TILT,
                move |id| pairing_of(pairable, id),
            ) {
                perform(connection, &step, &mut wire).await?;
            }
        }

        // The shade's own name, which no plan can carry because `somfy-mqtt`
        // does not hold names. `ShadeTopic::Name` is a published topic and the
        // retirement clears it, so leaving it unpublished would be exactly the
        // publisher/model drift `ShadeTopic` exists to prevent — the retirement
        // would tombstone an address nothing had ever written.
        for index in 0..self.inventory.len() {
            // By index rather than over `inventory.ids()`, because the name is
            // borrowed from the inventory and `perform_one` takes `&mut self`.
            // Copied into a local for the same reason; a name is 32 bytes.
            let Some(shade) = self.inventory.ids().get(index).copied() else {
                break;
            };
            let Some(held) = self.inventory.name(shade) else {
                continue;
            };
            let mut name: String<{ somfy_mqtt::MAX_NAME_LEN }> = String::new();
            if name.push_str(held).is_err() {
                // **Reported and skipped, not published.** `heapless`'
                // `push_str` is all-or-nothing, so a failure here would leave
                // `name` empty — and an empty payload on a retained publish is
                // a *tombstone*, the exact bytes `somfy_mqtt::tombstone` uses
                // to remove a topic. Publishing it would delete the shade's
                // name rather than leave the old one, which is a silent
                // deletion dressed as a truncation.
                //
                // Unreachable while `somfy_mqtt::MAX_NAME_LEN` and
                // `Inventory`'s own capacity are the same figure, which they
                // are; nothing ties them together, so the branch is here.
                esp_println::println!(
                    "mqtt: shade {}'s name does not fit its buffer — \
                     leaving whatever the broker holds rather than clearing it",
                    shade.0,
                );
                continue;
            }
            let published =
                PublishedTopic::of(ShadeTopic::Name).expect("a shade's name is published");
            let publish = Step::Send(self.config.state(shade, published, name.as_bytes()));
            self.perform_one(connection, &publish, commands).await?;
        }

        // Cleared only once every tombstone has been acknowledged. Without
        // this, every fresh session republishes the whole retirement forever —
        // harmless in itself, and `5N + 1` more operations in front of an
        // announcement that is already the tightest thing this task does.
        //
        // The ring still holds the superseded records, so a reboot brings them
        // back and repeats the retirement once more. That is idempotent and it
        // is the honest cost of not rewriting the configuration region from the
        // network path — a region whose other half holds Wi-Fi credentials.
        // **The orphans, after the live configuration and not before it.**
        // These are shades that were announced and have since been removed, and
        // their retained discovery configs are on the broker with nothing
        // behind them; clearing them is what `retire_shade` was written for and
        // has never had a caller for.
        //
        // Their topics belong to ids no live shade holds, so nothing above
        // publishes to them and the order is free — which makes it a latency
        // question, and there `online` and the covers win. A board with several
        // orphans would otherwise hold availability behind seven tombstones
        // each.
        //
        // Retried on every fresh session until the state task acknowledges each
        // one, which it does only once these have settled — so a power cut here
        // costs a repeat, and the id survives in flash either way.
        if !self.orphans.is_empty() {
            esp_println::println!(
                "mqtt: clearing the entities of {} removed shade(s)",
                self.orphans.len(),
            );
        }
        let orphans: Vec<ShadeId, MAX_SHADES> = self.orphans.clone();
        for id in orphans {
            self.retire(connection, id, commands).await?;
        }

        self.stale.clear();
        Ok(())
    }

    /// Which shades own a pairing button, as one bit per registry slot.
    fn pairable_bits(&self) -> u32 {
        self.inventory
            .ids()
            .iter()
            .filter(|id| matches!(self.inventory.pairing(**id), Pairing::Offered))
            .fold(0u32, |bits, id| bits | slot_bit(*id))
    }

    /// Clear everything the broker holds for one shade, then tell the state
    /// task it may forget that the entities ever existed.
    ///
    /// **The acknowledgement is sent after the tombstones have settled, never
    /// before.** `perform` settles each step, so by the time this returns the
    /// broker has acknowledged every removal. Clearing the persisted bit first
    /// would mean a power cut between the two lost the only record that the
    /// entities are there — which is the failure the bit exists to prevent, one
    /// step further along.
    async fn retire<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        id: ShadeId,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        // Collected first because the plan borrows `self.config` and
        // `perform_one` takes `&mut self`. A shade's retirement is seven steps.
        let steps: Vec<Step<'static>, RETIRE_STEPS> = self.config.retire_shade(id).collect();
        for step in &steps {
            self.perform_one(connection, step, commands).await?;
        }
        self.acks.send(ShadeAck::Retired { id }).await;
        self.orphans.retain(|held| *held != id);
        Ok(())
    }

    /// Announce one shade that has just been added, without re-announcing the
    /// rest.
    async fn announce_one<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        id: ShadeId,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        let pairing = self.inventory.pairing(id);
        let steps: Vec<Step<'static>, ANNOUNCE_STEPS> =
            self.config.announce_shade(id, HAS_TILT, pairing).collect();
        for step in &steps {
            self.perform_one(connection, step, commands).await?;
        }

        // The name, which no plan can carry because `somfy-mqtt` does not hold
        // names. Same reasoning as in `announce`, including why a name that
        // does not fit is skipped rather than published: an empty retained
        // payload is a tombstone.
        if let Some(held) = self.inventory.name(id) {
            let mut name: String<{ somfy_mqtt::MAX_NAME_LEN }> = String::new();
            if name.push_str(held).is_ok() {
                let published =
                    PublishedTopic::of(ShadeTopic::Name).expect("a shade's name is published");
                let publish = Step::Send(self.config.state(id, published, name.as_bytes()));
                self.perform_one(connection, &publish, commands).await?;
            }
        }

        self.acks.send(ShadeAck::Announced { id }).await;
        Ok(())
    }
}

/// The bit `id` occupies in a per-registry-slot bitmap.
///
/// Zero for an id past the registry, which is not reachable — every id comes
/// from the registry — and is a shift that would otherwise be undefined.
fn slot_bit(id: ShadeId) -> u32 {
    if (id.0 as usize) < MAX_SHADES {
        1u32 << id.0
    } else {
        0
    }
}

/// Read one shade's pairing status out of the bitmap.
fn pairing_of(bits: u32, id: ShadeId) -> Pairing {
    if bits & slot_bit(id) != 0 {
        Pairing::Offered
    } else {
        Pairing::Withheld
    }
}

/// Carry out one [`Step`] and wait for the broker to acknowledge it.
///
/// **This is the only function in this module that puts anything on the wire,
/// and it always settles.** That is the whole point of its existing: the
/// settling rule below is not one a call site can be trusted to remember, and
/// Task 4's larger entity set made forgetting it cheaper to do and more
/// expensive to suffer.
///
/// # Why settling is not an optimisation
///
/// `minimq` keeps a QoS 1 publish or a subscribe in its retained slots — there
/// are **eight** — until the broker's acknowledgement is *read*, and reading
/// happens only inside `recv`, `poll` or `drive`. Publishing does not read:
/// `publish` and `subscribe` flush the outbound direction and return. So a plan
/// walked without ever reading exhausts the slots and fails at the ninth
/// operation with `InflightExhausted`, and then does the same on every
/// reconnect, at the backoff ceiling, forever.
///
/// An announcement costs `1 + 5N + k` operations for `N` shades and the `k = 5`
/// entries of `DeviceEntity::ALL` — `online`, then per shade a discovery config
/// for each entry of `somfy_mqtt::SHADE_COMPONENTS` (a cover and a pairing
/// button) and one subscription per command topic (direction, target, pair),
/// then one discovery config per device entity. The firmware follows it with
/// `N` names, `2N` state publishes and `k` readings, so a fresh session costs
/// `1 + 8N + 2k` in all. **That is eleven with no shades provisioned at all** —
/// the ordinary state of a freshly flashed board — where in Task 3 the same
/// burst was `1 + 6N` and needed two shades to exceed eight. The plan alone
/// crosses eight at one shade.
/// `somfy-mqtt/tests/lifecycle.rs::walking_a_plan_without_settling_runs_out_of_slots_partway`
/// is that failure, executed on the host against a model of the client, and
/// `an_announcement_for_one_shade_already_exceeds_the_clients_inflight_slots`
/// pins the arithmetic so it cannot quietly fall back under the limit.
///
/// The retained packets also occupy [`MQTT_TX_BYTES`] until they are
/// acknowledged, and **two** unacknowledged discovery configs at the widest
/// configuration already overrun it, which is the tighter of the two ceilings.
///
/// Settling after each operation holds in-flight state at one, which makes both
/// ceilings unreachable rather than merely distant. The cost is one round trip
/// per operation, paid once per session and once per diagnostic interval.
async fn perform<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    step: &Step<'_>,
    wire: &mut Wire<'_>,
) -> Result<(), SessionEnd> {
    execute(connection, step, wire).await?;
    settle(connection, wire).await
}

/// Read the inbound direction until the broker has acknowledged everything
/// outstanding. See [`perform`], which is the only caller and the only reason
/// this is separate from it.
///
/// An inbound message that arrives while settling is a real command, so it is
/// acted on here rather than dropped: the subscriptions go out during the
/// announcement, and a person pressing a button does not wait for it to finish.
async fn settle<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    wire: &mut Wire<'_>,
) -> Result<(), SessionEnd> {
    while !connection.session().is_publish_quiescent() {
        // `poll` returns on any session progress, an acknowledgement included,
        // and it is the only thing that frees a retained slot.
        if let Some(inbound) = connection.poll().await.map_err(SessionEnd::mqtt)? {
            dispatch(wire, inbound.topic(), inbound.payload());
        }
    }
    Ok(())
}

/// Turn one inbound message into a command and hand it to the state task.
///
/// Shared by the session loop and by [`settle`] so that a command arriving
/// during an announcement is treated exactly like one arriving afterwards.
fn dispatch(wire: &mut Wire<'_>, topic: &str, payload: &[u8]) {
    let (config, inventory, commands, rare) =
        (wire.config, wire.inventory, wire.commands, &mut *wire.rare);
    match decode_command(config, inventory, topic, payload) {
        // `try_send`, never `send`: see this module's docs.
        Some(command) => {
            if commands.try_send(command).is_err() {
                report_rare(
                    &mut rare.dropped_commands,
                    "mqtt: command queue full, a command was dropped",
                );
            }
        }
        None => report_rare(
            &mut rare.unrecognised,
            "mqtt: a message arrived on a subscribed topic that is not a command this device knows",
        ),
    }
}

/// How often each of the two broker-driven log lines has fired.
///
/// Plain counters on the session rather than atomic statics, and the reason is
/// the matrix: **`riscv32imc` — the ESP32-C3's target — has no atomic
/// read-modify-write instruction**, so `AtomicU32::fetch_add` does not exist
/// there at all. Nothing needs synchronising either; the broker session is the
/// only thing that touches these.
#[derive(Default)]
struct Rare {
    /// Commands dropped because the state task's queue was full.
    dropped_commands: u32,
    /// Inbound messages that were not commands this device knows.
    unrecognised: u32,
}

/// Print `message` on the first occurrence and every [`RETRY_LOG_INTERVAL`]th
/// after it.
///
/// These two are the only log lines in this module whose rate a **remote peer**
/// sets, which is exactly why they need the bound the rest of the module
/// already has: `esp_println` writes each line inside a critical section, byte
/// by byte, and the radio task has about 5 ms to re-arm the receiver between a
/// frame and its repeat. A broker publishing steadily to a subscribed topic is
/// otherwise the one way something outside this house can reach into radio
/// timing.
///
/// The topic and the command are deliberately **not** in the message. Both are
/// attacker-influenced and both would make the line longer; the counter is what
/// an operator needs, and the topic set is fixed and knowable from the
/// announcement.
fn report_rare(counter: &mut u32, message: &str) {
    *counter = counter.saturating_add(1);
    if *counter == 1 || counter.is_multiple_of(RETRY_LOG_INTERVAL) {
        esp_println::println!("{} ({} so far)", message, counter);
    }
}

/// Carry out one [`Step`].
///
/// The retention is read off the step, never decided here. That is the whole
/// point of the plan being data: this function has no opinion about which
/// messages are retained, so it cannot hold a wrong one.
///
/// # Which failures end the session, and which do not
///
/// Only the ones reconnecting could fix. A transport or protocol failure ends
/// the session, because a fresh one is exactly the remedy. A **local** failure
/// — a payload that will not render, a component with no renderer, a shade the
/// inventory does not hold — is reported and skipped, because it would fail
/// identically on every reconnect: turning it into a session error would put
/// the device in a loop that reconnects every 60 seconds and publishes nothing
/// at all, losing every *other* entity over one that could not be built.
async fn execute<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    step: &Step<'_>,
    wire: &mut Wire<'_>,
) -> Result<(), SessionEnd> {
    let config = wire.config;
    let inventory = wire.inventory;
    let payload = &mut *wire.payload;
    match step {
        Step::Send(publish) => match publish.payload() {
            Payload::Discovery { shade, component } => {
                let Some(name) = inventory.name(shade) else {
                    // Unreachable: the plan is built from this inventory's own
                    // ids. Reported rather than panicked, because a panic here
                    // would reboot the board over a discovery config.
                    esp_println::println!(
                        "mqtt: shade {} is in the plan and not in the inventory — \
                         its {} entity is not published",
                        shade.0,
                        component.as_str(),
                    );
                    return Ok(());
                };
                // `render` leaves the buffer empty rather than half-written on
                // failure, so nothing partial can be sent — and a partial config
                // is truncated JSON, which Home Assistant discards without
                // saying so.
                let rendered = match component {
                    Component::Cover => config
                        .cover_discovery(shade, name, HAS_TILT)
                        .render(payload),
                    Component::Button => config.button_discovery(shade, name).render(payload),
                    // A shade owns an entity of each member of
                    // `SHADE_COMPONENTS`, and every member has an arm above, so
                    // this is unreachable. It is reported loudly rather than
                    // skipped in silence: an entity the plan announces and
                    // nothing publishes is exactly the half-configured state
                    // this integration exists to prevent.
                    other => {
                        esp_println::println!(
                            "mqtt: no payload renderer for a '{}' entity — \
                             shade {} will be missing one",
                            other.as_str(),
                            shade.0,
                        );
                        return Ok(());
                    }
                };
                if rendered.is_err() {
                    esp_println::println!(
                        "mqtt: the '{}' discovery config for shade {} does not fit \
                         its buffer — the entity will not appear",
                        component.as_str(),
                        shade.0,
                    );
                    return Ok(());
                }
                publish_bytes(
                    connection,
                    publish.topic().as_str(),
                    payload.as_bytes(),
                    publish.retention(),
                )
                .await
            }
            // R7's device-level entities. Rendered from the entity alone: what
            // it *reports* is published separately, on the topic this config
            // names.
            Payload::DeviceDiscovery(entity) => {
                if config.diagnostic_discovery(entity).render(payload).is_err() {
                    esp_println::println!(
                        "mqtt: the discovery config for '{}' does not fit its buffer — \
                         the entity will not appear",
                        entity.slug(),
                    );
                    return Ok(());
                }
                publish_bytes(
                    connection,
                    publish.topic().as_str(),
                    payload.as_bytes(),
                    publish.retention(),
                )
                .await
            }
            Payload::Bytes(bytes) => {
                publish_bytes(
                    connection,
                    publish.topic().as_str(),
                    bytes,
                    publish.retention(),
                )
                .await
            }
            // **The removal.** A zero-length payload with the retain flag set
            // is the only thing that deletes a retained message from a broker.
            Payload::Nothing => {
                publish_bytes(
                    connection,
                    publish.topic().as_str(),
                    &[],
                    publish.retention(),
                )
                .await
            }
        },
        Step::Listen(listen) => {
            let options = SubscriptionOptions::default()
                .maximum_qos(QoS::AtLeastOnce)
                // **R6's subscribe half.** A broker that already holds a
                // retained message on a command topic replays it to every new
                // subscriber, so a shade would act on whatever was last
                // commanded every time this device reconnected. `Never` is the
                // only defence a subscriber has, and the publisher cannot
                // supply it.
                .retain_behavior(if listen.retained_replay() {
                    RetainHandling::Immediately
                } else {
                    RetainHandling::Never
                });
            connection
                .subscribe(
                    &[TopicFilter::new(listen.topic().as_str()).options(options)],
                    &[],
                )
                .await
                .map(|_| ())
                .map_err(SessionEnd::mqtt)
        }
    }
}

/// The one place a packet is put on the wire, and the one place the retain flag
/// is set.
///
/// QoS 1 throughout. QoS 0 would lose a retained discovery config to a single
/// dropped packet with nothing anywhere reporting it — the entity would simply
/// never appear — and R6 permits 0 or 1 for commands, so there is no reason to
/// run two policies.
async fn publish_bytes<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    topic: &str,
    bytes: &[u8],
    retention: Retention,
) -> Result<(), SessionEnd> {
    let publication = Publication::bytes(topic, bytes).qos(QoS::AtLeastOnce);
    let publication = match retention {
        Retention::Retained => publication.retain(),
        Retention::Transient => publication,
    };
    match connection.publish(publication).await {
        Ok(_) => Ok(()),
        // **A packet that will not fit is not a reason to reconnect**, whichever
        // half of it does not fit, and both halves have to be spelled out
        // because `minimq` reports them through different variants.
        //
        // `PubError::Payload(())` is the payload overrunning the TX scratch
        // space. `Error::Resource(_)` is the same condition reached from the
        // header or the topic — and, more importantly, `PacketTooLarge`, which
        // comes from the **broker's** advertised MQTT 5 `MaximumPacketSize`
        // rather than from anything sized here: a broker configured with a
        // limit below a discovery config would refuse every one of them,
        // identically, on every reconnect, at the 60-second ceiling, forever.
        // That is precisely the loop `execute`'s policy exists to forbid, and
        // it is the one case the compile-time capacity proofs cannot rule out
        // because the limit is not ours.
        //
        // Reported and skipped, so the entities that *do* fit still reach Home
        // Assistant and the line names the size an operator has to change.
        Err(minimq::PubError::Payload(())) => {
            report_oversize(topic, bytes.len(), "the transmit buffer");
            Ok(())
        }
        Err(minimq::PubError::Session(minimq::Error::Resource(error))) => {
            report_oversize(
                topic,
                bytes.len(),
                match error {
                    minimq::ResourceError::PacketTooLarge => {
                        "the maximum packet size this broker advertised"
                    }
                    minimq::ResourceError::BufferTooSmall => "the transmit buffer",
                    // Unreachable while `perform` settles every operation — the
                    // slots cannot be exhausted when at most one is ever in use —
                    // and still local, so still not a reason to reconnect.
                    minimq::ResourceError::InflightExhausted => "the in-flight slots",
                    // `ResourceError` is `#[non_exhaustive]`. Every variant it
                    // has is a *local* limit, which is the whole reason this
                    // arm exists, so a new one defaults to the same policy —
                    // report and carry on — rather than to a reconnect loop.
                    _ => "a local limit",
                },
            );
            Ok(())
        }
        Err(minimq::PubError::Session(error)) => Err(SessionEnd::mqtt(error)),
    }
}

/// One line for a packet that could not be sent because something was too
/// small, naming *which* something.
///
/// Separate from the match above so the three causes read as one policy rather
/// than as three arms that happen to agree.
fn report_oversize(topic: &str, bytes: usize, limit: &str) {
    esp_println::println!(
        "mqtt: a {} byte payload for '{}' exceeds {} — not published, \
         and the session is kept because reconnecting would meet it again",
        bytes,
        topic,
        limit,
    );
}

/// The will's bytes, which are always a literal.
fn will_payload(publish: &somfy_mqtt::Publish<'static>) -> &'static [u8] {
    match publish.payload() {
        Payload::Bytes(bytes) => bytes,
        // Unreachable: `MqttConfig::will` is `offline`, a literal.
        _ => somfy_mqtt::OFFLINE,
    }
}

/// What Home Assistant publishes when a `button` entity is pressed.
///
/// Home Assistant's documented default for `payload_press`, matched rather than
/// declared — `somfy_mqtt::ButtonDiscovery` carries the argument for why the
/// literal lives on this side only.
const PAYLOAD_PRESS: &str = "PRESS";

/// Turn an inbound message into a command, or `None` if it is not one.
///
/// The topic is matched against what this device actually subscribed to rather
/// than parsed, so a message on an unexpected topic is ignored rather than
/// guessed at.
fn decode_command(
    config: &MqttConfig,
    inventory: &Inventory,
    topic: &str,
    payload: &[u8],
) -> Option<ControlCommand> {
    let text = core::str::from_utf8(payload).ok()?;
    // Over the shades this device actually announced, not over every `u8`. Two
    // reasons: it is exactly the set that was subscribed to, so a command
    // addressed to a shade that does not exist is refused here rather than one
    // layer down; and it builds topics per provisioned shade rather than per
    // possible shade id, which is an eighth of the work (32 against 256).
    for shade in inventory.ids().iter().copied() {
        if topic == config.shade_topic(shade, ShadeTopic::Command).as_str() {
            // Home Assistant's own cover defaults, which the discovery payload
            // deliberately does not override — see `CoverDiscovery::render`.
            let command = match text {
                "OPEN" => ShadeCommand::Up,
                "CLOSE" => ShadeCommand::Down,
                "STOP" => ShadeCommand::My,
                _ => return None,
            };
            return Some(ControlCommand::Shade { id: shade, command });
        }
        if topic == config.shade_topic(shade, ShadeTopic::SetPosition).as_str() {
            // Already in this project's 0-open to 100-closed scale: the
            // discovery payload states `position_open: 0` and
            // `position_closed: 100`, so Home Assistant converts before it
            // publishes.
            let percent: u8 = text.parse().ok()?;
            return Some(ControlCommand::Shade {
                id: shade,
                command: ShadeCommand::GoTo(Pos::from_percent(percent)),
            });
        }
        if topic == config.shade_topic(shade, ShadeTopic::Pair).as_str() {
            // Home Assistant's own default `payload_press`, which the discovery
            // payload deliberately does not override — see `ButtonDiscovery`.
            //
            // **Matched exactly, and anything else ignored.** This is the one
            // subscribed topic whose command puts `Prog` on the air. The burst
            // length is pinned to a pairing tap, so this path cannot *unpair* a
            // motor — but it can pair one that happens to be in programming
            // mode, and a lenient parse here ("any non-empty payload means
            // press") would let a stray retained message or a mistyped
            // `mosquitto_pub` do exactly that, at whichever motor is listening.
            if text != PAYLOAD_PRESS {
                return None;
            }
            return Some(ControlCommand::Shade {
                id: shade,
                command: ShadeCommand::Pair,
            });
        }
    }
    None
}

/// The last state this device observed for each shade, so a fresh broker
/// session can be given it without waiting for the next change.
///
/// Seeded from the boot inventory and updated from every delta. It is *this
/// task's* copy: the registry belongs to the state task and nothing reaches
/// across that boundary.
struct Known {
    /// Indexed by shade id. `None` means no shade in that slot.
    shades: [Option<Observed>; MAX_SHADES],
}

/// One shade's last observed state, rendered on demand.
#[derive(Clone, Copy)]
struct Observed {
    id: ShadeId,
    pos: Pos,
    direction: Direction,
    /// Whether a delta has actually reported this shade.
    ///
    /// **Nothing is published until it has.** A shade this device has not heard
    /// from has no position it can honestly report: `Pos::ZERO` here would
    /// agree with the state machine only by coincidence — both happen to start
    /// at zero today — and it would go out **retained**, so a wrong value would
    /// outlive the boot that produced it and be handed to every later
    /// subscriber. A confidently wrong retained value is the failure class this
    /// whole integration is written around. An absent one leaves Home Assistant
    /// showing the position unknown, which is what it is.
    seen: bool,
}

impl Known {
    fn new(inventory: &Inventory) -> Known {
        let mut shades = [None; MAX_SHADES];
        for (index, id) in inventory.ids().iter().enumerate() {
            if index < shades.len() {
                shades[index] = Some(Observed {
                    id: *id,
                    pos: Pos::ZERO,
                    direction: Direction::Idle,
                    seen: false,
                });
            }
        }
        Known { shades }
    }

    /// Start tracking a shade that has just been added.
    ///
    /// `seen` stays false, which is the honest state: nothing has reported a
    /// position for it yet, and publishing `Pos::ZERO` retained would hand
    /// every later subscriber a value this device does not know.
    fn track(&mut self, id: ShadeId) {
        if self.shades.iter().flatten().any(|slot| slot.id == id) {
            return;
        }
        if let Some(free) = self.shades.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(Observed {
                id,
                pos: Pos::ZERO,
                direction: Direction::Idle,
                seen: false,
            });
        }
    }

    /// Stop tracking a shade that has been removed, so its last position is not
    /// republished on the next reconnect to a topic that has just been cleared.
    fn forget(&mut self, id: ShadeId) {
        for slot in self.shades.iter_mut() {
            if slot.is_some_and(|held| held.id == id) {
                *slot = None;
            }
        }
    }

    /// Note what a delta said.
    fn record(&mut self, delta: &StateDelta) {
        for slot in self.shades.iter_mut().flatten() {
            if slot.id == delta.id {
                slot.pos = delta.pos;
                slot.direction = delta.direction;
                slot.seen = true;
                return;
            }
        }
    }

    /// This shade's current state, as values that still have to be addressed.
    ///
    /// Two per shade — position and direction — and never a rendered [`Topic`]:
    /// a `Topic` is 264 bytes, and a vector of them held across an `await` goes
    /// into the task's statically allocated future rather than onto a stack
    /// that unwinds. Sixty-four of those is 17 KB of DRAM, taken from the same
    /// segment as the heap and the main stack and never given back. The topic
    /// is built where it is used instead, one at a time.
    fn of(&self, id: ShadeId) -> Vec<StateValue, 2> {
        let mut out = Vec::new();
        for slot in self.shades.iter().flatten() {
            // `seen` is the guard: an unreported shade publishes nothing rather
            // than a retained zero. See [`Observed::seen`].
            if slot.id == id && slot.seen {
                let _ = out.push(StateValue::position(*slot));
                let _ = out.push(StateValue::direction(*slot));
            }
        }
        out
    }

    /// Every shade this device knows about, for a fresh broker session.
    fn ids(&self) -> impl Iterator<Item = ShadeId> + '_ {
        self.shades.iter().flatten().map(|slot| slot.id)
    }
}

/// One shade's state value, rendered but not yet addressed.
///
/// Deliberately small — see [`Known::of`]. Its retention is not carried
/// either: [`MqttConfig::state`] fixes that, and it is read off the
/// [`somfy_mqtt::Publish`] that call produces, at the moment of sending.
struct StateValue {
    id: ShadeId,
    topic: PublishedTopic,
    value: String<8>,
}

impl StateValue {
    fn position(observed: Observed) -> StateValue {
        let mut value: String<8> = String::new();
        let _ = core::fmt::Write::write_fmt(&mut value, format_args!("{}", observed.pos.percent()));
        StateValue::of(observed.id, ShadeTopic::Position, value)
    }

    fn direction(observed: Observed) -> StateValue {
        // Home Assistant's own cover state vocabulary, which the discovery
        // payload deliberately does not override.
        let text = match observed.direction {
            Direction::Up => "opening",
            Direction::Down => "closing",
            Direction::Idle if observed.pos == Pos::FULL => "closed",
            Direction::Idle => "open",
        };
        let mut value: String<8> = String::new();
        let _ = value.push_str(text);
        StateValue::of(observed.id, ShadeTopic::State, value)
    }

    fn of(id: ShadeId, topic: ShadeTopic, value: String<8>) -> StateValue {
        let topic = PublishedTopic::of(topic)
            .expect("position and direction are topics the firmware publishes");
        StateValue { id, topic, value }
    }
}

/// Why a session ended.
///
/// Every variant is reported and then retried. None of them stops the
/// controller: this whole module is a degradable service.
#[allow(
    dead_code,
    reason = "each payload exists to be printed, and a derived \
    Debug is not counted as a read by rustc's dead-code analysis"
)]
#[derive(Debug)]
enum SessionEnd {
    /// The TCP connection could not be established, or failed.
    Tcp(embassy_net::tcp::ConnectError),
    /// The broker refused the session, or the protocol failed.
    Mqtt(MqttFailure),
}

/// A `minimq` failure, flattened.
///
/// `minimq::Error` is generic over the transport's error type, which would
/// leak `embassy_net`'s socket error into every signature in this module. Only
/// the discriminant is ever printed, so it is reduced here.
#[derive(Debug)]
enum MqttFailure {
    /// The broker closed the session, or the keepalive timed out.
    Disconnected,
    /// The transport failed under the client.
    Transport,
    /// The broker rejected an operation or sent something invalid.
    Peer,
    /// A local buffer or in-flight slot was too small.
    Resource,
    /// Anything else the client reported.
    Other,
}

impl SessionEnd {
    fn mqtt<E>(error: minimq::Error<E>) -> SessionEnd {
        SessionEnd::Mqtt(match error {
            minimq::Error::Disconnected => MqttFailure::Disconnected,
            minimq::Error::Transport(_) | minimq::Error::WriteZero => MqttFailure::Transport,
            minimq::Error::Peer(_) => MqttFailure::Peer,
            minimq::Error::Resource(_) => MqttFailure::Resource,
            _ => MqttFailure::Other,
        })
    }
}
