//! The two Embassy tasks, and nothing else.
//!
//! Both bodies live in `somfy-tasks`, where a host compiler can reach them.
//! What is here is what cannot be: the concrete hardware types the loops are
//! instantiated with, the `#[embassy_executor::task]` wrappers, a clock, and
//! the decision about what is worth saying over the serial line.
//!
//! ## Stacks, and why there is no per-task figure
//!
//! An Embassy task is a state machine, not a thread. It has **no stack of its
//! own**: every task on the thread-mode executor is polled on the stack of
//! whatever is running the executor, which here is the main thread. So
//! "size the radio task's stack" is really two separate obligations, and they
//! are met in two separate places:
//!
//! - **Stack.** `RmtTx::transmit_frame` needs roughly 6.5 KB, nearly all of it
//!   in `somfy_rmt::build_symbols`'s two fixed 320-pulse buffers. Those are
//!   locals of a synchronous call, so they land on the main stack, and
//!   `main::check_stack_headroom` refuses to start if that stack is smaller
//!   than [`crate::REQUIRED_STACK_BYTES`].
//! - **Futures.** What a task holds *across an await* lives in a static sized
//!   by `embassy-executor` from the future's real type. Being wrong about it is
//!   a linker error — a DRAM overflow — not silent corruption, which is why
//!   there is no number to choose here.
//!
//! The 4a finding that "a default Embassy task is smaller than 6.5 KB" is about
//! neither of those. It described `embassy-executor`'s task *arena*, a fixed
//! pool that older versions carved task futures out of at spawn time; 0.10
//! removed it in favour of exactly-sized statics. There is no arena to size.

use embassy_executor::task;
use embassy_futures::select::{select, select3, select4, Either, Either3, Either4};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::pubsub::{ImmediatePublisher, Subscriber};
use embassy_time::{Duration, Instant, Ticker, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{delay::Delay, gpio::Output, spi::master::Spi, Blocking};
use heapless::{String, Vec};
use somfy_api::{ApiErrorCode, GroupDto, RoomDto, ShadeDto};
use somfy_config::{Catalog, CatalogError};
use somfy_domain::{
    allocate_with, AllocateError, DomainError, GroupId, Registry, RemoteIdentity, RoomId,
    ShadeCommand, ShadeId, StateDelta, DELTA_CAPACITY,
};
use somfy_rts::{Frame, RollingCode};
use somfy_store::{seed_if_absent, RegionState, Seeded};
use somfy_tasks::{
    ControlCommand, Dispatch, RadioEvent, RadioLoop, StateMachine, TransmitQueueHandle,
    COMMAND_QUEUE_DEPTH, DELTA_QUEUE_DEPTH, DELTA_SUBSCRIBERS, FRAME_QUEUE_DEPTH,
    TRANSMIT_QUEUE_DEPTH,
};

use crate::edits::{AckReceiver, EditReceiver, EventSender, ShadeAck, ShadeEdit, ShadeEvent};
use crate::radio::{air::Air, rmt_rx::RmtPulseSource};
use crate::rpc;
use crate::shades::ShadeStore;
use crate::store::{FlashStore, StoreError};

/// The mutex kind every channel in this firmware uses.
///
/// A critical section rather than the no-op mutex: the RMT and timer interrupt
/// handlers run outside the executor, so a channel touched from an interrupt
/// context needs real mutual exclusion. Nothing here sends from an interrupt
/// today, but the cost is a few instructions and the failure mode of getting it
/// wrong is corruption that only appears under timing nobody can reproduce.
pub type Mutex = CriticalSectionRawMutex;

/// How often the state task advances its position estimates.
///
/// 100 ms is 1% of a shade's default 10 s travel, so a `GoTo` cannot overshoot
/// its target by more than that before the arrival stop is planned. Faster
/// would cost wake-ups for precision the travel-time model does not have;
/// slower would start showing up as overshoot.
pub const TICK_MS: u64 = 100;

/// The SPI bus the CC1101 hangs off, with its chip-select line.
type RadioSpi = ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, Delay>;

/// The radio loop, with every type parameter resolved.
///
/// Spelled out because `#[embassy_executor::task]` builds a static sized to the
/// task's future and so cannot be generic.
pub type Radio = RadioLoop<
    'static,
    RmtPulseSource<'static>,
    Air<'static, RadioSpi>,
    Mutex,
    TRANSMIT_QUEUE_DEPTH,
    FRAME_QUEUE_DEPTH,
>;

/// Deltas out of the state task, for whoever is listening.
pub type Deltas =
    ImmediatePublisher<'static, Mutex, StateDelta, DELTA_QUEUE_DEPTH, DELTA_SUBSCRIBERS, 1>;

#[allow(
    dead_code,
    reason = "held by the broker session and by each websocket; a build with \
              neither transport publishes deltas into a channel nobody reads, \
              which is what `publish_immediate` is for"
)]
/// The listening end of [`Deltas`]. The MQTT session holds one.
///
/// Publish/subscribe rather than a queue, so a slow subscriber cannot make the
/// state task wait: `publish_immediate` drops for a subscriber that has fallen
/// behind, and the subscriber is told how many it missed. That is the right
/// trade for a delta, which is a report about a position that a later delta
/// reports again — and it is one of the four things that keep the broker from
/// being able to affect radio control.
pub type DeltaSubscriber =
    Subscriber<'static, Mutex, StateDelta, DELTA_QUEUE_DEPTH, DELTA_SUBSCRIBERS, 1>;

