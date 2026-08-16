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
use embassy_sync::channel::Receiver;
use embassy_sync::pubsub::ImmediatePublisher;
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

/// Deltas out of the state task, for whoever is listening — nobody, in Plan 4.
pub type Deltas =
    ImmediatePublisher<'static, Mutex, StateDelta, DELTA_QUEUE_DEPTH, DELTA_SUBSCRIBERS, 1>;

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
/// Anomalies are logged, because they are rare by construction and because a
/// receiver that silently discards is indistinguishable from a quiet band. A
/// completed transmission is logged too: the radio is out of receive for the
/// whole burst anyway, so there is no window left to protect.
#[task]
pub async fn radio(mut radio: Radio) -> ! {
    loop {
        match radio.step().await {
            // Published, deliberately silent. See above.
            RadioEvent::Received(_) => {}
            RadioEvent::Undecodable { bit_length } => {
                esp_println::println!("radio: undecodable {}-bit burst", bit_length);
            }
            RadioEvent::ReceiveQueueFull(frame) => {
                esp_println::println!(
                    "radio: dropped a frame for {:#08X} — state task is behind",
                    frame.address,
                );
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
        let mut emitted: Vec<StateDelta, DELTA_CAPACITY> = Vec::new();
        let event = select3(commands.receive(), frames.receive(), ticker.next()).await;
        // Sampled after the wait, not before it: the wait is the part that
        // takes time, and the domain dead-reckons from this number.
        let now_ms = Instant::now().as_millis();

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
