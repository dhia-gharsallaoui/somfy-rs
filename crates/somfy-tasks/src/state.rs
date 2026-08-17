//! The state loop: the domain, the store, and the only path to the radio.
//!
//! [`StateMachine`] wraps `somfy_domain::Controller` and adds the one thing the
//! domain deliberately does not own — rolling codes. Every frame the domain
//! plans leaves through [`somfy_store::transmit`], which commits the code before
//! it enqueues anything, so the ordering is not a rule this module follows, it
//! is the only route out.
//!
//! ## Flash writes stop reception, and this is where they happen
//!
//! `esp-storage` runs every flash operation with interrupts disabled on the
//! core. A read is short; an **erase is not** — the ring erases a sector once
//! every [`SectorRing::slots_per_sector`](somfy_store::SectorRing::slots_per_sector)
//! commits, which is one button press in sixteen, and a 4 KB erase is tens of
//! milliseconds typical with a datasheet worst case in the hundreds. RMT
//! reception during that window is simply lost, and moving the commit to
//! another task does not help, because the core is the core.
//!
//! The window cannot be removed. What can be chosen is **where it lands**, and
//! it lands in the best available place for a reason worth stating:
//!
//! - A commit only ever happens on the way to a transmission. [`somfy_store::transmit`]
//!   is the sole commit path in the running controller, and it commits and then
//!   immediately enqueues, so every erase is followed within microseconds by a
//!   request the radio task picks up.
//! - Servicing that request keys the radio into **transmit**, which stops
//!   reception outright for the length of the burst — roughly 100 ms a frame.
//!   So the erase does not open a new deaf window; it extends by a few tens of
//!   milliseconds one that was about to open anyway.
//! - Any other placement would be worse. A periodic checkpoint, a
//!   write-behind flush, a commit on a timer: each would deafen the receiver at
//!   a moment unrelated to anything the controller was doing, which is exactly
//!   the kind of loss that never explains itself.
//!
//! What remains is the sliver between the commit returning and the radio task
//! keying up. It is small — the enqueue is the next statement, and on a
//! single-threaded executor the radio task runs as soon as this one awaits —
//! but it is not zero, and a frame arriving inside it is lost. That is the
//! honest residue of the design; nothing here pretends otherwise.
//!
//! Note also that a `load` scans the whole ring, so an ordinary command costs
//! two full scans before it costs an erase. Those are reads: short critical
//! sections, many of them, rather than one long one. They have not been
//! measured on hardware.
//!
//! ## Overheard frames cannot transmit
//!
//! [`StateMachine::on_rx_frame`] takes neither a store nor a queue. That is the
//! contract from `somfy_domain::Controller` — an overheard frame drives the
//! position estimate and must never be retransmitted, because retransmitting it
//! would double-drive the motor — expressed as a signature rather than as a
//! promise.

use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use heapless::Vec;
use somfy_domain::DomainError;
use somfy_domain::{
    Controller, GroupId, PlannedTx, Registry, ShadeCommand, ShadeId, StateDelta, DELTA_CAPACITY,
    TX_CAPACITY,
};
use somfy_rts::Frame;
use somfy_store::{
    transmit, FrameBits, RollingCodeStore, TransmitError, TransmitPlan, TransmitQueue,
};

/// Commands the state task may have waiting.
///
/// Shallow on purpose: a queue of shade commands is a queue of *intentions*
/// about where a shade should be, and acting on a stale one is worse than
/// dropping it. Four covers a burst of presses without letting a backlog
/// accumulate.
pub const COMMAND_QUEUE_DEPTH: usize = 4;

/// State deltas held for a subscriber that has fallen behind.
pub const DELTA_QUEUE_DEPTH: usize = 16;

/// Subscribers the delta channel supports at once.
///
/// **Zero of them exist in Plan 4.** The state task publishes into a channel
/// nothing is listening to, which is the seam Plan 5's MQTT and websocket
/// consumers plug into. Publishing with no subscribers discards immediately, so
/// the unused seam costs a call and nothing else.
pub const DELTA_SUBSCRIBERS: usize = 4;

/// One command for the state task, addressed to a shade or a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Shade { id: ShadeId, command: ShadeCommand },
    Group { id: GroupId, command: ShadeCommand },
}

/// The bounded channel carrying commands into the state task.
///
/// A plain `embassy_sync` channel: a command carries no rolling-code
/// obligation, because acting on it is what creates one. The obligation
/// attaches on the way *out*, at [`TransmitQueue`].
pub type CommandChannel<M, const N: usize = COMMAND_QUEUE_DEPTH> = Channel<M, ControlCommand, N>;

