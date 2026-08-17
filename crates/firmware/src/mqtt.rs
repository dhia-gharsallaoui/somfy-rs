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
//!    settings, a [`Broker`] (topic configuration and a snapshot of the shades
//!    to announce), a command *sender* and a delta *subscriber*. It holds no
//!    flash, no radio, no transmit queue, and no reference to the registry —
//!    the shades are a copy taken before the state task owned it. Giving this
//!    task any of those would be a change to its type, not an oversight in its
//!    body.
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

use core::net::SocketAddrV4;

use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_sync::pubsub::WaitResult;
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, Vec};
use minimq::{
    Buffers, ConfigBuilder, ConnectEvent, Publication, QoS, RetainHandling, Session,
    SubscriptionOptions, TopicFilter, Will,
};
use somfy_config::{MqttSettings, Namespaces};
use somfy_domain::{Direction, Pos, ShadeCommand, ShadeId, StateDelta, MAX_SHADES};
use somfy_mqtt::{
    reconfigure, Component, ConfigError, DeviceId, DiscoveryPrefix, MqttConfig, NodeId, Payload,
    PublishedTopic, Retention, ShadeTopic, StateRoot, Step, PAYLOAD_CAPACITY,
};
use somfy_tasks::{Backoff, ControlCommand};

use crate::config::MAX_SUPERSEDED;
use crate::inventory::Inventory;
use crate::tasks::{CommandSender, DeltaSubscriber};

/// Inbound MQTT packet buffer.
///
/// `minimq` advertises this as MQTT 5's `MaximumPacketSize` in CONNECT, so it
/// is not merely local storage — it is the ceiling the broker is told to obey,
/// and inbound is bounded by construction rather than by hope. Everything this
/// device subscribes to is a cover command: `OPEN`, `CLOSE`, `STOP`, or a
/// number. 512 bytes is two orders of magnitude beyond that and still small
/// enough to leave the ESP32-S2 — the tightest chip in the matrix — its stack.
const MQTT_RX_BYTES: usize = 512;

/// Outbound MQTT arena: the largest packet plus whatever QoS 1 state is in
/// flight.
///
/// The largest packet this device sends is a retained discovery config: a
/// topic under 80 bytes, a payload bounded by
/// [`somfy_mqtt::PAYLOAD_CAPACITY`] at 1024, and a fixed header. 1536 covers
/// that with room for a subscribe waiting to be acknowledged, and every byte
/// here is a byte of the DRAM the main stack is carved from.
const MQTT_TX_BYTES: usize = 1536;

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
const SOCKET_TIMEOUT_S: u64 = 20;

/// MQTT keepalive advertised in CONNECT.
///
/// `minimq` drives PINGREQ itself from inside `recv`, so this is the interval
/// at which a dead broker becomes visible. Sixty seconds is the protocol's own
/// common default and well inside the socket timeout above.
const KEEPALIVE_S: u16 = 60;

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
    /// The shades to announce, copied at boot.
    inventory: Inventory,
    /// The last state observed for each shade, so a fresh broker session can be
    /// given it without waiting for the next change.
    known: Known,
    /// The one discovery-payload buffer. One, because only one config is
    /// rendered at a time and a kilobyte is not free on the tightest chip here.
    payload: String<PAYLOAD_CAPACITY>,
    /// Whether the MQTT-version observation has already been logged. It is a
    /// fact about the broker, not about the session, so it is said once.
    version_logged: bool,
    /// How often the two broker-driven log lines have fired. See [`Rare`].
    rare: Rare,
}

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
    ) -> Broker {
        let known = Known::new(&inventory);
        Broker {
            config,
            stale,
            inventory,
            known,
            payload: String::new(),
            version_logged: false,
            rare: Rare::default(),
        }
    }
}

