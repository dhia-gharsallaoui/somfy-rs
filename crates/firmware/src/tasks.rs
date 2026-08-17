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
use embassy_futures::select::{select3, Either3};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::pubsub::{ImmediatePublisher, Subscriber};
use embassy_time::{Duration, Instant, Ticker};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{delay::Delay, gpio::Output, spi::master::Spi, Blocking};
use heapless::Vec;
use somfy_domain::{StateDelta, DELTA_CAPACITY};
use somfy_rts::Frame;
use somfy_tasks::{
    ControlCommand, Dispatch, RadioEvent, RadioLoop, StateMachine, TransmitQueueHandle,
    COMMAND_QUEUE_DEPTH, DELTA_QUEUE_DEPTH, DELTA_SUBSCRIBERS, FRAME_QUEUE_DEPTH,
    TRANSMIT_QUEUE_DEPTH,
};

use crate::radio::{air::Air, rmt_rx::RmtPulseSource};
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
    mut queue: TransmitQueueHandle<'static, Mutex, TRANSMIT_QUEUE_DEPTH>,
    frames: Receiver<'static, Mutex, Frame, FRAME_QUEUE_DEPTH>,
    commands: Receiver<'static, Mutex, ControlCommand, COMMAND_QUEUE_DEPTH>,
    deltas: Deltas,
) -> ! {
    let mut ticker = Ticker::every(Duration::from_millis(TICK_MS));
    loop {
        // `select3` polls its arms in order and returns on the first that is
        // ready, so a permanently-ready earlier arm would starve a later one.
        // The order is chosen for that: a command is the thing a person is
        // waiting on, a frame is an observation, and the tick is what plans
        // arrival stops. It is safe because none of the three can be
        // continuously ready — the two channels are drained faster than any
        // radio can fill them, and the ticker fires at `TICK_MS`.
        let event = select3(commands.receive(), frames.receive(), ticker.next()).await;
        // Sampled after the wait, not before it: the wait is the part that
        // takes time, and the domain dead-reckons from this number.
        let now_ms = Instant::now().as_millis();
        // Declared after the await, not before it, so this 32-slot buffer is
        // not live across the wait and therefore not carried in the task's
        // statically-allocated future.
        let mut emitted: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();

        let dispatched = match event {
            Either3::First(command) => {
                match machine.apply(&mut store, &mut queue, command, now_ms, &mut emitted) {
                    Ok(dispatch) => report(&dispatch),
                    Err(error) => {
                        esp_println::println!("state: {:?} rejected: {:?}", command, error);
                        false
                    }
                }
            }
            Either3::Second(frame) => {
                esp_println::println!(
                    "state: heard {:?} from {:#08X} (code {})",
                    frame.command,
                    frame.address,
                    frame.rolling_code,
                );
                machine.on_rx_frame(&frame, now_ms, &mut emitted);
                false
            }
            Either3::Third(()) => {
                let dispatch = machine.tick(&mut store, &mut queue, now_ms, &mut emitted);
                report(&dispatch)
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
