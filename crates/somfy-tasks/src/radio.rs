//! The radio loop: sole owner of the radio, and the only thing that is allowed
//! to be slow.
//!
//! One iteration ([`RadioLoop::step`]) waits for whichever comes first — a
//! pulse off the air, or a transmission the state task has authorised — and
//! deals with it. Nothing else in the firmware touches the radio, so nothing
//! else can put a symbol on the air out of turn or steal the receiver
//! mid-burst.
//!
//! The loop is written against [`PulseSource`] and [`Transmitter`] rather than
//! against hardware, which is what lets a real wall-remote capture drive the
//! whole receive path on the host through [`somfy_rmt::ReplayPulseSource`], and
//! lets a recording transmitter pin the keying order of a burst without a
//! radio. That is the entire reason both traits exist.

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use heapless::Vec;
use somfy_rmt::{PulseSource, FRAME56_BYTES, FRAME80_BYTES};
use somfy_rts::{decode56, decode80, encode56, encode80, Frame, FrameError, FrameKind, RxDecoder};
use somfy_store::{FrameBits, TransmitRequest};

/// Decoded frames the state task may have waiting.
///
/// A physical remote's press arrives as a first frame plus its repeats, and
/// `somfy_domain::Controller` collapses those to one logical event — but the
/// collapsing happens on the state task, so every repeat travels through this
/// channel. Eight holds a full press from each of two remotes pressed at once,
/// which is already an unlikely amount of traffic for a shade controller.
///
/// Overflow costs an overheard frame, i.e. a position estimate that stays where
/// it was. That is a degraded estimate, not a wrong one, and it is a far
/// cheaper failure than making the radio task wait on the state task.
pub const FRAME_QUEUE_DEPTH: usize = 8;

/// The bounded channel carrying decoded frames from the radio task to the
/// state task.
///
/// A plain `embassy_sync` channel, not a wrapper: unlike the transmit
/// direction there is no invariant to protect here. Anyone may publish a frame
/// they decoded — a frame carries no rolling-code obligation, since receiving
/// one is an observation rather than an action.
pub type FrameChannel<M, const N: usize = FRAME_QUEUE_DEPTH> = Channel<M, Frame, N>;

/// Putting an encoded frame on the air.
///
/// Split into keying and sending rather than offered as one "transmit this
/// burst" call, so that the burst's *shape* — key the radio, first frame, its
/// repeats, park the radio whatever happened — lives here in a crate the host
/// can test, and the implementation is left with three operations that each do
/// exactly one thing to a chip.
///
/// A Somfy transmitter is keyed around a whole burst, not around each frame:
/// the CC1101 only radiates while it is in TX, and a real remote holds the
/// carrier across the frames of one press.
pub trait Transmitter {
    /// Why a frame did not reach the air.
    type Error;

    /// Put the radio into transmit and wait for it to be ready to radiate.
    ///
    /// Implementations must not return until the transmitter is actually keyed.
    /// A CC1101 calibrates its synthesiser after the strobe and radiates
    /// nothing until that finishes, so returning early costs the leading edge
    /// of the wake-up pulse — a frame that goes out shortened with nothing
    /// anywhere reporting it.
    fn key_on(&mut self) -> Result<(), Self::Error>;

    /// Clock one encoded frame out, and return once it is fully sent.
    // No `Send` bound on the returned future, for the same reason
    // `somfy_rmt::PulseSource` has none: the executor polling it is
    // single-threaded, and the bound would rule out an implementation holding a
    // peripheral handle across the await — which the RMT one must.
    #[allow(async_fn_in_trait)]
    async fn send_frame(&mut self, bytes: &[u8], kind: FrameKind) -> Result<(), Self::Error>;

    /// Take the radio out of transmit and return it to receiving.
    ///
    /// Called after every burst, successful or not. Leaving a synthesiser
    /// running holds the band and wastes power, and leaving the chip out of
    /// receive mode would silently end reception for good.
    fn key_off(&mut self) -> Result<(), Self::Error>;
}

/// What one turn of the radio loop did.
///
/// Returned rather than logged so that the caller decides what is worth saying
/// out loud — and so that host tests can assert on the loop's behaviour without
/// inspecting a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioEvent<E> {
    /// A frame was decoded off the air and handed to the state task.
    Received(Frame),
    /// A burst completed the right number of bits but did not decode — a
    /// failed checksum, or a bit length this crate has no decoder for. Almost
    /// always a marginal signal rather than a bug, which is why it is counted
    /// rather than escalated.
    Undecodable { bit_length: u8 },
    /// A frame decoded but the state task's channel was full, so it was
    /// dropped. Costs a position estimate; see [`FRAME_QUEUE_DEPTH`].
    ReceiveQueueFull(Frame),
    /// The pulse source will never yield another pulse. The loop stops asking
    /// and keeps serving transmissions, because a dead receiver is no reason to
    /// stop moving shades.
    SourceFinished,
    /// A burst went out: the rolling code it carried, and how many frames of
    /// the burst were sent.
    Transmitted { rolling_code: u16, frames: u8 },
    /// The radio refused somewhere in the burst. The radio was parked
    /// regardless.
    TransmitFailed(E),
    /// The request could not be encoded, so nothing was keyed at all. Only
    /// reachable for commands `somfy-rts` refuses to put in a 56-bit frame.
    Unencodable(FrameError),
}