#[allow(
    dead_code,
    reason = "held by the broker session; the web server reaches the same \
              `run_command` through `crate::rpc` instead, so a build with only \
              `http` never names this"
)]
/// The writing end of the command channel, for whatever produces commands.
///
/// Handed to the MQTT session, which uses `try_send` and never `send`: a full
/// queue must drop the newest command rather than park the sender. See
/// [`crate::mqtt`].
pub type CommandSender = Sender<'static, Mutex, ControlCommand, COMMAND_QUEUE_DEPTH>;

/// One in this many repeats of an anomaly is logged, after the first.
///
/// See [`radio`] for why a log line on the receive path is expensive. The first
/// occurrence is always reported — it is the one that tells you the receiver is
/// alive and hearing something — and after that the rate is bounded, so a storm
/// costs a line every so often rather than a line each.
const ANOMALY_LOG_INTERVAL: u32 = 64;

/// Whether this occurrence of an anomaly is one to say out loud.
fn worth_reporting(count: u32) -> bool {
    count == 1 || count.is_multiple_of(ANOMALY_LOG_INTERVAL)
}

/// Sole owner of the CC1101 and both RMT channels.
///
/// ## What it does and does not print
///
/// A received frame is published and **not** logged here, which is a timing
/// decision rather than a stylistic one. A repeat frame follows the one just
/// decoded by `somfy_rts::TIMINGS::INTER_FRAME_GAP`, and the reception that
/// delivered the first one ends `somfy_rmt::IDLE_THRESHOLD_US` into that gap —
/// leaving about 5 ms to re-arm the receiver before the repeat's first edge.
/// A serial line at 115200 baud moves roughly 11 characters per millisecond, so
/// one log line here would consume most of that window. The state task logs the
/// frame instead, by which time this loop is already awaiting the next
/// reception.
///
/// The two receive-side anomalies sit at exactly the same point in that cycle
/// and cost exactly as much, so they are **counted** and reported at a bounded
/// rate rather than printed each time. Neither is as rare as it looks: a
/// marginal signal produces an undecodable burst for every repeat of every
/// press, and a dropped frame happens precisely when the state task is already
/// behind — so printing one per occurrence would slow the radio task most in
/// the two situations where it can least afford it.
///
/// Transmit-side events are printed as they happen: the radio is out of receive
/// for the whole burst anyway, so there is no window left to protect.
#[task]
pub async fn radio(mut radio: Radio) -> ! {
    let mut undecodable = 0u32;
    let mut dropped = 0u32;
    loop {
        match radio.step().await {
            // Published, deliberately silent. See above.
            RadioEvent::Received(_) => {}
            RadioEvent::Undecodable { bit_length } => {
                undecodable = undecodable.saturating_add(1);
                if worth_reporting(undecodable) {
                    esp_println::println!(
                        "radio: undecodable {}-bit burst ({} so far)",
                        bit_length,
                        undecodable,
                    );
                }
            }
            RadioEvent::ReceiveQueueFull(frame) => {
                dropped = dropped.saturating_add(1);
                if worth_reporting(dropped) {
                    esp_println::println!(
                        "radio: dropped a frame for {:#08X} — state task is behind ({} so far)",
                        frame.address,
                        dropped,
                    );
                }
            }
            RadioEvent::SourceFinished => {
                // Not a shutdown: transmission carries on. Worth one line
                // because from here on the controller is deaf, and a deaf
                // controller looks exactly like a quiet house.
                esp_println::println!("radio: receiver stopped — transmit only from here");
            }
            RadioEvent::Transmitted {
                rolling_code,
                frames,
            } => {
                esp_println::println!(
                    "radio: sent {} frame(s), rolling_code={}",
                    frames,
                    rolling_code,
                );
            }
            RadioEvent::TransmitFailed(error) => {
                esp_println::println!("radio: transmit failed: {:?}", error);
            }
            RadioEvent::Unencodable(error) => {
                esp_println::println!("radio: request could not be encoded: {:?}", error);
            }
        }
    }
}

