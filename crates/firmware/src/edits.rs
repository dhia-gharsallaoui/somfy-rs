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
use somfy_domain::ShadeId;
use somfy_mqtt::MAX_NAME_LEN;

use crate::tasks::Mutex;

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
/// **Nothing in this image constructs one yet**, and that is the honest state
/// of it: the producer is the device's API surface, which is a separate piece
/// of work. What is here is the consumer — the state task applies every variant
/// below, persists it and tells the broker — so the API layer has a seam to
/// send into rather than a registry to reach across.
#[allow(
    dead_code,
    reason = "the producer is the API surface, which is not in this image yet; \
              every variant is applied by `tasks::apply_edit`"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadeEdit {
    /// Add a shade, allocating it an address from this controller's own space.
    ///
    /// No address here, deliberately: the caller does not choose one. An
    /// address is allocated once, by `somfy_domain::allocate_if_absent`, and
    /// never moves — a motor paired at one address obeys that address, and
    /// nothing in a one-way protocol can tell it otherwise.
    Add {
        /// What to call it. The only thing a person supplies.
        name: String<MAX_NAME_LEN>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadeEvent {
    /// A shade now exists and has no entities yet.
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
pub type EventReceiver = Receiver<'static, Mutex, ShadeEvent, EVENT_QUEUE_DEPTH>;
/// The sending end of the acknowledgement channel, as the broker session holds
/// it.
pub type AckSender = Sender<'static, Mutex, ShadeAck, EVENT_QUEUE_DEPTH>;
/// The receiving end of the acknowledgement channel, as the state task holds
/// it.
pub type AckReceiver = Receiver<'static, Mutex, ShadeAck, EVENT_QUEUE_DEPTH>;