/// The radio task's body.
///
/// Owns the pulse source, the transmitter, and both ends it needs of the two
/// channels. Construct it in the task, then call [`RadioLoop::step`] forever.
pub struct RadioLoop<'ch, S, T, M, const TXN: usize, const RXN: usize>
where
    M: RawMutex,
{
    source: S,
    transmitter: T,
    requests: Receiver<'ch, M, TransmitRequest, TXN>,
    frames: Sender<'ch, M, Frame, RXN>,
    /// One decoder for the life of the task, deliberately never reset between
    /// bursts: every burst opens with the hardware-sync preamble, whose 2560 µs
    /// half-pulses match neither timing family the data phase accepts, so the
    /// decoder re-acquires on its own. Resetting at a burst boundary would also
    /// throw away a frame split across two receptions.
    decoder: RxDecoder,
    /// Set once the source reports it is finished, so the loop stops polling a
    /// source that would otherwise resolve instantly forever and spin the
    /// executor at full tilt.
    source_finished: bool,
}

impl<'ch, S, T, M, const TXN: usize, const RXN: usize> RadioLoop<'ch, S, T, M, TXN, RXN>
where
    S: PulseSource,
    T: Transmitter,
    M: RawMutex,
{
    /// Take ownership of the radio and both channel ends.
    pub fn new(
        source: S,
        transmitter: T,
        requests: Receiver<'ch, M, TransmitRequest, TXN>,
        frames: Sender<'ch, M, Frame, RXN>,
    ) -> Self {
        Self {
            source,
            transmitter,
            requests,
            frames,
            decoder: RxDecoder::new(),
            source_finished: false,
        }
    }

    /// Wait for the next thing to happen, and deal with it.
    ///
    /// ## Which one wins, and what cancelling costs
    ///
    /// A pending transmission takes the radio out of receive, so the two arms
    /// are genuinely exclusive and `select` is the right shape: whichever
    /// resolves first, the other is dropped. Dropping a part-received burst
    /// loses it — the receiver is stopped and its buffer discarded — which is
    /// unavoidable, because keying the transmitter would have destroyed that
    /// reception anyway. Dropping the request arm loses nothing at all: an
    /// `embassy_sync` receive future takes its message only on the poll that
    /// resolves it, so a cancelled receive leaves the queue untouched.
    ///
    /// ## Why transmitting is allowed to be slow here
    ///
    /// A burst is roughly 100 ms a frame and this loop is inside it for the
    /// whole time. That is the point of the split: the radio task blocks on the
    /// radio, and nothing else does. The state task is delayed, not damaged —
    /// its position estimator reads a timestamp rather than counting ticks, so
    /// a late tick produces a late delta, never a wrong position.
    pub async fn step(&mut self) -> RadioEvent<T::Error> {
        // Destructured so the two arms borrow disjoint fields; `select` needs
        // both futures alive at once, and one of them takes `&mut` of the
        // source and the decoder.
        let Self {
            source,
            transmitter,
            requests,
            frames,
            decoder,
            source_finished,
        } = self;

        if *source_finished {
            let request = requests.receive().await;
            return transmit(transmitter, &request).await;
        }

        match select(requests.receive(), receive(source, decoder, frames)).await {
            Either::First(request) => transmit(transmitter, &request).await,
            Either::Second(reception) => {
                if matches!(reception, Reception::Finished) {
                    *source_finished = true;
                }
                reception.into()
            }
        }
    }
}

/// What pumping the pulse source produced. Separate from [`RadioEvent`] only
/// because this half of the loop knows nothing about the transmitter's error
/// type.
enum Reception {
    Frame(Frame),
    Undecodable { bit_length: u8 },
    QueueFull(Frame),
    Finished,
}

impl<E> From<Reception> for RadioEvent<E> {
    fn from(reception: Reception) -> Self {
        match reception {
            Reception::Frame(frame) => RadioEvent::Received(frame),
            Reception::Undecodable { bit_length } => RadioEvent::Undecodable { bit_length },
            Reception::QueueFull(frame) => RadioEvent::ReceiveQueueFull(frame),
            Reception::Finished => RadioEvent::SourceFinished,
        }
    }
}

