//! The three messages a runtime change to the shade table travels on.
//!
//! # Why three channels and not one shared registry
//!
//! For the same reason [`crate::inventory`] is a snapshot: the registry belongs
//! to the state task, and nothing may reach across that boundary. A shared
//! registry behind a mutex would mean the broker session holding a lock the
//! state task needs to plan an arrival stop — the first crack in the separation
//! that keeps a broker from being able to affect radio control at all.
//!
//! So a change is a message, and it makes a round trip:
//!
//! 1. [`ShadeEdit`] — someone asks for a change. The producer is whatever the
//!    device exposes to a person; the consumer is the state task, which owns
//!    the registry and is the only thing that may touch it.
//! 2. [`ShadeEvent`] — the state task says what it did. The consumer is the
//!    broker session, which announces or retires the entities.
//! 3. [`ShadeAck`] — the broker session says the entities are on or off the
//!    broker. The consumer is the state task, which is what persists that fact.
//!
//! **The third one is not bookkeeping for its own sake.** Removing a shade
//! leaves a retained discovery config behind, and clearing it needs an id
//! nothing else in the system can produce once the shade is gone. The persisted
//! `announced` bit is what names it, and the bit may only be cleared *after*
//! the tombstones have landed — otherwise a power cut between the two loses the
//! only record that the entities exist. `somfy_config::Catalog`'s docs carry
//! the ordering; this is the wire it travels on.
//!
//! # What happens when there is no broker
//!
//! Nothing bad, and nothing silent. [`ShadeEvent`]s are sent with `try_send`,
//! so a board with no broker provisioned fills the queue once and then drops
//! them with a line on the serial console. The shade is still added, still
//! commandable and still persisted; only the announcement is missed, and the
//! next broker session announces from the table anyway.

use embassy_sync::channel::{Channel, Receiver, Sender};
use heapless::String;
use somfy_api::{CreateShadeDto, PatchShadeDto};
use somfy_domain::ShadeId;

use crate::tasks::Mutex;

/// Longest shade name this vocabulary carries, in bytes.
///
/// Thirty-two, which is `somfy_domain::ShadeConfig::name`'s own capacity and
/// therefore the most a shade can be called. It used to be `somfy_mqtt`'s
/// constant of the same value, and **that was a coupling rather than a
/// coincidence**: an edit is a change to the shade table, not a message to a
/// broker, so a build with no broker in it had no business needing a constant
/// from one. Feature-gating the transports is what made the borrowing visible;
/// see `crates/firmware/Cargo.toml`.
pub const MAX_NAME_LEN: usize = 32;

/// Edits waiting for the state task.
///
/// Four: an edit is a person pressing a button, and a queue deeper than the
/// number of buttons a person can press before the state task drains it buys
/// nothing.
pub const EDIT_QUEUE_DEPTH: usize = 4;

/// Events waiting for the broker session.
///
/// Deeper than the edit queue, because the broker session can be away for a
/// whole reconnect backoff while edits continue. Anything dropped is recovered
/// by the next full announcement, which is built from the table rather than
/// from this queue.
pub const EVENT_QUEUE_DEPTH: usize = 8;

/// A change to the shade table, as asked for.
///
/// # One vocabulary, two ways of arriving
///
/// Every variant below is applied by exactly one function —
/// [`crate::tasks::apply_edit`] — and there are two ways to reach it. The HTTP
/// surface hands one over [`crate::rpc`] and waits for the answer, because
/// `POST /api/v1/shades` owes the client the id and the address the device just
/// allocated and no other party can produce them. Anything that does not need
/// an answer sends one down [`EditChannel`] and carries on.
///
/// The difference is the *transport*, and it is a real one: a request/response
/// protocol has somewhere to put a refusal and a fire-and-forget queue does
/// not. What must never differ is what the edit *does*, which is why neither
/// path contains any of it.
#[allow(
    dead_code,
    reason = "`Link` and `Unlink` have no producer yet — the wall-remote screen \
              is a later piece of work — and both are applied by \
              `tasks::apply_edit` exactly as the other three are"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadeEdit {
    /// Add a shade, allocating it an address from this controller's own space.
    ///
    /// No address in the request, deliberately: the caller does not choose one.
    /// An address is allocated once, by `somfy_domain::allocate_with`, and never
    /// moves — a motor paired at one address obeys that address, and nothing in
    /// a one-way protocol can tell it otherwise. The configuration is built
    /// *at* the allocated address by [`CreateShadeDto::to_config`], which is
    /// also where every rule about what a shade may be lives.
    Add {
        /// The request, unvalidated. It is validated where the address is
        /// known, because one of the rules is about the address.
        request: CreateShadeDto,
    },
    /// Change a shade that already exists.
    ///
    /// Carries the *patch*, not the resulting configuration, so that the
    /// absent-means-unchanged rule is resolved against the shade's real current
    /// state at the moment it is applied rather than against a copy taken when
    /// the request arrived.
    Reconfigure {
        /// Which one.
        id: ShadeId,
        /// What to change.
        patch: PatchShadeDto,
    },
    /// Record that an operator reported this shade working, which is what
    /// makes it announceable.
    ///
    /// **Not a claim about the motor**, and the name says so: RTS is one-way,
    /// so nothing here observed anything. What happened is that a person
    /// commanded the shade, watched it move, and said so — see
    /// `somfy_domain::PairingState`.
    ///
    /// It carries only an id because there is nothing else to carry. A payload
    /// would be a value a client could vary, and the only variation available
    /// is "unconfirm", which would retire the entities of a working shade.
    ConfirmPairing {
        /// Which one.
        id: ShadeId,
    },
    /// Remove a shade, and everything the broker holds for it.
    Remove {
        /// Which one.
        id: ShadeId,
    },
    /// Register a wall remote whose presses drive this shade's position
    /// estimate.
    Link {
        /// Which shade.
        id: ShadeId,
        /// The remote's 24-bit address, as decoded from one of its frames.
        address: u32,
    },
    /// Forget a wall remote.
    Unlink {
        /// Which shade.
        id: ShadeId,
        /// The remote's address.
        address: u32,
    },
}