/// Owns the `somfy-domain` controller, the rolling-code store, and the only
/// handle that can reach the radio's queue.
///
/// ## Where the flash writes land
///
/// Every commit this firmware performs happens on this task, inside
/// `somfy_store::transmit`, and therefore immediately before a transmission.
/// That matters because `esp-storage` runs flash operations with interrupts
/// disabled: a sector erase — one commit in sixteen — costs tens of
/// milliseconds during which the RMT receiver hears nothing. Putting it
/// immediately before a burst means the deaf window abuts one the burst was
/// about to open anyway, rather than opening a new one at an unrelated moment.
/// `somfy_tasks::state`'s module docs carry the full argument.
///
/// The [`yield_now`] after a dispatch is the other half: it hands the executor
/// to the radio task at once, so the gap between the commit finishing and the
/// radio keying up stays as short as a single-threaded executor allows.
#[task]
pub async fn state(
    mut machine: StateMachine,
    mut store: FlashStore<'static>,
    mut table: Table,
    mut queue: TransmitQueueHandle<'static, Mutex, TRANSMIT_QUEUE_DEPTH>,
    frames: Receiver<'static, Mutex, Frame, FRAME_QUEUE_DEPTH>,
    commands: Receiver<'static, Mutex, ControlCommand, COMMAND_QUEUE_DEPTH>,
    deltas: Deltas,
) -> ! {
    let Table {
        ref mut shades,
        ref mut catalog,
        identity,
        edits,
        acks,
        ref events,
    } = table;
    let mut ticker = Ticker::every(Duration::from_millis(TICK_MS));
    loop {
        // The table's own clock. `due_at` is `None` when nothing is pending, and
        // a far-future deadline then keeps this arm out of the way rather than
        // making it a fourth thing that is always ready. See
        // `somfy_config::Catalog` for why a shade table is debounced while a
        // rolling code is not.
        let write_due = catalog.due_at();
        let persist = async {
            match write_due {
                Some(at) => {
                    Timer::at(Instant::from_millis(at)).await;
                }
                None => core::future::pending::<()>().await,
            }
        };

        // `select4` polls its arms in order and returns on the first that is
        // ready, so a permanently-ready earlier arm would starve a later one.
        // The order is chosen for that: a command is the thing a person is
        // waiting on, an edit is a person too but a rarer one, a frame is an
        // observation, and the last arm is either the tick that plans arrival
        // stops or the flash write that is already overdue. It is safe because
        // none of them can be continuously ready — the channels are drained
        // faster than any radio or any person can fill them, and the ticker
        // fires at `TICK_MS`.
        // The web server's requests ride in this arm rather than in a fifth,
        // because `select4` is as wide as `embassy-futures` goes and because
        // they belong here: an HTTP request *is* a person, arriving through a
        // different door than the broker's. It sits after the two channels for
        // the same ordering reason the arms themselves are ordered — nothing
        // here can be continuously ready, so this is a preference and not a
        // starvation risk.
        let edit_or_ack = async {
            match select3(edits.receive(), acks.receive(), rpc::RPC.next()).await {
                Either3::First(edit) => Edited::Edit(edit),
                Either3::Second(ack) => Edited::Ack(ack),
                Either3::Third(request) => Edited::Request(request),
            }
        };
        let tick_or_write = async {
            match select(ticker.next(), persist).await {
                Either::First(()) => Timed::Tick,
                Either::Second(()) => Timed::Persist,
            }
        };
        let event = select4(
            commands.receive(),
            edit_or_ack,
            frames.receive(),
            tick_or_write,
        )
        .await;
        // Sampled after the wait, not before it: the wait is the part that
        // takes time, and the domain dead-reckons from this number.
        let now_ms = Instant::now().as_millis();
        // Declared after the await, not before it, so this 32-slot buffer is
        // not live across the wait and therefore not carried in the task's
        // statically-allocated future.
        let mut emitted: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();

        let dispatched = match event {
            Either4::First(command) => run_command(
                &mut machine,
                &mut store,
                &mut queue,
                command,
                now_ms,
                &mut emitted,
            )
            .unwrap_or(false),
            Either4::Second(Edited::Edit(edit)) => {
                // The answer is discarded, and that is the whole difference
                // between this arm and the one below: a queue has nowhere to
                // put a refusal, so it is logged by `apply_edit` on the way
                // past. What the edit *does* is identical.
                let _ = apply_edit(
                    machine.registry_mut(),
                    catalog,
                    &mut store,
                    &identity,
                    events,
                    edit,
                    now_ms,
                );
                false
            }
            Either4::Second(Edited::Request(request)) => serve_request(
                request,
                &mut machine,
                &mut store,
                &mut queue,
                catalog,
                &identity,
                events,
                now_ms,
                &mut emitted,
            ),
            // The other half of the round trip a removal makes: the broker has
            // cleared the entities, so the persisted bit that names them may go
            // — and not before. `somfy_config::Catalog` carries the ordering.
            Either4::Second(Edited::Ack(ack)) => {
                match ack {
                    ShadeAck::Announced { id } => catalog.mark_announced(id, now_ms),
                    ShadeAck::Retired { id } => catalog.mark_retired(id, now_ms),
                }
                false
            }
            Either4::Third(frame) => {
                esp_println::println!(
                    "state: heard {:?} from {:#08X} (code {})",
                    frame.command,
                    frame.address,
                    frame.rolling_code,
                );
                machine.on_rx_frame(&frame, now_ms, &mut emitted);
                false
            }
            Either4::Fourth(Timed::Tick) => {
                let dispatch = machine.tick(&mut store, &mut queue, now_ms, &mut emitted);
                report(&dispatch)
            }
            // The debounce has run out. **Written here, on the state task**,
            // because this is the task that owns both the flash and the
            // registry the record is built from — and because a write is an
            // erase with interrupts disabled, which must not happen on the
            // radio task's clock.
            Either4::Fourth(Timed::Persist) => {
                persist_table(&mut store, shades, catalog, machine.registry());
                false
            }
        };

        for delta in &emitted {
            deltas.publish_immediate(*delta);
        }

        if dispatched {
            // Let the radio task pick the request up before this one does
            // anything else — in particular before it can reach another flash
            // commit. See the note on this task.
            yield_now().await;
        }
    }
}

