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
use heapless::Deque;
use heapless::{String, Vec};
use minimq::{
    Buffers, ConfigBuilder, ConnectEvent, Publication, QoS, RetainHandling, Session,
    SubscriptionOptions, TopicFilter, Will,
};
use somfy_api::{ApiErrorCode, CreateShadeDto, PatchShadeDto};
use somfy_config::{MqttSettings, Namespaces};
use somfy_domain::{
    Direction, Pos, ShadeCommand, ShadeId, StateDelta, TiltMode, FACTORY_TILT_TIME_MS, MAX_SHADES,
};
use somfy_mqtt::{
    reconfigure, Ask, Component, ConfigError, ConfigurationUrl, DeviceEntity, DeviceId,
    DiscoveryPrefix, Effect, FormChange, MqttConfig, NodeId, Pairing, Payload, PublishedTopic,
    Retention, Setup, SetupEntity, SetupInput, SetupMessage, SetupValue, ShadeTopic, StateRoot,
    Step, MAX_DRAFT_NAME_LEN, MAX_KIND_LABEL_LEN, PAYLOAD_CAPACITY,
};
use somfy_tasks::{Backoff, ControlCommand};

use crate::config::MAX_SUPERSEDED;
use crate::edits::{AckSender, EventReceiver, ShadeAck, ShadeEdit, ShadeEvent};
use crate::inventory::Inventory;
use crate::rpc::{Reply, Request as Rpc, RPC};
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
    /// The add-a-shade form's state: which phase, what has been typed, what
    /// `Next step` says. Pure, and it holds no behaviour — see
    /// `somfy_mqtt::Setup`.
    setup: Setup,
    /// Effects produced by an inbound message and not yet carried out.
    ///
    /// **A queue rather than an immediate action**, because a press arrives
    /// while `minimq` holds the connection borrowed for the inbound packet, and
    /// because one can arrive in the middle of an announcement. Applying the
    /// flow is pure, so it happens the moment the message is decoded; only the
    /// *publishing* waits, and it waits at most until the session loop comes
    /// round.
    effects: Deque<Effect, EFFECT_QUEUE_DEPTH>,
    /// Whether the broker may be holding a form this device does not know it
    /// left there.
    ///
    /// **True at boot**, because a device that was power-cut mid-setup left
    /// eight retained discovery configs and their values behind, and nothing in
    /// RAM remembers them. The first fresh session clears the form
    /// unconditionally and sets this false; every later session in the same
    /// boot knows what it published and clears nothing it need not.
    ///
    /// The alternative was persisting a "setup running" bit beside the shade
    /// table, which is a flash write per press of `Add shade` for something a
    /// reboot deliberately abandons. Thirteen tombstones once per boot is the
    /// cheaper honesty.
    form_dirty: bool,
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

/// Steps the add-a-shade form's announcement or retirement costs.
///
/// Eight discovery configs to open it and thirteen tombstones to close it —
/// eight configs and the five values behind them. Sixteen covers the wider of
/// the two with room, and the assertion below is the check rather than the
/// constant.
const FORM_STEPS: usize = 16;

/// Effects waiting to be carried out.
///
/// Four, and the argument is the same one `crate::edits::EDIT_QUEUE_DEPTH`
/// makes: an effect is a person pressing a button, and a queue deeper than the
/// number of buttons a person can press while one announcement finishes buys
/// nothing. An overflow is reported rather than dropped silently.
const EFFECT_QUEUE_DEPTH: usize = 4;

/// Bytes one form value may occupy, excluding the instructions.
///
/// A name is the longest — the same 32 a shade record holds — and a kind label
/// and a rendered millisecond count are both shorter. The instructions are
/// **not** here: they are a `&'static str` on `SetupMessage`, so they are
/// published without a copy and their 255 bytes never reach a stack frame.
const FORM_VALUE_BYTES: usize = MAX_DRAFT_NAME_LEN;

const _: () = assert!(
    FORM_VALUE_BYTES >= MAX_KIND_LABEL_LEN,
    "a shade-kind label no longer fits the form-value buffer",
);