/// Bring up the broker session.
///
/// Returns a `SpawnError` and nothing else; the caller reports it and carries
/// on without MQTT, exactly as it does for Wi-Fi. There is no failure here that
/// stops the controller.
pub fn start(
    spawner: embassy_executor::Spawner,
    stack: Stack<'static>,
    settings: MqttSettings,
    superseded: Vec<Namespaces, MAX_SUPERSEDED>,
    inventory: Inventory,
    commands: CommandSender,
    deltas: DeltaSubscriber,
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

    let broker = Broker::new(config, stale, inventory);
    spawner.spawn(session(stack, settings, broker, commands, deltas)?);
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
                    .serve(&mut session, socket, &commands, &mut deltas)
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
            ConnectEvent::Connected => {
                self.announce(&mut connection, commands).await?;
                // R9's "republish retained state on reconnect". The broker may
                // have lost its retained store, so the values this device last
                // observed go out again rather than waiting for the next change
                // — which, for a shade nobody touches, may be days.
                //
                // One shade at a time, and settled after each: see [`settle`]
                // for why an unsettled burst cannot exceed eight operations.
                for id in self.known.ids() {
                    for state in self.known.of(id) {
                        send_state(&mut connection, &self.config, &state).await?;
                        settle(
                            &mut connection,
                            &self.config,
                            &self.inventory,
                            commands,
                            &mut self.rare,
                        )
                        .await?;
                    }
                }
            }
            // A resumed session: subscriptions and in-flight QoS state survived,
            // so re-announcing the entities would be a broker's worth of
            // retained publishes for no change. Availability still goes out,
            // because the will may have fired while this device was away and
            // left `offline` retained.
            ConnectEvent::Reconnected => {
                let online = self.config.online();
                send(&mut connection, &online).await?;
                // **The retirement is not conditional on the event.** A
                // superseded namespace still has orphans under it whether or
                // not the broker resumed the session, and R5's obligation is
                // about what is retained on the broker rather than about what
                // this client's session remembers. Skipping it here would make
                // the rule depend on a CONNACK flag — which, with
                // `session_expiry_interval(0)`, is a branch nothing takes today
                // and would silently turn the rule off for whoever raises it.
                if !self.stale.is_empty() {
                    self.announce(&mut connection, commands).await?;
                }
            }
        }

        loop {
            // Both halves in one wait. The delta subscriber's future is
            // cancel-safe (it advances its cursor only on `Poll::Ready`) and
            // `minimq`'s `recv` is documented as cancel-safe, so whichever loses
            // is simply dropped.
            //
            // The inbound branch is handled *inside* the match and yields
            // nothing borrowed: an `InboundPublish` borrows the connection
            // mutably, and the publish below needs that borrow back.
            let delta = match select(connection.recv(), deltas.next_message()).await {
                Either::First(inbound) => {
                    let inbound = inbound.map_err(SessionEnd::mqtt)?;
                    dispatch(
                        &self.config,
                        &self.inventory,
                        commands,
                        &mut self.rare,
                        inbound.topic(),
                        inbound.payload(),
                    );
                    None
                }
                Either::Second(WaitResult::Message(delta)) => Some(delta),
                // The subscriber fell behind and the channel dropped deltas for
                // it. Worth one line: it means this task was blocked long enough
                // for the state task to publish `DELTA_QUEUE_DEPTH` updates, so
                // the position now on the broker is behind the shade.
                Either::Second(WaitResult::Lagged(missed)) => {
                    esp_println::println!("mqtt: fell behind, {} state updates dropped", missed);
                    None
                }
            };

            if let Some(delta) = delta {
                self.known.record(&delta);
                for state in self.known.of(delta.id) {
                    send_state(&mut connection, &self.config, &state).await?;
                }
                // A moving shade produces a delta every 100 ms, two publishes
                // each. `recv` above does consume acknowledgements while it
                // waits, but only when it is reached — and it is not reached
                // while the subscriber has a backlog. Settling here bounds
                // in-flight state at every loop boundary instead of relying on
                // that. See [`settle`].
                settle(
                    &mut connection,
                    &self.config,
                    &self.inventory,
                    commands,
                    &mut self.rare,
                )
                .await?;
            }
        }
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
        for step in reconfigure(&self.stale, &self.config, self.inventory.ids(), HAS_TILT) {
            execute(
                connection,
                &step,
                &self.config,
                &self.inventory,
                &mut self.payload,
            )
            .await?;
            // **After every operation, not at the end.** See [`settle`]: an
            // announcement is `1 + 5N` operations and `minimq` holds eight.
            settle(
                connection,
                &self.config,
                &self.inventory,
                commands,
                &mut self.rare,
            )
            .await?;
        }

        // The shade's own name, which no plan can carry because `somfy-mqtt`
        // does not hold names. `ShadeTopic::Name` is a published topic and the
        // retirement clears it, so leaving it unpublished would be exactly the
        // publisher/model drift `ShadeTopic` exists to prevent — the retirement
        // would tombstone an address nothing had ever written.
        for shade in self.inventory.ids().iter().copied() {
            let Some(name) = self.inventory.name(shade) else {
                continue;
            };
            let published =
                PublishedTopic::of(ShadeTopic::Name).expect("a shade's name is published");
            let publish = self.config.state(shade, published, name.as_bytes());
            send(connection, &publish).await?;
            settle(
                connection,
                &self.config,
                &self.inventory,
                commands,
                &mut self.rare,
            )
            .await?;
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
        self.stale.clear();
        Ok(())
    }
}