/// Everything the state task needs in order to change the shade table and say
/// so.
///
/// One value rather than six arguments, and the six are genuinely one thing: a
/// change arrives on `edits`, is applied to `catalog`, is written through
/// `shades`, is announced on `events`, and is confirmed back on `acks`. A board
/// with no shade region has `shades: None` and everything else still works —
/// the change is applied and commandable, and only its durability is lost.
pub struct Table {
    /// The flash region, if the partition table has one.
    pub shades: Option<ShadeStore>,
    /// The table as the controller believes it, plus the debounce.
    pub catalog: Catalog,
    /// This controller's virtual-remote identity, which is what a new shade's
    /// address is allocated from.
    pub identity: RemoteIdentity,
    /// Changes coming in.
    pub edits: EditReceiver,
    /// Confirmations coming back from the broker session.
    pub acks: AckReceiver,
    /// Changes going out to the broker session.
    pub events: EventSender,
}

/// Which of the three message kinds the second select arm delivered.
enum Edited {
    /// Somebody asked for a change to the table.
    Edit(ShadeEdit),
    /// The broker session finished announcing or retiring one.
    Ack(ShadeAck),
    /// The web server asked something and is waiting to be told.
    Request(rpc::Request),
}

/// Apply one command and say whether anything reached the radio.
///
/// **The one place a `ControlCommand` is acted on**, and therefore the answer to
/// "do HTTP and MQTT share a path". They do not merely call the same
/// `StateMachine::apply`; they arrive at the same six lines around it, so the
/// logging, the error handling and the `yield_now` that follows a dispatch are
/// one implementation rather than two that agree today.
///
/// `Err` is the domain's refusal, which the HTTP caller renders and the queue
/// caller has already had logged.
fn run_command(
    machine: &mut StateMachine,
    store: &mut FlashStore<'static>,
    queue: &mut TransmitQueueHandle<'static, Mutex, TRANSMIT_QUEUE_DEPTH>,
    command: ControlCommand,
    now_ms: u64,
    emitted: &mut Vec<StateDelta, DELTA_CAPACITY>,
) -> Result<bool, DomainError> {
    match machine.apply(store, queue, command, now_ms, emitted) {
        Ok(dispatch) => Ok(report(&dispatch)),
        Err(error) => {
            esp_println::println!("state: {:?} rejected: {:?}", command, error);
            Err(error)
        }
    }
}