/// The channel state deltas are published on.
///
/// Publish/subscribe rather than a queue, because the publisher must never
/// wait: a state task blocked on a slow consumer stops estimating positions and
/// stops planning arrival stops. `publish_immediate` drops for a subscriber
/// that has fallen behind, which is the right trade — a delta is a report about
/// a position that a later delta will report again.
pub type DeltaChannel<
    M,
    const N: usize = DELTA_QUEUE_DEPTH,
    const SUBS: usize = DELTA_SUBSCRIBERS,
> = PubSubChannel<M, StateDelta, N, SUBS, 1>;

/// Repeat frames sent after the first frame of a burst.
///
/// A physical remote sends several; two repeats is what this project's own
/// transmit bring-up used and what a motor accepted on air. All repeats carry
/// the same rolling code — one button press is one code.
pub const DEFAULT_REPEATS: u8 = 2;

/// How this controller puts a planned frame on the air.
///
/// Per-controller rather than per-shade, and that is a real limitation: a
/// motor is paired as either a 56-bit or an 80-bit device, so a mixed
/// installation needs the width recorded against each shade. `ShadeConfig`
/// has no field for it, adding one is a change to the migration format as well
/// as to the domain, and no 80-bit-capable hardware exists to test against —
/// so the width is stated once, here, where it is visible, rather than guessed
/// per frame somewhere it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxProfile {
    pub bits: FrameBits,
    pub repeats: u8,
}

impl Default for TxProfile {
    /// 56-bit, two repeats — what every committed capture and every on-air
    /// transmission this project has made so far actually used.
    fn default() -> Self {
        Self {
            bits: FrameBits::Bits56,
            repeats: DEFAULT_REPEATS,
        }
    }
}

/// What became of the frames one call planned.
///
/// A failure is reported rather than propagated because the frames in one call
/// belong to different shades: a group command that cannot reach one shade's
/// stored code must still move the rest. Only the first error is kept — there
/// is no allocator, and the second store failure in a row says nothing the
/// first did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch<S, Q> {
    /// Frames the domain planned.
    pub planned: usize,
    /// Frames that reached the radio queue, each with its rolling code already
    /// committed.
    pub sent: usize,
    /// The first thing that went wrong, if anything did.
    pub first_error: Option<TransmitError<S, Q>>,
}

impl<S, Q> Dispatch<S, Q> {
    fn new(planned: usize) -> Self {
        Self {
            planned,
            sent: 0,
            first_error: None,
        }
    }
}

/// The state task's body: the domain plus the route to the radio.
pub struct StateMachine {
    controller: Controller,
    profile: TxProfile,
}

impl StateMachine {
    /// A controller with no shades yet. Provision them through
    /// [`StateMachine::registry_mut`].
    pub fn new(profile: TxProfile) -> Self {
        Self {
            controller: Controller::new(),
            profile,
        }
    }

    /// The shade/group/room registry, for reading state.
    pub fn registry(&self) -> &Registry {
        &self.controller.registry
    }