/// Read the inbound direction until the broker has acknowledged everything
/// outstanding.
///
/// # Why this is not an optimisation
///
/// `minimq` keeps a QoS 1 publish or a subscribe in its retained slots — there
/// are **eight** — until the broker's acknowledgement is *read*, and reading
/// happens only inside `recv`, `poll` or `drive`. Publishing does not read:
/// `publish` and `subscribe` flush the outbound direction and return. So a plan
/// walked without ever reading exhausts the slots and fails at the ninth
/// operation with `InflightExhausted`, and then does the same on every
/// reconnect, at the backoff ceiling, forever.
///
/// An announcement costs `1 + 5N` operations for `N` shades — `online`, then
/// per shade one discovery config, two subscriptions and two state publishes —
/// so **it exceeds eight at two shades**, and at one shade plus one superseded
/// namespace. The retained packets also occupy [`MQTT_TX_BYTES`] until they are
/// acknowledged, and three unacknowledged discovery configs fill it, which is
/// the tighter of the two ceilings.
///
/// Settling after each operation holds in-flight state at one, which makes both
/// ceilings unreachable rather than merely distant. The cost is one round trip
/// per operation, paid once per session.
///
/// An inbound message that arrives while settling is a real command, so it is
/// acted on here rather than dropped: the subscriptions go out during the
/// announcement, and a person pressing a button does not wait for it to finish.
async fn settle<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    config: &MqttConfig,
    inventory: &Inventory,
    commands: &CommandSender,
    rare: &mut Rare,
) -> Result<(), SessionEnd> {
    while !connection.session().is_publish_quiescent() {
        // `poll` returns on any session progress, an acknowledgement included,
        // and it is the only thing that frees a retained slot.
        if let Some(inbound) = connection.poll().await.map_err(SessionEnd::mqtt)? {
            dispatch(
                config,
                inventory,
                commands,
                rare,
                inbound.topic(),
                inbound.payload(),
            );
        }
    }
    Ok(())
}

/// Turn one inbound message into a command and hand it to the state task.
///
/// Shared by the session loop and by [`settle`] so that a command arriving
/// during an announcement is treated exactly like one arriving afterwards.
fn dispatch(
    config: &MqttConfig,
    inventory: &Inventory,
    commands: &CommandSender,
    rare: &mut Rare,
    topic: &str,
    payload: &[u8],
) {
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
    config: &MqttConfig,
    inventory: &Inventory,
    payload: &mut String<PAYLOAD_CAPACITY>,
) -> Result<(), SessionEnd> {
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
                match component {
                    Component::Cover => {
                        if config
                            .cover_discovery(shade, name, HAS_TILT)
                            .render(payload)
                            .is_err()
                        {
                            // `render` leaves the buffer empty rather than
                            // half-written, so nothing partial can be sent —
                            // and a partial config is truncated JSON, which
                            // Home Assistant discards without saying so.
                            esp_println::println!(
                                "mqtt: the discovery config for shade {} does not fit \
                                 its buffer — the entity will not appear",
                                shade.0,
                            );
                            return Ok(());
                        }
                    }
                    // Task 4 adds the rest of R7's entity set. Reported loudly
                    // rather than skipped in silence: an entity the plan
                    // announces and nothing publishes is exactly the
                    // half-configured state this integration exists to prevent.
                    other => {
                        esp_println::println!(
                            "mqtt: no payload renderer for a '{}' entity — \
                             shade {} will be missing one",
                            other.as_str(),
                            shade.0,
                        );
                        return Ok(());
                    }
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

/// Send one already-decided [`somfy_mqtt::Publish`].
async fn send<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    publish: &somfy_mqtt::Publish<'_>,
) -> Result<(), SessionEnd> {
    let bytes = match publish.payload() {
        Payload::Bytes(bytes) => bytes,
        Payload::Nothing => &[],
        // Unreachable: `send` is only used for publishes that carry their own
        // bytes, because rendering needs the buffer only [`execute`] holds.
        // Reported and skipped rather than ended, for the reason on `execute`.
        Payload::Discovery { component, .. } => {
            esp_println::println!(
                "mqtt: a '{}' discovery config reached the buffer-free path and \
                 was not published",
                component.as_str(),
            );
            return Ok(());
        }
    };
    publish_bytes(
        connection,
        publish.topic().as_str(),
        bytes,
        publish.retention(),
    )
    .await
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
        Err(minimq::PubError::Session(error)) => Err(SessionEnd::mqtt(error)),
        // The payload did not fit the TX scratch space. That is a **local**
        // limit, not a broker one, so it is reported and skipped rather than
        // ending the session: reconnecting would meet it again, and one packet
        // that cannot be encoded must not cost every other entity its config.
        // Unreachable for anything within `somfy-mqtt`'s own bounds, which
        // [`MQTT_TX_BYTES`] is sized for.
        Err(minimq::PubError::Payload(())) => {
            esp_println::println!(
                "mqtt: {} bytes do not fit the transmit buffer — '{}' not published",
                bytes.len(),
                topic,
            );
            Ok(())
        }
    }
}

/// The will's bytes, which are always a literal.
fn will_payload(publish: &somfy_mqtt::Publish<'static>) -> &'static [u8] {
    match publish.payload() {
        Payload::Bytes(bytes) => bytes,
        // Unreachable: `MqttConfig::will` is `offline`, a literal.
        _ => somfy_mqtt::OFFLINE,
    }
}

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
    // layer down; and it is at most 32 topic constructions instead of 512.
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
    /// that unwinds. Sixty-four of those is 17 KB of DRAM, which is what took
    /// the ESP32-S2 below the point where its image links at all. The topic is
    /// built where it is used instead, one at a time.
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

/// Address a state value and send it, retained.
async fn send_state<'buf, IO: minimq::Io>(
    connection: &mut minimq::Connection<'_, 'buf, IO>,
    config: &MqttConfig,
    state: &StateValue,
) -> Result<(), SessionEnd> {
    let publish = config.state(state.id, state.topic, state.value.as_bytes());
    send(connection, &publish).await
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