/// Answer one request from the web server, and say whether anything reached the
/// radio.
///
/// Every arm is a call into something that already existed: the registry for a
/// read, [`run_command`] for a movement, [`apply_edit`] for a change. Nothing
/// here decides anything a person could disagree with — except one thing, and
/// it is the pairing rule, which is here because only this task can see the
/// address it is a rule about.
#[allow(
    clippy::too_many_arguments,
    reason = "these are the state task's own locals, passed rather than \
              captured so that this stays a function a reader can check \
              against the arm it replaced"
)]
fn serve_request(
    request: rpc::Request,
    machine: &mut StateMachine,
    store: &mut FlashStore<'static>,
    queue: &mut TransmitQueueHandle<'static, Mutex, TRANSMIT_QUEUE_DEPTH>,
    catalog: &mut Catalog,
    identity: &RemoteIdentity,
    events: &EventSender,
    now_ms: u64,
    emitted: &mut Vec<StateDelta, DELTA_CAPACITY>,
) -> bool {
    let (reply, dispatched) = match request {
        rpc::Request::ShadeFrom(slot) => (
            rpc::Reply::Shade(
                machine
                    .registry()
                    .shades()
                    .find(|(id, _)| id.0 >= slot)
                    .map(|(id, shade)| ShadeDto::from_shade(id, shade)),
            ),
            false,
        ),
        rpc::Request::Shade(id) => (
            rpc::Reply::Shade(
                machine
                    .registry()
                    .shade(id)
                    .map(|shade| ShadeDto::from_shade(id, shade)),
            ),
            false,
        ),
        rpc::Request::GroupFrom(slot) => (rpc::Reply::Group(group_from(machine, slot)), false),
        rpc::Request::RoomFrom(slot) => (rpc::Reply::Room(room_from(machine, slot)), false),
        rpc::Request::Command(command) => {
            match run_command(machine, store, queue, command, now_ms, emitted) {
                Ok(dispatched) => (rpc::Reply::Done, dispatched),
                // Every refusal a movement can draw is about the target not
                // being there — `command_shade` and `command_group` both start
                // by looking it up. Anything else is this device's fault and
                // has just been logged by `run_command`.
                Err(DomainError::NotFound) => (rpc::Reply::Refused(ApiErrorCode::NotFound), false),
                Err(_) => (rpc::Reply::Refused(ApiErrorCode::InvalidAddress), false),
            }
        }
        // **The one rule that lives here.** A `Prog` burst at an address this
        // controller did not allocate teaches the motor an address it already
        // obeys, so the person standing at the shade watches for a jog that
        // means nothing — and the two-controllers-one-identity clash survives
        // the whole procedure. The broker surface enforces the same rule by
        // never giving such a shade a pairing button (`Inventory::snapshot`);
        // HTTP has no button to withhold, so it refuses instead.
        rpc::Request::Pair(id) => match machine.registry().shade(id) {
            None => (rpc::Reply::Refused(ApiErrorCode::NotFound), false),
            Some(shade) if !RemoteIdentity::is_allocated(shade.config.address) => (
                rpc::Reply::Refused(ApiErrorCode::AddressNotAllocated),
                false,
            ),
            Some(_) => {
                let command = ControlCommand::Shade {
                    id,
                    command: ShadeCommand::Pair,
                };
                match run_command(machine, store, queue, command, now_ms, emitted) {
                    Ok(dispatched) => (rpc::Reply::Done, dispatched),
                    Err(_) => (rpc::Reply::Refused(ApiErrorCode::InvalidAddress), false),
                }
            }
        },
        rpc::Request::Edit(edit) => (
            match apply_edit(
                machine.registry_mut(),
                catalog,
                store,
                identity,
                events,
                edit,
                now_ms,
            ) {
                Ok(Applied::Added(id)) => rpc::Reply::Created(id),
                Ok(Applied::Changed) => rpc::Reply::Done,
                Err(code) => rpc::Reply::Refused(code),
            },
            false,
        ),
    };
    rpc::RPC.answer(reply);
    dispatched
}

/// The first group at or after `slot`, as a wire DTO.
///
/// A free function because the registry exposes groups one accessor at a time —
/// name, membership and existence are three calls — and doing that inline would
/// bury the arm it belongs to.
fn group_from(machine: &StateMachine, slot: u8) -> Option<GroupDto> {
    let registry = machine.registry();
    (slot..somfy_domain::MAX_GROUPS as u8)
        .map(GroupId)
        .find(|id| registry.group_exists(*id))
        .map(|id| GroupDto {
            id: id.0,
            name: name_of(registry.group_name(id)),
            shade_ids: registry.group_shades(id).map(|shade| shade.0).collect(),
        })
}