/// What the state task did, for the broker session to reflect.
///
/// # There is no event for "a shade was created"
///
/// Creating a shade allocates an address **no motor has ever heard**, so the
/// entities it would announce would appear in Home Assistant, accept commands,
/// and drive nothing. That is the failure this vocabulary is shaped to make
/// unreachable: [`Added`](ShadeEvent::Added) is emitted when an operator
/// reports the shade working, not when the record is written, and
/// [`Removed`](ShadeEvent::Removed) is emitted only for a shade that reached
/// that point — a setup abandoned halfway has nothing on the broker, so there
/// is nothing to clear and nothing is published.
///
/// `crates/firmware/src/tasks.rs`'s `announce_shade` is the one gate, and every
/// producer goes through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadeEvent {
    /// A shade an operator has reported working now exists and needs entities.
    Added {
        /// Its id.
        id: ShadeId,
        /// Its name, which the broker session needs and the registry will not
        /// lend it.
        name: String<MAX_NAME_LEN>,
        /// Whether this controller allocated its address, and therefore whether
        /// it is offered a pairing button. Decided here because the address is
        /// the state task's to know.
        pairable: bool,
    },
    /// A shade no longer exists and its entities must go.
    Removed {
        /// Its id.
        id: ShadeId,
    },
}

/// What the broker session did, for the state task to persist.
#[allow(
    dead_code,
    reason = "the producer is the broker session, which a build without `mqtt` \
              does not have; the state task applies both either way"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeAck {
    /// The entities are on the broker.
    Announced {
        /// Which shade.
        id: ShadeId,
    },
    /// The entities have been cleared from the broker, and the persisted
    /// `announced` bit may now be cleared with them.
    Retired {
        /// Which shade.
        id: ShadeId,
    },
}

/// The channel edits travel on.
pub type EditChannel = Channel<Mutex, ShadeEdit, EDIT_QUEUE_DEPTH>;
/// The channel events travel on.
pub type EventChannel = Channel<Mutex, ShadeEvent, EVENT_QUEUE_DEPTH>;
/// The channel acknowledgements travel on.
pub type AckChannel = Channel<Mutex, ShadeAck, EVENT_QUEUE_DEPTH>;

/// The receiving end of the edit channel, as the state task holds it.
pub type EditReceiver = Receiver<'static, Mutex, ShadeEdit, EDIT_QUEUE_DEPTH>;
/// The sending end of the event channel, as the state task holds it.
pub type EventSender = Sender<'static, Mutex, ShadeEvent, EVENT_QUEUE_DEPTH>;
/// The receiving end of the event channel, as the broker session holds it.
#[allow(dead_code, reason = "held by the broker session, which `mqtt` gates")]
pub type EventReceiver = Receiver<'static, Mutex, ShadeEvent, EVENT_QUEUE_DEPTH>;
/// The sending end of the acknowledgement channel, as the broker session holds
/// it.
#[allow(dead_code, reason = "held by the broker session, which `mqtt` gates")]
pub type AckSender = Sender<'static, Mutex, ShadeAck, EVENT_QUEUE_DEPTH>;
/// The receiving end of the acknowledgement channel, as the state task holds
/// it.
pub type AckReceiver = Receiver<'static, Mutex, ShadeAck, EVENT_QUEUE_DEPTH>;