    /// The registry, for provisioning.
    ///
    /// Deliberately the registry and not the whole `Controller`: a caller with
    /// a `&mut Controller` could plan frames directly and get a `PlannedTx`
    /// buffer that never reaches a store. That would not break the ordering
    /// invariant — a `PlannedTx` cannot reach a queue either — but it would
    /// produce commands that silently never transmit.
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.controller.registry
    }

    /// Apply one [`ControlCommand`], whichever kind it is.
    ///
    /// What the state task calls; the two methods below are what it calls
    /// *through*, and are public because a bring-up harness has a shade in hand
    /// rather than a channel message.
    pub fn apply<S, Q>(
        &mut self,
        store: &mut S,
        queue: &mut Q,
        command: ControlCommand,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<Dispatch<S::Error, Q::Error>, DomainError>
    where
        S: RollingCodeStore,
        Q: TransmitQueue,
    {
        match command {
            ControlCommand::Shade { id, command } => {
                self.command_shade(store, queue, id, command, now_ms, deltas)
            }
            ControlCommand::Group { id, command } => {
                self.command_group(store, queue, id, command, now_ms, deltas)
            }
        }
    }

    /// Apply a command to one shade, and put every frame it plans on the queue
    /// — each with its rolling code committed first.
    pub fn command_shade<S, Q>(
        &mut self,
        store: &mut S,
        queue: &mut Q,
        id: ShadeId,
        command: ShadeCommand,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<Dispatch<S::Error, Q::Error>, DomainError>
    where
        S: RollingCodeStore,
        Q: TransmitQueue,
    {
        let mut planned: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
        self.controller
            .command_shade(id, command, now_ms, &mut planned, deltas)?;
        Ok(self.dispatch(store, queue, &planned))
    }

    /// Apply a command to every member of a group.
    pub fn command_group<S, Q>(
        &mut self,
        store: &mut S,
        queue: &mut Q,
        group: GroupId,
        command: ShadeCommand,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Result<Dispatch<S::Error, Q::Error>, DomainError>
    where
        S: RollingCodeStore,
        Q: TransmitQueue,
    {
        let mut planned: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
        self.controller
            .command_group(group, command, now_ms, &mut planned, deltas)?;
        Ok(self.dispatch(store, queue, &planned))
    }

    /// Account for a frame overheard on the air.
    ///
    /// Takes no store and no queue, so it cannot transmit — see the module
    /// docs. Frames from addresses this controller does not know, and repeats
    /// of a press it has already seen, are dropped by the domain.
    pub fn on_rx_frame(
        &mut self,
        frame: &Frame,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) {
        self.controller.on_rx_frame(frame, now_ms, deltas);
    }

    /// Advance every shade to `now_ms`, dispatching any arrival-stop frames.
    ///
    /// A tick usually plans nothing at all; it plans a frame only when a
    /// position seek reaches its target and the motor must be told to stop.
    /// Those frames go through the same commit-then-enqueue path as a
    /// commanded one, because they are just as much a transmission.
    pub fn tick<S, Q>(
        &mut self,
        store: &mut S,
        queue: &mut Q,
        now_ms: u64,
        deltas: &mut Vec<StateDelta, DELTA_CAPACITY>,
    ) -> Dispatch<S::Error, Q::Error>
    where
        S: RollingCodeStore,
        Q: TransmitQueue,
    {
        let mut planned: Vec<PlannedTx, TX_CAPACITY> = Vec::new();
        self.controller.tick(now_ms, &mut planned, deltas);
        self.dispatch(store, queue, &planned)
    }

    /// Commit and enqueue each planned frame, in order.
    ///
    /// The whole of this module's contribution to the persist-before-transmit
    /// invariant is that this is the only body in it that touches a queue, and
    /// all it can do is call [`transmit`].
    fn dispatch<S, Q>(
        &self,
        store: &mut S,
        queue: &mut Q,
        planned: &[PlannedTx],
    ) -> Dispatch<S::Error, Q::Error>
    where
        S: RollingCodeStore,
        Q: TransmitQueue,
    {
        let mut outcome = Dispatch::new(planned.len());
        for frame in planned {
            let plan = TransmitPlan {
                address: frame.address,
                command: frame.command,
                bits: self.profile.bits,
                // **The profile is a default, not an override.** The domain
                // plans a `Repeats` policy rather than a count, because two
                // kinds of frame cannot take whatever this controller happens
                // to be configured for: a frame that must not be lost needs a
                // floor above it, and a pairing frame needs an exact count,
                // since the length of a `Prog` burst is what distinguishes
                // pairing a remote from removing one. Resolving here is where
                // the domain's policy and the radio's configuration meet, and
                // it is the only place either is read.
                repeats: frame.repeats.resolve(self.profile.repeats),
            };
            match transmit(store, queue, plan) {
                Ok(_) => outcome.sent += 1,
                Err(error) => {
                    // A full queue stops the whole dispatch; a store failure
                    // does not. The asymmetry is the point.
                    //
                    // A store failure is about **one address** — the next
                    // planned frame belongs to a different shade with its own
                    // record, and must still go out.
                    //
                    // A full queue is about **the radio**, and the next frame
                    // would be refused too — but only after paying for its own
                    // commit first. A full group is 64 planned frames against a
                    // queue four deep, so carrying on would mean 60 more ring
                    // scans, 60 more flash writes and around four sector
                    // erases, each burning a rolling code for a frame nobody
                    // will ever send, and each holding this core with
                    // interrupts disabled while the receiver hears nothing.
                    // That is exactly the deaf window this module's docs argue
                    // must only ever sit next to a burst.
                    let backlogged = matches!(error, TransmitError::Queue(_));
                    if outcome.first_error.is_none() {
                        outcome.first_error = Some(error);
                    }
                    if backlogged {
                        break;
                    }
                }
            }
        }
        outcome
    }
}