/// The first room at or after `slot`, as a wire DTO.
fn room_from(machine: &StateMachine, slot: u8) -> Option<RoomDto> {
    let registry = machine.registry();
    (slot..somfy_domain::MAX_ROOMS as u8)
        .map(RoomId)
        .find(|id| registry.room_name(*id).is_some())
        .map(|id| RoomDto {
            id: id.0,
            name: name_of(registry.room_name(id)),
            shade_ids: registry.room_shades(id).map(|shade| shade.0).collect(),
        })
}

/// Copy a registry name into the wire type's buffer.
///
/// The two capacities are both 32 and both are `heapless::String<32>`, so this
/// cannot truncate; `heapless`' `push_str` is all-or-nothing in any case, so the
/// failure it cannot have would be an empty name rather than a corrupted one.
fn name_of(name: Option<&str>) -> String<32> {
    let mut held = String::new();
    if let Some(name) = name {
        let _ = held.push_str(name);
    }
    held
}

/// Which of the two the last select arm delivered.
enum Timed {
    /// The estimator's clock.
    Tick,
    /// The shade table's debounce ran out.
    Persist,
}

/// What an edit did, for a caller that is waiting to be told.
///
/// The fire-and-forget path throws it away; the HTTP surface turns it into a
/// response body, and `Added` is the one that carries something no client could
/// have known — the id the registry assigned and the address this controller
/// allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// A shade was created.
    Added(ShadeId),
    /// A shade was changed, removed, linked or unlinked.
    Changed,
}

