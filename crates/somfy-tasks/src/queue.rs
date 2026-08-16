//! The radio task's transmit channel, and the one handle that can put anything
//! into it.
//!
//! ## What this module is for
//!
//! `somfy-store` establishes half of the persist-before-transmit guarantee:
//! [`TransmitQueue::enqueue`] accepts only a [`TransmitTicket`], a ticket cannot
//! be built from outside that crate, and the only function that mints one does
//! so after [`RollingCodeStore::commit`](somfy_store::RollingCodeStore::commit)
//! has returned `Ok`. So no call site can enqueue a frame whose rolling code is
//! not already in flash.
//!
//! The other half is this module's job, and it is the half a type cannot state
//! on its own. `somfy-store` names an obligation on implementations —
//!
//! > An implementation must not expose a second, ticket-free way in — doing so
//! > reintroduces exactly the failure this seam removes.
//!
//! — and an obligation is only as good as the code that meets it. A queue type
//! that implemented [`TransmitQueue`] *and* also offered an inherent
//! `send(request)` on the same underlying channel would satisfy every type in
//! `somfy-store` and leave the invariant wide open: a caller wanting to skip
//! the commit would simply call the other method.
//!
//! ## How it is kept shut
//!
//! [`TransmitChannel`] owns the channel; its `inner` field is private and no
//! method hands out a reference to it, because `embassy_sync::channel::Channel`
//! carries `send`/`try_send` on the channel itself — a `&Channel` *is* a
//! producer handle.
//!
//! Of the two ends it does hand out:
//!
//! - [`TransmitChannel::requests`] returns a consumer, which cannot send.
//! - [`TransmitChannel::queue`] returns a [`TransmitQueueHandle`], whose
//!   `sender` field is private, which has no inherent methods at all, and which
//!   is reachable only through the [`TransmitQueue`] trait.
//!
//! `TransmitQueueHandle` therefore has exactly one operation — `enqueue`, which
//! demands a ticket — and this crate exports nothing else that can reach the
//! channel's producer side. The compile-fail doctests on
//! [`TransmitQueueHandle`] pin both halves of that: no field access, no method.
//!
//! The enforcement is a **crate** boundary rather than a module one, which
//! matters: `crates/firmware` is where the tasks are wired together, and
//! nothing it can write reaches a private field over here.

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use somfy_store::{TransmitQueue, TransmitRequest, TransmitTicket};

/// Requests the radio task may have waiting behind the one it is sending.
///
/// One button press is one request, and servicing a request takes roughly
/// 100 ms per frame — so a depth of four is about a third of a second of
/// backlog. Deeper would not help: a queue that has been full long enough to
/// matter means the radio cannot keep up, and holding more presses only means
/// acting on them later, which for a shade is worse than not acting at all.
///
/// A full queue is not a lost rolling code. [`somfy_store::transmit`] commits
/// before it enqueues, so a refused request skips a code forward — which a
/// motor accepts — rather than replaying one, which it does not.
pub const TRANSMIT_QUEUE_DEPTH: usize = 4;

/// The queue would not take the request.
///
/// Carries no payload deliberately. The request that failed is the one the
/// caller just built, and `embassy_sync`'s `TrySendError` hands it back inside
/// the error — which for a [`TransmitTicket`] would be a way to get a second
/// look at an authorised transmission after its one enqueue was spent. Dropping
/// it keeps a ticket's single use single.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFull;

/// The bounded channel carrying transmissions from the state task to the radio
/// task.
///
/// Statically allocated by the firmware — `Channel::new` is `const`, so this
/// lives in a `static` with no allocator and no lazy initialisation.
///
/// Generic over the mutex kind so that the firmware can use a
/// critical-section mutex while host tests use the no-op one, without either
/// having to provide the other's runtime.
pub struct TransmitChannel<M: RawMutex, const N: usize = TRANSMIT_QUEUE_DEPTH> {
    /// Private, and deliberately never lent out even by shared reference:
    /// `Channel` carries `send`/`try_send` itself, so a `&Channel` escaping
    /// this module would be a ticket-free producer handle.
    inner: Channel<M, TransmitRequest, N>,
}

impl<M: RawMutex, const N: usize> TransmitChannel<M, N> {
    /// An empty channel, in a `const` context.
    pub const fn new() -> Self {
        Self {
            inner: Channel::new(),
        }
    }

    /// The producer end, as the only thing it can be: a [`TransmitQueue`].
    pub fn queue(&self) -> TransmitQueueHandle<'_, M, N> {
        TransmitQueueHandle {
            sender: self.inner.sender(),
        }
    }

    /// The consumer end, for the radio task.
    pub fn requests(&self) -> Receiver<'_, M, TransmitRequest, N> {
        self.inner.receiver()
    }
}

impl<M: RawMutex, const N: usize> Default for TransmitChannel<M, N> {
    fn default() -> Self {
        Self::new()
    }
}

/// The producer end of a [`TransmitChannel`], usable only through
/// [`TransmitQueue`].
///
/// It has no inherent methods and its one field is private, so `enqueue` is
/// the entire surface — and `enqueue` demands a [`TransmitTicket`], which only
/// [`somfy_store::transmit`] can produce and only after a successful commit.
///
/// The underlying sender is not reachable:
///
/// ```compile_fail,E0616
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// use somfy_tasks::TransmitChannel;
///
/// let channel: TransmitChannel<NoopRawMutex, 2> = TransmitChannel::new();
/// let handle = channel.queue();
/// let sender = handle.sender;
/// ```
///
/// and neither is a ticket-free send:
///
/// ```compile_fail,E0599
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// use somfy_rts::{Command, Frame};
/// use somfy_store::{FrameBits, TransmitRequest};
/// use somfy_tasks::TransmitChannel;
///
/// let channel: TransmitChannel<NoopRawMutex, 2> = TransmitChannel::new();
/// let handle = channel.queue();
/// handle.try_send(TransmitRequest {
///     frame: Frame { key: 0xA0, command: Command::Up, rolling_code: 1, address: 0x11 },
///     bits: FrameBits::Bits56,
///     repeats: 2,
/// });
/// ```
pub struct TransmitQueueHandle<'ch, M: RawMutex, const N: usize = TRANSMIT_QUEUE_DEPTH> {
    sender: Sender<'ch, M, TransmitRequest, N>,
}

impl<M: RawMutex, const N: usize> TransmitQueue for TransmitQueueHandle<'_, M, N> {
    type Error = QueueFull;

    /// Non-blocking on purpose.
    ///
    /// `enqueue` runs inside [`somfy_store::transmit`], which is synchronous
    /// and is called with the rolling code already committed. Waiting for space
    /// here would mean holding the state task on a channel the radio task
    /// drains at radio speed — roughly 100 ms a frame — while the domain's
    /// remaining planned frames, each with a committed code of its own, sat
    /// behind it. Refusing instead reports the backlog immediately and skips
    /// one code, which is the direction this seam is built to fail in.
    fn enqueue(&mut self, ticket: TransmitTicket) -> Result<(), Self::Error> {
        self.sender
            .try_send(ticket.into_request())
            .map_err(|_| QueueFull)
    }
}
