//! The ordering helper, and the token that makes the ordering unforgeable.

use crate::store::RollingCodeStore;
use somfy_rts::{Command, Frame};

/// Which RTS frame width a request is to be sent as.
///
/// Not derivable from the command alone: extended commands
/// ([`Command::is_extended`]) force 80 bits, but a base command may be sent
/// either way depending on what the motor was paired as. The caller decides;
/// the radio task is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameBits {
    /// 7-byte frame — [`somfy_rts::encode56`].
    Bits56,
    /// 10-byte frame — [`somfy_rts::encode80`].
    Bits80,
}

/// One transmission for the radio task: a fully-determined frame, its width,
/// and how many repeats follow it.
///
/// The rolling code inside `frame` is the value that was persisted before this
/// request existed. The radio task must transmit it as given and must not
/// re-derive it — re-deriving is how the persisted value and the transmitted
/// value drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransmitRequest {
    pub frame: Frame,
    pub bits: FrameBits,
    /// Repeat frames sent after the first frame. All repeats carry the same
    /// rolling code as the first frame — one button press is one code.
    pub repeats: u8,
}

/// What to transmit, before a rolling code has been assigned to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransmitPlan {
    /// 24-bit remote address.
    pub address: u32,
    pub command: Command,
    pub bits: FrameBits,
    pub repeats: u8,
}

/// Proof that a rolling code was durably committed, carrying the transmission
/// that commit authorises.
///
/// This type is the enforcement mechanism. Its single field is private, it has
/// no public constructor, and it derives neither `Clone`, `Copy` nor `Default`,
/// so **no code outside this crate can create one**. Since
/// [`TransmitQueue::enqueue`] accepts nothing else, and the only function that
/// mints a ticket is [`transmit`] — after [`RollingCodeStore::commit`] has
/// returned `Ok` — a caller cannot reach a queue without having committed
/// first. Getting the order wrong is not a mistake to be avoided by review; it
/// is a program that does not compile.
///
/// `enqueue` takes the ticket **by value** and the type is not `Clone`, so one
/// commit authorises at most one enqueue.
///
/// A ticket cannot be built from the outside — the field is private and there
/// is no constructor:
///
/// ```compile_fail,E0451
/// use somfy_rts::{Command, Frame};
/// use somfy_store::{FrameBits, TransmitRequest, TransmitTicket};
///
/// let request = TransmitRequest {
///     frame: Frame { key: 0xA0, command: Command::Up, rolling_code: 1, address: 0x11 },
///     bits: FrameBits::Bits56,
///     repeats: 2,
/// };
/// let ticket = TransmitTicket { request };
/// ```
///
/// Nor can one be reused, so a single commit cannot authorise two frames:
///
/// ```compile_fail,E0599
/// use somfy_store::{TransmitQueue, TransmitTicket};
///
/// struct Queue;
/// impl TransmitQueue for Queue {
///     type Error = ();
///     fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), ()> {
///         let second = ticket.clone();
///         let _ = (ticket, second);
///         Ok(())
///     }
/// }
/// ```
#[derive(Debug)]
pub struct TransmitTicket {
    request: TransmitRequest,
}

impl TransmitTicket {
    /// Inspect the authorised transmission without consuming the ticket.
    pub fn request(&self) -> &TransmitRequest {
        &self.request
    }

    /// Consume the ticket, yielding the transmission it authorises.
    ///
    /// Intended for queue implementations, which are handed a ticket and need
    /// the request inside it. Unwrapping a ticket you were given does not let
    /// you make another one.
    pub fn into_request(self) -> TransmitRequest {
        self.request
    }
}

/// The producer end of the radio task's transmit channel.
///
/// Implementations wrap a real bounded channel. The trait deliberately accepts
/// only a [`TransmitTicket`], never a bare [`TransmitRequest`]: that is what
/// stops a call site reaching the channel without committing first. An
/// implementation must not expose a second, ticket-free way in — doing so
/// reintroduces exactly the failure this seam removes.
///
/// Handing a queue a request that no commit stands behind does not compile:
///
/// ```compile_fail,E0308
/// use somfy_rts::{Command, Frame};
/// use somfy_store::{FrameBits, TransmitQueue, TransmitRequest, TransmitTicket};
///
/// struct Queue;
/// impl TransmitQueue for Queue {
///     type Error = ();
///     fn enqueue(&mut self, _ticket: TransmitTicket) -> Result<(), ()> { Ok(()) }
/// }
///
/// let request = TransmitRequest {
///     frame: Frame { key: 0xA0, command: Command::Up, rolling_code: 1, address: 0x11 },
///     bits: FrameBits::Bits56,
///     repeats: 2,
/// };
/// Queue.enqueue(request).unwrap();
/// ```
pub trait TransmitQueue {
    /// Why the request could not be queued — a full channel, typically.
    type Error;