/// Apply one edit to the registry and the table, and tell the broker session.
///
/// # The only place an edit happens
///
/// Both transports reach this function and neither contains any part of it: the
/// HTTP surface calls it through `crate::rpc` and renders what it returns,
/// and anything arriving on [`crate::edits::EditChannel`] calls it and logs.
/// That is what "one command path, two transports" means concretely — a rule
/// added here is added for both, and there is nowhere else for one of them to
/// disagree.
///
/// # Refusals leave nothing half-applied
///
/// An edit that cannot be applied must not leave the registry and the table
/// disagreeing, which is why each arm either completes or returns before
/// touching the second one — and why `Add` validates the configuration inside
/// `allocate_with`, before the address is spent.
///
/// # It opens a deaf window that is not next to a burst, and that is accepted
///
/// `somfy_tasks::state`'s docs argue that every flash erase in this firmware
/// sits immediately before a transmission, so the tens of milliseconds it holds
/// interrupts down extend a window the burst was about to open anyway. **`Add`
/// breaks that**: `seed_if_absent` writes a rolling-code seed, and nothing is
/// transmitted afterwards.
///
/// It is accepted rather than avoided, for reasons that do not generalise:
/// adding a shade is a rare, human-initiated act; the person doing it is at a
/// browser rather than at a wall remote, so there is no press to miss; and the
/// alternative — deferring the seed — would leave a shade that exists and
/// cannot transmit until some later write, which is a worse failure than a lost
/// reception. The debounced *table* write is separate and already lands on the
/// state task's own clock (`somfy_config::Catalog::due_at`).
///
/// The honest residue is that a wall-remote press arriving in the same tens of
/// milliseconds as somebody clicking "add shade" is lost. Nothing reports it,
/// because a lost reception and a quiet band are indistinguishable.
fn apply_edit(
    registry: &mut Registry,
    catalog: &mut Catalog,
    store: &mut FlashStore<'static>,
    identity: &RemoteIdentity,
    events: &EventSender,
    edit: ShadeEdit,
    now_ms: u64,
) -> Result<Applied, ApiErrorCode> {
    match edit {
        ShadeEdit::Add { request } => {
            // The id is the registry's lowest free slot, and the address is
            // allocated inside the branch where that slot is empty — see
            // `somfy_domain::allocate_with` for why reallocating is not a thing
            // it declines to do but a thing it cannot express.
            let id = free_shade_id(registry).ok_or(ApiErrorCode::RegistryFull)?;
            // `to_config` is the *same* validator `PATCH` runs, and it runs
            // here — at the allocated address, before the shade exists — so a
            // request the API would refuse never costs an address.
            let allocated =
                allocate_with(registry, identity, id, |address| request.to_config(address))
                    .map_err(|error| match error {
                        AllocateError::Description(code) => code,
                        AllocateError::Domain(error) => {
                            // Unreachable: the id came from `free_shade_id`, so
                            // it is in range and its slot is empty, and the
                            // allocator probes one more candidate than the
                            // registry can hold. Reported rather than
                            // `expect`ed, because a panic reboots the board.
                            esp_println::println!("shades: allocator refused ({error:?})");
                            ApiErrorCode::InvalidAddress
                        }
                    })?;
            let address = allocated.address();
            let name = registry
                .shade(id)
                .map_or_else(String::new, |shade| shade.config.name.clone());

            // **A freshly allocated address needs a rolling code before its
            // first transmission.** `seed_if_absent` is where that rule lives:
            // it reads first and writes only into the branch where the store
            // held nothing, so this cannot move an existing counter backwards
            // — which is what would stop a motor obeying.
            //
            // The region state is `Intact` because this address is new by
            // construction: it came from the allocator, which stepped over
            // every address the table holds. A damaged region is a reason not
            // to believe an *empty read for an old address*, and there is no
            // old address here.
            let seed = RollingCode(1);
            match seed_if_absent(store, address, seed, RegionState::Intact) {
                Ok(Seeded::Planted(code)) => {
                    esp_println::println!(
                        "shades: ShadeId({}) '{}' allocated {:#08X}, rolling code seeded at {}",
                        id.0,
                        name,
                        address,
                        code.0,
                    );
                }
                Ok(other) => {
                    // Unreachable: the address is new. Reported rather than
                    // ignored, because the two ways it could happen — an
                    // address the allocator handed out twice, or a store that
                    // remembers one it should not — are both worth knowing.
                    esp_println::println!(
                        "shades: ShadeId({}) allocated {:#08X} and the store answered {:?}",
                        id.0,
                        address,
                        other,
                    );
                }
                Err(error) => {
                    // The shade exists and cannot transmit. Left in place
                    // rather than rolled back: removing it would burn the
                    // address, and a shade that reports `NoStoredCode` is
                    // recoverable by a person who is told.
                    esp_println::println!(
                        "shades: ShadeId({}) at {:#08X} has no rolling code ({:?}) — it will \
                         refuse to transmit until the region is repaired",
                        id.0,
                        address,
                        error,
                    );
                }
            }

            catalog.add(id, seed, now_ms);
            announce(
                events,
                ShadeEvent::Added {
                    id,
                    name,
                    pairable: RemoteIdentity::is_allocated(address),
                },
            );
            Ok(Applied::Added(id))
        }
        // **Announced as `Added` again, deliberately.** A discovery config is
        // retained and keyed by the entity's `unique_id`, so republishing one
        // with a new name replaces it in place — which is what a rename is. The
        // broker session's `Inventory::insert` and `Known::track` are both
        // idempotent on the id, so nothing else moves: the position it has
        // observed survives, and no entity is created or orphaned.
        ShadeEdit::Reconfigure { id, patch } => {
            let current = &registry.shade(id).ok_or(ApiErrorCode::NotFound)?.config;
            // The same validator `POST /api/v1/shades` runs, resolved against
            // the shade's real current configuration — which is why the patch
            // travels rather than a config built when the request arrived.
            let next = patch.apply(current)?;
            let name = next.name.clone();
            catalog
                .reconfigure(registry, id, next, now_ms)
                .map_err(catalog_refusal)?;
            esp_println::println!("shades: ShadeId({}) reconfigured as '{}'", id.0, name);
            announce(
                events,
                ShadeEvent::Added {
                    id,
                    name,
                    pairable: registry
                        .shade(id)
                        .is_some_and(|shade| RemoteIdentity::is_allocated(shade.config.address)),
                },
            );
            Ok(Applied::Changed)
        }
        ShadeEdit::Remove { id } => {
            // **The entities go before the shade does.** The record is written
            // with the shade gone and its announced bit still set, so from here
            // on flash names an orphan — which is the only thing that can name
            // it once the id is out of the registry.
            catalog
                .remove(registry, id, now_ms)
                .map_err(catalog_refusal)?;
            esp_println::println!("shades: ShadeId({}) removed", id.0);
            announce(events, ShadeEvent::Removed { id });
            Ok(Applied::Changed)
        }
        ShadeEdit::Link { id, address } => {
            catalog
                .link(registry, id, address, now_ms)
                .map_err(catalog_refusal)?;
            esp_println::println!(
                "shades: ShadeId({}) now follows the remote at {:#08X}",
                id.0,
                address,
            );
            Ok(Applied::Changed)
        }
        ShadeEdit::Unlink { id, address } => {
            catalog
                .unlink(registry, id, address, now_ms)
                .map_err(catalog_refusal)?;
            esp_println::println!(
                "shades: ShadeId({}) no longer follows the remote at {:#08X}",
                id.0,
                address,
            );
            Ok(Applied::Changed)
        }
    }
}