/// Pump pulses until one of them completes a frame, or the source is done.
///
/// Awaits inside the loop rather than returning after every pulse, because a
/// pulse that completes nothing is not an event anybody wants to hear about —
/// a single press is several hundred of them.
async fn receive<S, M, const RXN: usize>(
    source: &mut S,
    decoder: &mut RxDecoder,
    frames: &Sender<'_, M, Frame, RXN>,
) -> Reception
where
    S: PulseSource,
    M: RawMutex,
{
    loop {
        let Some(pulse) = source.next_pulse().await else {
            return Reception::Finished;
        };
        let Some(raw) = decoder.push(pulse) else {
            continue;
        };
        let frame = match decode(&raw.bytes, raw.bit_length) {
            Some(frame) => frame,
            None => {
                return Reception::Undecodable {
                    bit_length: raw.bit_length,
                }
            }
        };
        // `try_send`, never `send`: waiting here would hold the radio task on
        // the state task, which is the one thing this split exists to prevent.
        return match frames.try_send(frame) {
            Ok(()) => Reception::Frame(frame),
            Err(_) => Reception::QueueFull(frame),
        };
    }
}

/// Turn a completed bit stream into a frame, or report that it is not one.
///
/// A wrong bit length is folded into the same `None` as a failed checksum on
/// purpose: both mean "the air produced something this controller cannot act
/// on", and the caller reports the length either way.
fn decode(bytes: &[u8], bit_length: u8) -> Option<Frame> {
    match bit_length {
        56 => decode56(bytes.try_into().ok()?).ok(),
        80 => decode80(bytes.try_into().ok()?).ok(),
        _ => None,
    }
}

/// Key the radio, send the first frame and its repeats, then park the radio.
///
/// Parking happens on every path once the radio has been keyed, including a
/// failed frame — a synthesiser left running after a failure holds the band and
/// stops the receiver hearing anything ever again. A send failure outranks a
/// parking failure in the report, because it is the one that explains why
/// nothing moved.
async fn transmit<T: Transmitter>(
    transmitter: &mut T,
    request: &TransmitRequest,
) -> RadioEvent<T::Error> {
    // Encoded before anything is keyed: a request that cannot be encoded should
    // not put a carrier on the air at all.
    if let Err(error) = encode(request, 0) {
        return RadioEvent::Unencodable(error);
    }

    if let Err(error) = transmitter.key_on() {
        // Parked even though nothing was keyed, and this is the case where it
        // matters most: keying goes through IDLE, so a failure *between* the
        // strobes leaves the radio neither transmitting nor receiving. Without
        // this the controller would go deaf from the first failed burst, and a
        // deaf controller looks exactly like a quiet house.
        let _ = transmitter.key_off();
        return RadioEvent::TransmitFailed(error);
    }

    let mut sent = 0u8;
    let mut failure = None;
    for repeat in 0..=request.repeats {
        // Re-encoded per repeat, not encoded once and resent. An 80-bit frame
        // re-encodes its tail for each repeat index (`somfy_rts::encode80`), so
        // reusing the first frame's bytes would put the wrong tail on every
        // repeat. A 56-bit frame encodes identically every time, so one code
        // path covers both and the width cannot be got wrong here.
        let bytes = match encode(request, repeat) {
            Ok(bytes) => bytes,
            Err(error) => {
                // Unreachable: repeat 0 encoded above, and neither width's
                // encoder can start failing at a later repeat index. Reported
                // rather than unwrapped, because a panic in the radio task
                // takes the whole controller off the air.
                let _ = transmitter.key_off();
                return RadioEvent::Unencodable(error);
            }
        };
        let kind = if repeat == 0 {
            FrameKind::First
        } else {
            FrameKind::Repeat
        };
        match transmitter.send_frame(&bytes, kind).await {
            Ok(()) => sent += 1,
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }

    let parked = transmitter.key_off();
    match (failure, parked) {
        (Some(error), _) => RadioEvent::TransmitFailed(error),
        (None, Err(error)) => RadioEvent::TransmitFailed(error),
        (None, Ok(())) => RadioEvent::Transmitted {
            rolling_code: request.frame.rolling_code,
            frames: sent,
        },
    }
}

/// Encode one frame of a burst at `repeat` (0 = first frame).
///
/// The rolling code inside `request.frame` is transmitted as given and never
/// re-derived: it is the value the store already committed, and re-deriving it
/// is exactly how the persisted code and the transmitted code drift apart.
fn encode(request: &TransmitRequest, repeat: u8) -> Result<Vec<u8, FRAME80_BYTES>, FrameError> {
    let mut bytes: Vec<u8, FRAME80_BYTES> = Vec::new();
    match request.bits {
        FrameBits::Bits56 => {
            let encoded = encode56(&request.frame)?;
            // Infallible: the buffer's capacity is the wider frame's length.
            let _ = bytes.extend_from_slice(&encoded);
        }
        FrameBits::Bits80 => {
            let encoded = encode80(&request.frame, repeat);
            let _ = bytes.extend_from_slice(&encoded);
        }
    }
    Ok(bytes)
}

// The encode buffer above is sized to the wider frame, so neither width can
// overflow it. Asserted rather than assumed: `extend_from_slice`'s failure is
// discarded there, and a buffer one byte short would silently hand the
// transmitter a truncated frame.
const _: () = assert!(
    FRAME56_BYTES <= FRAME80_BYTES,
    "the encode buffer must hold either frame width"
);