/// Which of the form's two plans [`Broker::walk_form`] is walking.
///
/// An enum rather than a `bool`, for the reason `somfy_mqtt::Retention` is not
/// one: at a call site `true` says nothing about which way round it is, and
/// getting it backwards would clear a form that was meant to appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Publish the eight discovery configs, then the values behind them.
    Open,
    /// Clear the configs and the values, and publish nothing afterwards.
    Close,
    /// Republish the values alone, the form itself being unchanged.
    Values,
}

/// How many times one queued effect may lead to another.
///
/// The longest real chain is two — `Send pairing` creates a shade, and the
/// created id is what the pairing burst is addressed to. A bound rather than
/// recursion, because a flow that answered its own ask would otherwise spin
/// inside a broker session with the connection held.
const MAX_EFFECT_CHAIN: usize = 4;

/// The nearest thing the form can say about a refusal from the shade table.
///
/// Deliberately coarse. The form has one sensor and 255 characters, and the
/// codes that reach here are either already prevented by
/// `somfy_mqtt::Draft::blocker` — an empty name, a zero travel time — or are
/// facts about the device rather than about the request. `RegistryFull` is
/// singled out because it is the one an operator can act on without a serial
/// cable, and the rest name the two surfaces that do carry the detail.
fn refusal_message(code: ApiErrorCode) -> SetupMessage {
    match code {
        ApiErrorCode::RegistryFull => SetupMessage::RegistryFull,
        ApiErrorCode::NameTooLong | ApiErrorCode::NameEmpty => SetupMessage::NeedsName,
        ApiErrorCode::TravelTimeZero => SetupMessage::NeedsTimes,
        _ => SetupMessage::Refused,
    }
}

/// The draft, as the request `POST /api/v1/shades` would carry.
///
/// **The same DTO and therefore the same validator.** `CreateShadeDto::to_config`
/// is where every rule about what a shade may be lives, and it runs at the
/// allocated address inside `tasks::apply_edit` — so a form that got past
/// `Draft::blocker` and should not have is still refused there, by the code the
/// web surface is refused by.
///
/// `tilt_time_ms` is the factory figure because the form has no tilt field to
/// carry one, and `somfy_api::supplied_source` marks a value equal to the
/// factory default as `FactoryDefault` — which is the honest record: nobody
/// supplied it. `tilt_mode` is `None` for the reason `HAS_TILT` is false; see
/// that constant.
fn create_request(setup: &Setup) -> CreateShadeDto {
    let draft = setup.draft();
    // Capacity comes from the field's own type, so this cannot drift from
    // `somfy-api`'s inbound name budget.
    let mut name = heapless::String::new();
    // All-or-nothing in `heapless`, and it cannot fail: `Draft` holds at most
    // `MAX_DRAFT_NAME_LEN` bytes and the DTO's inbox is wider. An empty name
    // reaches `checked_name`, which refuses it.
    let _ = name.push_str(draft.name());
    CreateShadeDto {
        name,
        kind: draft.kind() as u8,
        tilt_mode: TiltMode::None as u8,
        // Unreachable while `Draft::blocker` gates the create — both are set —
        // and zero rather than a guess if it ever is not, because
        // `checked_lift_times` refuses zero and a guessed travel time is the
        // fault this form exists to prevent.
        up_time_ms: draft.up_ms().unwrap_or(0),
        down_time_ms: draft.down_ms().unwrap_or(0),
        tilt_time_ms: FACTORY_TILT_TIME_MS,
    }
}