/// Translate a table refusal into the vocabulary the UI can translate.
///
/// Only three of `CatalogError`'s causes are things a person can act on, and
/// the rest share the one 5xx the contract has — which is the honest place for
/// them, because each is either unreachable from the API surface or a fault in
/// this device rather than in the request. They are printed on the way past so
/// that "500" is never the whole story anybody gets.
fn catalog_refusal(error: CatalogError) -> ApiErrorCode {
    match error {
        CatalogError::Domain(DomainError::NotFound) => ApiErrorCode::NotFound,
        CatalogError::Domain(DomainError::RegistryFull) => ApiErrorCode::RegistryFull,
        CatalogError::Domain(DomainError::NameTooLong) => ApiErrorCode::NameTooLong,
        other => {
            esp_println::println!("shades: the table refused a change ({other})");
            ApiErrorCode::InvalidAddress
        }
    }
}

/// The lowest id the registry has free, or `None` if it is full.
///
/// Asked here rather than by `add_shade`, because the address has to be
/// allocated *for* an id and the allocation must happen inside the branch where
/// the slot is empty.
fn free_shade_id(registry: &Registry) -> Option<ShadeId> {
    (0..somfy_domain::MAX_SHADES as u8)
        .map(ShadeId)
        .find(|id| registry.shade(*id).is_none())
}

/// Tell the broker session what changed, or say that nothing heard.
///
/// `try_send`, never `send`: a board with no broker provisioned has nothing
/// draining this queue, and a state task parked on it would stop estimating
/// positions and stop planning arrival stops. Anything dropped here is
/// recovered by the next full announcement, which is built from the table.
fn announce(events: &EventSender, event: ShadeEvent) {
    if events.try_send(event).is_err() {
        esp_println::println!(
            "shades: nothing is listening for entity changes — the broker will catch up on \
             its next session"
        );
    }
}

/// Write the shade table, and report whichever way it went.
///
/// The debounce is cleared **only** on a durable write. A failure leaves the
/// table dirty, so the next deadline tries again rather than silently believing
/// in a record the flash did not take.
fn persist_table(
    store: &mut FlashStore<'static>,
    shades: &mut Option<ShadeStore>,
    catalog: &mut Catalog,
    registry: &Registry,
) {
    // No region, nothing to write to. The change stays in memory and the
    // controller keeps working; `written` is called anyway so the deadline does
    // not fire again every debounce for a region that is not coming back.
    let Some(shades) = shades.as_mut() else {
        esp_println::println!(
            "shades: there is no shade region, so this change will not survive a reboot"
        );
        catalog.written();
        return;
    };
    let (record, dropped) = catalog.record(registry);
    if dropped.links > 0 {
        esp_println::println!(
            "shades: {} linked remote(s) did not fit the record's pool and will not survive \
             the next boot",
            dropped.links,
        );
    }
    if dropped.seeds > 0 {
        // Unreachable, and loud because of what it would cost if it were not:
        // a zero seed is ignored while the rolling-code region holds a code for
        // that address, and planted the moment that region is lost — at which
        // point the motor stops obeying and only a walk to it fixes that.
        esp_println::println!(
            "shades: {} shade(s) were written with a rolling-code seed of 0 because this \
             table holds none for them. That is a bug, and it costs a re-pairing if the \
             rolling-code region is ever lost.",
            dropped.seeds,
        );
    }
    let shade_count = record.shades.len();
    // One borrow of the flash, for the length of this call. See
    // `FlashStore::with_flash` for why a borrow rather than a second owner.
    match store.with_flash(|flash| shades.store(flash, &record)) {
        Ok(seq) => {
            catalog.written();
            esp_println::println!(
                "shades: table written (seq {}, {} shade(s), {} link(s))",
                seq,
                shade_count,
                record.links.len(),
            );
        }
        // Left dirty on purpose: the next deadline retries. A controller that
        // believed in a table the flash refused would lose every shade at the
        // next boot with nothing to say why.
        Err(error) => esp_println::println!(
            "shades: the table could not be written ({:?}) — it will be retried",
            error,
        ),
    }
}

/// Log whatever went wrong, and report whether anything reached the radio.
fn report(dispatch: &Dispatch<StoreError, somfy_tasks::QueueFull>) -> bool {
    if let Some(error) = &dispatch.first_error {
        esp_println::println!(
            "state: {} of {} planned frame(s) sent; first failure: {:?}",
            dispatch.sent,
            dispatch.planned,
            error,
        );
    }
    dispatch.sent > 0
}