    /// Hand the authorised transmission to the radio task.
    ///
    /// A failure here happens *after* the code was committed, so it skips a
    /// code rather than replaying one. That is the safe direction and is a
    /// deliberate consequence of committing first.
    fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), Self::Error>;
}

/// Why a [`transmit`] call put nothing on the air.
///
/// Every variant means **no frame was transmitted**, with one nuance:
/// [`Queue`](TransmitError::Queue) means the code was already committed, so
/// the counter has advanced past a frame nobody sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmitError<S, Q> {
    /// The store has no record for this address.
    ///
    /// Reported rather than seeded, so an erased or unreadable region can never
    /// masquerade as a fresh pairing and replay codes the motor has seen. Seed
    /// a genuinely new address with an explicit
    /// [`RollingCodeStore::commit`].
    NoStoredCode { address: u32 },
    /// The store failed to read or to commit. Nothing was transmitted.
    Store(S),
    /// The code was committed; the radio queue would not take the request.
    Queue(Q),
}

/// Commit the next rolling code, then — and only then — queue the frame.
///
/// The only way to reach a [`TransmitQueue`], because it is the only source of
/// the [`TransmitTicket`] that `enqueue` requires.
///
/// Order of operations, which is the whole point:
///
/// 1. `load` the next-to-send code. Missing or unreadable stops here.
/// 2. Build the frame from it and advance the counter.
/// 3. `commit` the advanced counter. A failure stops here, so a frame carrying
///    an uncommitted code never exists, let alone reaches a queue.
/// 4. Mint the ticket and enqueue it.
///
/// On success, returns the rolling code the frame carries — the value *before*
/// the increment, since the stored value is next-to-send.
///
/// ```
/// use somfy_rts::{Command, RollingCode};
/// use somfy_store::{
///     transmit, FrameBits, RollingCodeStore, TransmitPlan, TransmitQueue, TransmitTicket,
/// };
///
/// struct Store(Option<RollingCode>);
/// impl RollingCodeStore for Store {
///     type Error = ();
///     fn load(&mut self, _address: u32) -> Result<Option<RollingCode>, ()> { Ok(self.0) }
///     fn commit(&mut self, _address: u32, code: RollingCode) -> Result<(), ()> {
///         self.0 = Some(code);
///         Ok(())
///     }
/// }
///
/// struct Queue(Vec<u16>);
/// impl TransmitQueue for Queue {
///     type Error = ();
///     fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), ()> {
///         self.0.push(ticket.request().frame.rolling_code);
///         Ok(())
///     }
/// }
///
/// let mut store = Store(Some(RollingCode(42)));
/// let mut queue = Queue(Vec::new());
/// let plan = TransmitPlan {
///     address: 0x00_1234,
///     command: Command::Up,
///     bits: FrameBits::Bits56,
///     repeats: 2,
/// };
///
/// assert_eq!(transmit(&mut store, &mut queue, plan), Ok(42));
/// assert_eq!(queue.0, vec![42]);   // the frame carries 42...
/// assert_eq!(store.0, Some(RollingCode(43)));  // ...and 43 was persisted first
/// ```
pub fn transmit<S, Q>(
    store: &mut S,
    queue: &mut Q,
    plan: TransmitPlan,
) -> Result<u16, TransmitError<S::Error, Q::Error>>
where
    S: RollingCodeStore,
    Q: TransmitQueue,
{
    // 1. What is next to send? A missing record is a fact to report, not a
    //    gap to fill with a plausible-looking default.
    let mut code = store
        .load(plan.address)
        .map_err(TransmitError::Store)?
        .ok_or(TransmitError::NoStoredCode {
            address: plan.address,
        })?;

    // 2. `next_frame` puts the current code in the frame and advances `code`
    //    to the next-to-send value. Deriving the frame here, from the loaded
    //    counter, is what keeps the transmitted code and the persisted code
    //    the same number.
    let frame = code.next_frame(plan.command, plan.address);

    // 3. Persist. Everything after this line is unreachable if the write did
    //    not land, so no frame carrying an uncommitted code can exist.
    store
        .commit(plan.address, code)
        .map_err(TransmitError::Store)?;

    // 4. Only now does a ticket — the sole key to any queue — come into being.
    let ticket = TransmitTicket {
        request: TransmitRequest {
            frame,
            bits: plan.bits,
            repeats: plan.repeats,
        },
    };
    queue.enqueue(ticket).map_err(TransmitError::Queue)?;

    Ok(frame.rolling_code)
}