/// The draft, as the request `PATCH /api/v1/shades/{id}` would carry.
///
/// Only the four fields the form owns. Everything absent means unchanged, which
/// is what lets an operator correct a travel time after watching the shade move
/// without disturbing anything the guided calibration measured.
fn amend_request(setup: &Setup) -> PatchShadeDto {
    let draft = setup.draft();
    let mut name = heapless::String::new();
    let _ = name.push_str(draft.name());
    PatchShadeDto {
        name: (!draft.name().is_empty()).then_some(name),
        kind: Some(draft.kind() as u8),
        up_time_ms: draft.up_ms(),
        down_time_ms: draft.down_ms(),
        ..PatchShadeDto::default()
    }
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
        orphans: Vec<ShadeId, MAX_SHADES>,
        survey: Survey,
        acks: AckSender,
    ) -> Broker {
        let known = Known::new(&inventory);
        let awaiting_setup = inventory.awaiting_setup();
        Broker {
            config,
            stale,
            inventory,
            orphans,
            known,
            diagnostics: Diagnostics {
                rollcode_damaged: survey.damaged,
                awaiting_setup,
            },
            payload: String::new(),
            version_logged: false,
            rare: Rare::default(),
            acks,
            setup: Setup::new(),
            effects: Deque::new(),
            // See the field: a boot knows nothing about a form it may have left
            // on the broker, so the first fresh session clears one.
            form_dirty: true,
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
/// | shades awaiting setup | the boot inventory, then [`ShadeEvent::AwaitingSetup`] |
///
/// The first four are read at the moment they are published, so nothing about
/// them has to be kept up to date. The last two are carried, for different
/// reasons and with different consequences.
///
/// The rolling-code figure is a **snapshot of the region as it was at boot**:
/// the store belongs to the state task from the moment it is handed over, and
/// re-surveying it would mean reaching across the boundary that keeps a broker
/// from being able to affect radio control. A slot damaged after boot is
/// therefore reported at the next one — which is the same latency an operator
/// reading the serial line has, and `docs/provenance.md` records the condition
/// for improving it.
///
/// The awaiting-setup figure is carried for the same boundary reason and is
/// **not** stale in the same way: the state task restates it after every edit
/// that could move it, so the only window is one dropped event on a full queue,
/// closed by the next edit or the next boot. See [`ShadeEvent::AwaitingSetup`],
/// which is absolute rather than a delta precisely so that a dropped one heals.
struct Diagnostics {
    /// Slots in the rolling-code region that were neither valid nor blank at
    /// boot.
    rollcode_damaged: usize,
    /// Shades that exist and that nobody has reported working.
    ///
    /// **The only thing in Home Assistant that knows an unfinished setup
    /// exists.** A created-and-unconfirmed shade has no cover and no button —
    /// deliberately, because its address has been heard by no motor — so
    /// without this number the whole first half of adding a shade is invisible
    /// from the surface the operator is actually looking at.
    awaiting_setup: u8,
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
            // Zero is published like any other reading. An entity that goes
            // blank when there is nothing to report is one an operator cannot
            // tell from a broken one, and "no setups are pending" is exactly
            // the answer somebody checking this wants to be given.
            DeviceEntity::AwaitingSetup => write!(&mut out, "{}", self.awaiting_setup),
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
    /// The add-a-shade flow, which an inbound message may advance.
    ///
    /// Applying an input is **pure**, so it happens here, synchronously, while
    /// the connection is still borrowed for the inbound packet. What it
    /// produces is an [`Effect`], which is a `Copy` value with no borrows and
    /// therefore something the queue below can hold until the session loop can
    /// publish.
    setup: &'a mut Setup,
    /// Effects this message produced, for the session loop to carry out.
    effects: &'a mut Deque<Effect, EFFECT_QUEUE_DEPTH>,
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
    // Once, so the line it prints is a fact about this boot rather than one per
    // configuration. The superseded ones are given it too: every step of their
    // plans is a zero-length tombstone, so it never renders, and one
    // construction path is easier to hold than a second that omits it.
    let url = configuration_url();
    let config = match topic_config(
        settings.discovery_prefix(),
        settings.state_root(),
        &device_id,
        url.as_ref(),
    ) {
        Ok(config) => config,
        Err(error) => {
            // Unreachable through the provisioning path — `MqttSettings::new`
            // has already refused everything this can report — but reported
            // rather than `expect`ed, because a panic here reboots the board,
            // and it would do so on every boot.
            crate::logln!(
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
        match topic_config(
            old.discovery_prefix(),
            old.state_root(),
            &device_id,
            url.as_ref(),
        ) {
            Ok(config) => {
                let _ = stale.push(config);
            }
            Err(error) => crate::logln!(
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
///
/// `url` is computed once by the caller rather than here, because saying which
/// address Home Assistant's device page will point at is a line worth one boot
/// rather than one per superseded namespace pair.
fn topic_config(
    discovery_prefix: &str,
    state_root: &str,
    device_id: &str,
    url: Option<&ConfigurationUrl>,
) -> Result<MqttConfig, ConfigError> {
    let config = MqttConfig::new(
        DiscoveryPrefix::new(discovery_prefix)?,
        StateRoot::new(state_root)?,
        NodeId::new(device_id)?,
        DeviceId::new(device_id)?,
    )?;
    Ok(match url {
        Some(url) => config.with_configuration_url(url.clone()),
        None => config,
    })
}

/// The scheme this device's web server answers on. No TLS: there is no
/// certificate a `.local` name could carry that a browser would accept.
#[cfg(feature = "mdns")]
const URL_SCHEME: &str = "http://";

/// The domain `edge-mdns` appends to the name this device claims.
#[cfg(feature = "mdns")]
const MDNS_DOMAIN: &str = ".local";

/// `http://<hostname>.local` at its widest, which is also its only width.
#[cfg(feature = "mdns")]
const CONFIGURATION_URL_LEN: usize =
    URL_SCHEME.len() + crate::identity::HOSTNAME_LEN + MDNS_DOMAIN.len();

// The two limits meet here and nowhere else, so this is where the claim has to
// be checked. `somfy_mqtt::ConfigurationUrl::new` refuses an over-long URL
// rather than truncating it, which would leave the device page with no link and
// one serial line to explain it — a compile error is the better end of that.
#[cfg(feature = "mdns")]
const _: () = assert!(
    CONFIGURATION_URL_LEN <= somfy_mqtt::MAX_CONFIGURATION_URL_LEN,
    "this device's own URL no longer fits somfy-mqtt's configuration-URL budget",
);

/// Where Home Assistant's device page sends a person who wants to configure
/// this controller.
///
/// # Why this is the answer to "add a shade from Home Assistant"
///
/// Adding a shade is a guided procedure with a person in the middle of it, and
/// `somfy_mqtt`'s crate docs carry the ruling on why it does not become a set
/// of entities. What it becomes instead is this: Home Assistant's device page
/// links straight into the assistant that runs it, so an operator standing at
/// the window with a phone gets there from the app they already have open —
/// which is what "without the web UI" was actually asking for. The one thing
/// Home Assistant then reports on its own is
/// [`DeviceEntity::AwaitingSetup`], so a setup left half-way is visible there
/// too.
///
/// # Why it is gated on mDNS rather than on the web server
///
/// Because the name has to resolve. `http` alone means there is a server; only
/// `mdns` means this device answers to `<hostname>.local`, and a link that
/// fails to open is worse than no link — it reads as a device that has broken
/// rather than one that was never advertised.
///
/// A DHCP address is deliberately **not** used as the fallback. A discovery
/// config is retained, so an address baked into one outlives the lease that
/// produced it and goes on pointing at whatever holds it next. That is the
/// confidently-wrong retained value this whole integration is written around.
#[cfg(feature = "mdns")]
fn configuration_url() -> Option<ConfigurationUrl> {
    let mut text: String<CONFIGURATION_URL_LEN> = String::new();
    // None can fail: the capacity is the sum of exactly these three pieces.
    let _ = text.push_str(URL_SCHEME);
    let _ = text.push_str(&crate::identity::hostname());
    let _ = text.push_str(MDNS_DOMAIN);
    match ConfigurationUrl::new(&text) {
        Ok(url) => {
            crate::logln!(
                "mqtt: Home Assistant's device page will link to {} — which is where a shade is \
                 added, because pairing needs a person at the motor and a remote this controller \
                 is not",
                url.as_str(),
            );
            Some(url)
        }
        Err(error) => {
            // Unreachable: `identity::hostname` is a validated DNS label by
            // construction and the assertion above bounds the length. Reported
            // rather than `expect`ed, because a panic here reboots the board
            // over a hyperlink.
            crate::logln!(
                "mqtt: this device's own URL is not a usable configuration_url ({}) — \
                 Home Assistant's device page will have no link to its web UI",
                error,
            );
            None
        }
    }
}

/// No mDNS responder, so no name to link to. See the other arm.
#[cfg(not(feature = "mdns"))]
fn configuration_url() -> Option<ConfigurationUrl> {
    crate::logln!(
        "mqtt: no mDNS responder in this image, so Home Assistant's device page will have no \
         link to this controller's web UI — a DHCP address is not used instead, because a \
         retained discovery config would outlive the lease"
    );
    None
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
                    crate::logln!("mqtt: session at {} ended after {} ms", endpoint, lasted,)
                }
                Err(end) => crate::logln!(
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
            crate::logln!("mqtt: reconnecting in {} ms", waiting);
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
            crate::logln!(
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
            // **Before the wait, not after.** An effect can be queued from two
            // places: the inbound branch below, which returns here, and
            // `settle` inside an announcement, which does not — so draining
            // after the `select4` would leave a press produced mid-announcement
            // sitting until something else woke the task, which on a quiet
            // device is the next diagnostic tick a minute later.
            self.drain_setup(&mut connection, commands).await?;

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
                            setup: &mut self.setup,
                            effects: &mut self.effects,
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
                    crate::logln!("mqtt: fell behind, {} state updates dropped", missed);
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
                // No entity is created or retired here: the count is a fact
                // about the controller, and its discovery config went out with
                // the rest of `DeviceEntity::ALL`. What changes is the reading,
                // and it is published at once rather than at the next
                // diagnostic tick — a person who has just created a shade and
                // gone to look at Home Assistant should not wait a minute to
                // see that something is pending.
                Some(Woken::Shade(ShadeEvent::AwaitingSetup { count })) => {
                    self.diagnostics.awaiting_setup = count;
                    self.publish_reading(&mut connection, DeviceEntity::AwaitingSetup, commands)
                        .await?;
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
            self.publish_reading(connection, entity, commands).await?;
        }
        Ok(())
    }

    /// One device-level entity's current reading, retained.
    ///
    /// Split out of [`Broker::publish_diagnostics`] so that a figure which has
    /// just changed can be sent on its own, without a round trip for each of
    /// the other five. Both callers get the same "nothing honest to report
    /// publishes nothing" rule, which is the half that must not be duplicated:
    /// a second copy would be a second chance to publish a placeholder
    /// retained.
    async fn publish_reading<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        entity: DeviceEntity,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        let Some(reading) = self.diagnostics.reading(entity) else {
            return Ok(());
        };
        let publish = Step::Send(self.config.device_state(entity, reading.as_bytes()));
        self.perform_one(connection, &publish, commands).await
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
                setup: &mut self.setup,
                effects: &mut self.effects,
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
            crate::logln!(
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
                setup: &mut self.setup,
                effects: &mut self.effects,
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
                crate::logln!(
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
            crate::logln!(
                "mqtt: clearing the entities of {} removed shade(s)",
                self.orphans.len(),
            );
        }
        let orphans: Vec<ShadeId, MAX_SHADES> = self.orphans.clone();
        for id in orphans {
            self.retire(connection, id, commands).await?;
        }

        // **The form a power cut left behind.** Its eight discovery configs and
        // five values are retained on the broker and nothing in RAM remembers
        // them, so the first fresh session of a boot clears them
        // unconditionally. Thirteen zero-length publishes, once, against a
        // flash write on every press of `Add shade` — see `Broker::form_dirty`.
        //
        // Skipped entirely when a setup is running, which is the reconnect
        // case: the configs are still wanted, and `carry_out` republishes the
        // values anyway on the next input.
        if self.form_dirty && !self.setup.phase().is_open() {
            self.sync_form(connection, commands, Form::Close).await?;
        }
        self.form_dirty = false;

        // And a setup that survived a reconnect gets its form back: the retained
        // configs should still be on the broker, but a broker that lost its
        // retained store did not keep them, and this is the one moment that is
        // knowable.
        if self.setup.phase().is_open() {
            self.sync_form(connection, commands, Form::Open).await?;
        }

        self.stale.clear();
        Ok(())
    }

    /// Carry out every effect the inbound path queued.
    ///
    /// Called at the top of the session loop, so an effect produced while the
    /// connection was borrowed — or in the middle of an announcement — waits
    /// exactly until the next time round.
    async fn drain_setup<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        while let Some(effect) = self.effects.pop_front() {
            self.carry_out(connection, effect, commands).await?;
        }
        Ok(())
    }

    /// One effect, and whatever the shade table's answer leads to.
    ///
    /// # Why the asks run before anything is published
    ///
    /// Because the answer changes what there is to publish. `Send pairing`
    /// produces [`Ask::Create`], whose answer moves the flow to
    /// `AwaitingReport` and changes `Next step`; publishing before the round
    /// trip would put the old message on the broker and then replace it a few
    /// milliseconds later. So the chain runs first, the form change is
    /// remembered, and exactly one publish pass follows.
    ///
    /// The loop is bounded rather than recursive. The longest real chain is two
    /// — create, then pair — and a bound is what stops a flow that answered its
    /// own ask from spinning inside a broker session.
    async fn carry_out<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        first: Effect,
        commands: &CommandSender,
    ) -> Result<(), SessionEnd> {
        let mut opened = false;
        let mut closed = false;
        let mut effect = first;
        for _ in 0..MAX_EFFECT_CHAIN {
            match effect.form {
                FormChange::Open => opened = true,
                FormChange::Close => closed = true,
                FormChange::Unchanged => {}
            }
            let Some(ask) = effect.ask else { break };
            match self.answer(ask).await {
                Some(next) => effect = self.setup.apply(next),
                None => break,
            }
        }

        if closed {
            // **R5 for the form.** Every config and every value the form could
            // own goes, and nothing is published afterwards — the entities are
            // gone, so a value would be a retained orphan under no config at
            // all.
            self.sync_form(connection, commands, Form::Close).await?;
            return Ok(());
        }
        // One call, one nested frame. `Open` publishes the configs and then the
        // values; `Values` publishes the values alone.
        if opened {
            self.sync_form(connection, commands, Form::Open).await?;
        } else if self.setup.phase().is_open() {
            self.sync_form(connection, commands, Form::Values).await?;
        }
        Ok(())
    }

    /// Put the broker's copy of the form where the flow says it should be.
    ///
    /// # Why the configs and the values are one function
    ///
    /// For ordering, not for size. Opening publishes the configs and then the
    /// values, closing publishes the tombstones and then **nothing**, and
    /// keeping both orders in one place is what stops a caller getting either
    /// backwards — a value published after its config was cleared is a retained
    /// orphan under no entity at all.
    ///
    /// It was *also* tried as a size measure, on the theory that two awaited
    /// futures would each carry their own copy of the deep publish chain, and
    /// **the theory was wrong**: split into `walk_form` and `publish_form` the
    /// image measured 64,560 bytes of `.stack` on the ESP32-C3, merged it
    /// measures 64,552 — eight bytes, which is alignment. The compiler already
    /// overlaps sequential awaits. Recorded because a plausible optimisation
    /// that does nothing is worth not attempting twice.
    ///
    /// # Why the plan is re-created per step
    ///
    /// A collected plan is worse still. A `Step` carries a `Topic`, which is a
    /// `String<TOPIC_CAPACITY>` — about 280 bytes — so a `Vec<Step, 16>` is
    /// **4.5 KB** resident for the life of the boot whether or not anybody ever
    /// opens the form. That shape was measured at 7,088 bytes of DRAM across
    /// three collections; this one is 1,328. Re-creating the iterator per step
    /// is quadratic in a plan of at most thirteen entries of a few string
    /// pushes, which is nothing against the broker round trip every step pays
    /// anyway.
    async fn sync_form<'buf, IO: minimq::Io>(
        &mut self,
        connection: &mut minimq::Connection<'_, 'buf, IO>,
        commands: &CommandSender,
        which: Form,
    ) -> Result<(), SessionEnd> {
        if which != Form::Values {
            let mut overran = true;
            for index in 0..FORM_STEPS {
                // The borrow of `self.config` ends with this expression: a
                // `Step` owns its topic, so `perform_one` gets `&mut self` back.
                let step = match which {
                    Form::Close => self.config.close_form().nth(index),
                    _ => self.config.open_form().nth(index),
                };
                let Some(step) = step else {
                    overran = false;
                    break;
                };
                self.perform_one(connection, &step, commands).await?;
            }
            if overran {
                // A plan longer than the buffer would silently announce or clear
                // fewer entities than it holds — the half-configured state R5 is
                // about, arrived at from the firmware's side.
                esp_println::println!(
                    "mqtt: the add-a-shade form's plan is longer than {} steps — part of \
                     it was not carried out",
                    FORM_STEPS,
                );
            }
        }

        // **The entities are gone; nothing follows.** A value published after
        // the configs were cleared is a retained orphan under no config at all.
        if which == Form::Close {
            return Ok(());
        }

        // The instructions first, and without a copy: `SetupMessage::as_str` is
        // a `&'static str`, so its 255 bytes never reach a stack frame.
        //
        // **All five values, after any input that leaves the form open** — see
        // `somfy_mqtt::Effect` for why that rule is stated once rather than
        // optimised into a per-input list of what moved. `Next step` changes on
        // almost every input in any case.
        let message = Step::Send(self.config.setup_message(self.setup.message()));
        self.perform_one(connection, &message, commands).await?;

        for entity in SetupEntity::FORM {
            if entity == SetupEntity::NextStep || !entity.has_state() {
                continue;
            }
            // Copied out of the flow before the publish, because `perform_one`
            // takes `&mut self` and the value borrows it. Thirty-two bytes.
            //
            // A value the flow reports as unset publishes **nothing**, exactly
            // as a diagnostic with nothing to report does: the publish is
            // retained, so a placeholder would outlive the setup that produced
            // it, and an empty retained payload is a tombstone rather than a
            // blank.
            let Some(value) = self.form_value(entity) else {
                continue;
            };
            let Some(publish) = self.config.setup_state(entity, value.as_bytes()) else {
                // Unreachable: filtered on `has_state` above.
                continue;
            };
            self.perform_one(connection, &Step::Send(publish), commands)
                .await?;
        }
        Ok(())
    }

    /// One form value as bytes, or `None` when there is nothing honest to
    /// publish.
    fn form_value(&self, entity: SetupEntity) -> Option<String<FORM_VALUE_BYTES>> {
        let mut out: String<FORM_VALUE_BYTES> = String::new();
        let written = match self.setup.value(entity) {
            SetupValue::Text(text) => out.push_str(text).map_err(|()| core::fmt::Error),
            SetupValue::Number(value) => write!(&mut out, "{value}"),
            SetupValue::Unset => return None,
        };
        // Unreachable — `FORM_VALUE_BYTES` holds every one of these, and the
        // assertion beside it pins the widest — and treated as "nothing to
        // publish" rather than published half-written, because a truncated
        // value is a plausible wrong value.
        written.ok().map(|()| out)
    }

    /// Ask the shade table for one thing, and turn its answer back into an
    /// input.
    ///
    /// **This is the whole of the flow's contact with behaviour**, and every
    /// arm goes through `crate::rpc` to `tasks::apply_edit` — the same function
    /// and the same seam the web server uses. Nothing about adding a shade is
    /// implemented twice.
    ///
    /// `Rpc::Pair` rather than a `ShadeCommand::Pair` on the command channel:
    /// the RPC carries a rule a movement does not — a shade whose address came
    /// from another controller is refused — and it answers, which is what lets
    /// a refusal reach the form instead of a serial console.
    async fn answer(&mut self, ask: Ask) -> Option<SetupInput<'static>> {
        let request = match ask {
            Ask::Create => Rpc::Edit(ShadeEdit::Add {
                request: create_request(&self.setup),
            }),
            Ask::Pair(id) => Rpc::Pair(id),
            Ask::Confirm(id) => Rpc::Edit(ShadeEdit::ConfirmPairing { id }),
            Ask::Abandon(id) => Rpc::Edit(ShadeEdit::Remove { id }),
            Ask::Amend(id) => Rpc::Edit(ShadeEdit::Reconfigure {
                id,
                patch: amend_request(&self.setup),
            }),
        };
        match RPC.call(request).await {
            Some(Reply::Created(id)) => Some(SetupInput::Created(id)),
            // Only a confirmation ends the setup. The other three are done and
            // have nothing to say, so the form stays where it is.
            Some(Reply::Done) => matches!(ask, Ask::Confirm(_)).then_some(SetupInput::Done),
            Some(Reply::Refused(error)) => {
                esp_println::println!("mqtt: the setup form's {:?} was refused ({:?})", ask, error);
                Some(SetupInput::Refused(refusal_message(error.code)))
            }
            other => {
                // The state task did not answer inside its timeout, or answered
                // something this call cannot produce. Both are faults in this
                // device rather than in the request, and both are reported —
                // the alternative is a form that swallowed a press.
                esp_println::println!(
                    "mqtt: the setup form's {:?} went unanswered ({:?})",
                    ask,
                    other
                );
                Some(SetupInput::Refused(SetupMessage::Refused))
            }
        }
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
/// An announcement costs `1 + 5N + k` operations for `N` shades and the `k = 6`
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
    let (config, inventory, commands) = (wire.config, wire.inventory, wire.commands);
    if let Some(command) = decode_command(config, inventory, topic, payload) {
        // `try_send`, never `send`: see this module's docs.
        if commands.try_send(command).is_err() {
            report_rare(
                &mut wire.rare.dropped_commands,
                "mqtt: command queue full, a command was dropped",
            );
        }
        return;
    }

    // **The form, applied here and published later.** `Setup::apply` is pure —
    // no socket, no clock, no registry — so the flow advances the moment the
    // message is decoded, even though the connection is borrowed for the
    // inbound packet and nothing can be published yet. What comes out is a
    // `Copy` value the session loop carries out.
    if let Some(input) = Setup::decode(config, topic, payload) {
        let effect = wire.setup.apply(input);
        if wire.effects.push_back(effect).is_err() {
            // Reported rather than dropped in silence: the operator pressed
            // something and the form will not move, and the `Next step` sensor
            // is the only thing that could have said so.
            report_rare(
                &mut wire.rare.dropped_commands,
                "mqtt: the setup-effect queue is full, a form action was dropped",
            );
        }
        return;
    }

    report_rare(
        &mut wire.rare.unrecognised,
        "mqtt: a message arrived on a subscribed topic that is not a command this device knows",
    );
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
        crate::logln!("{} ({} so far)", message, counter);
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
                    crate::logln!(
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
                        crate::logln!(
                            "mqtt: no payload renderer for a '{}' entity — \
                             shade {} will be missing one",
                            other.as_str(),
                            shade.0,
                        );
                        return Ok(());
                    }
                };
                if rendered.is_err() {
                    crate::logln!(
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
            // The add-a-shade form. Rendered from the entity alone, exactly as a
            // diagnostic is: what the control currently *holds* is published
            // separately, on the topic this config names.
            Payload::SetupDiscovery(entity) => {
                if config.setup_discovery(entity).render(payload).is_err() {
                    esp_println::println!(
                        "mqtt: the discovery config for '{}' does not fit its buffer — \
                         that part of the add-a-shade form will not appear",
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
            Payload::DeviceDiscovery(entity) => {
                if config.diagnostic_discovery(entity).render(payload).is_err() {
                    crate::logln!(
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
    crate::logln!(
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
            // The first three are Home Assistant's own cover defaults, which
            // the discovery payload deliberately does not override — see
            // `CoverDiscovery::render`.
            //
            // **`VENT` is ours, and it is not one Home Assistant will ever
            // send.** It is here so the broker surface reaches every behaviour
            // the HTTP surface does, which is this project's standing rule; an
            // automation or a button card gets at it with `mqtt.publish`.
            //
            // Why a fourth payload rather than a second per-shade button
            // entity: a shade's entity identity is `(device, component, shade
            // id)` throughout `somfy_mqtt`, so two `button`s on one shade would
            // collide on both `object_id` and `unique_id`. Giving a shade more
            // than one entity of a component means adding a per-shade entity
            // dimension mirroring `somfy_mqtt::DeviceEntity`, which is a change
            // to the identity that every retained discovery config on the
            // broker is keyed by — not a change to make in passing for one
            // command. Adding a payload here cannot confuse the cover, because
            // Home Assistant only ever sends the three above.
            let command = match text {
                "OPEN" => ShadeCommand::Up,
                "CLOSE" => ShadeCommand::Down,
                "STOP" => ShadeCommand::My,
                "VENT" => ShadeCommand::Vent,
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
