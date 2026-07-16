//! The [`Controller`] facade: the single object the firmware task talks to.
//!
//! Commands and overheard RX frames go in; the [`PlannedTx`] radio work and the
//! [`StateDelta`] state changes the UI/telemetry layer must surface come out.
//! It owns the [`Registry`] (shades/groups/rooms) and an [`RxDeduper`] and holds
//! the one piece of cross-call state a delta stream needs: the last state it
//! reported per shade slot, so unchanged ticks stay silent.
//!
//! # Contracts
//! - Overheard frames drive the estimate but NEVER retransmit (that would
//!   double-drive the motor) — see [`Shade::apply_overheard`].
//! - `Command::Stop` is never planned by the domain (the `My` button stops a
//!   56-bit motor) — see [`Shade`].

use crate::registry::{GroupId, Registry, ShadeId};
use crate::shade::{PlannedTx, ShadeCommand};
use crate::{Direction, DomainError, Pos};
use heapless::Vec;
use somfy_rts::{Frame, RxDeduper};

/// RX dedupe window: repeats of one button press inside this span collapse to a
/// single logical event. A physical RTS remote fires ~7 repeats over well under
/// a second per press, so 2 s comfortably covers one press without swallowing a
/// deliberate second press.
pub const RX_DEDUPE_WINDOW_MS: u32 = 2_000;

/// Caller-facing TX buffer capacity, sized to the structural worst case so
/// overflow is impossible rather than merely documented: a full group holds
/// [`MAX_SHADES`](crate::registry::MAX_SHADES) = 32 members and
/// [`Shade::handle`] plans at most 2 frames per shade (a sync-crossed arrival
/// stop plus the command's own frame), so `command_group` can plan at most
/// 32 x 2 = **64** frames in one call. `tick` is bounded lower (32 shades x
/// at most 1 arrival-stop frame = 32) and `command_shade` at 2.
pub const TX_CAPACITY: usize = crate::registry::MAX_SHADES * 2;

/// The observable state every shade is assumed to start at (fully open, at
/// rest). A shade sitting at this baseline produces no delta — deltas report
/// *changes* from it, so a freshly added, untouched shade is silent until it
/// actually moves. Also the reset point a slot returns to when a new shade
/// reuses it (see [`Controller::emit_if_changed`]).
const RESTING: (Pos, Pos, Direction) = (Pos::ZERO, Pos::ZERO, Direction::Idle);

/// One shade whose observable state `(pos, tilt_pos, direction)` changed this
/// call. The firmware fans these out to MQTT/websocket/telemetry consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateDelta {
    pub id: ShadeId,
    pub pos: Pos,
    pub tilt_pos: Pos,
    pub direction: Direction,
}

/// Facade the firmware talks to: commands in, [`PlannedTx`] + [`StateDelta`] out.
pub struct Controller {
    pub registry: Registry,
    dedupe: RxDeduper,
    /// Last state reported per shade slot, tagged with the shade's radio
    /// address. The address tag makes slot reuse safe: the registry hands a
    /// freed slot to the next `add_shade`, so a bare `(pos, tilt, dir)` cache
    /// keyed only by slot index could suppress a re-added shade's first delta
    /// when it coincidentally matched the previous occupant. Comparing the
    /// address first treats any slot now holding a *different* shade as having
    /// no baseline (i.e. [`RESTING`]).
    last_emitted: [Option<(u32, Pos, Pos, Direction)>; crate::registry::MAX_SHADES],
}

impl Controller {
    pub fn new() -> Controller {
        Controller {
            registry: Registry::new(),
            dedupe: RxDeduper::new(RX_DEDUPE_WINDOW_MS),
            last_emitted: [None; crate::registry::MAX_SHADES],
        }
    }

    /// Push a [`StateDelta`] for `id` iff its observable state differs from what
    /// was last reported for the shade currently in that slot. A slot whose
    /// cached address does not match the live shade (fresh controller, or the
    /// slot reused by a different shade) is compared against [`RESTING`], so a
    /// re-added shade still emits its first real change and an untouched shade
    /// at rest stays silent.
    fn emit_if_changed(&mut self, id: ShadeId, deltas: &mut Vec<StateDelta, 32>) {
        let Some(shade) = self.registry.shade(id) else {
            return;
        };
        let addr = shade.config.address;
        let now = (shade.pos(), shade.tilt_pos(), shade.direction());
        let slot = &mut self.last_emitted[id.0 as usize];
        let baseline = match *slot {
            Some((a, p, t, d)) if a == addr => (p, t, d),
            _ => RESTING,
        };
        if baseline != now {
            *slot = Some((addr, now.0, now.1, now.2));
            let _ = deltas.push(StateDelta {
                id,
                pos: now.0,
                tilt_pos: now.1,
                direction: now.2,
            });
        }
    }

    /// Drain a shade's local [`PlannedTx`] buffer (capacity 4: `handle`/`tick`
    /// plan at most 2 frames each) into the caller's `tx`. The caller buffer is
    /// sized to [`TX_CAPACITY`], the structural worst case, so a failed push
    /// means the capacity math itself regressed — scream in debug builds.
    fn drain(local: &Vec<PlannedTx, 4>, tx: &mut Vec<PlannedTx, TX_CAPACITY>) {
        for t in local {
            let pushed = tx.push(*t);
            debug_assert!(
                pushed.is_ok(),
                "PlannedTx buffer overflow — capacity math violated"
            );
        }
    }

    /// Apply a command to one shade: update its motion model, queue any radio
    /// frame(s), and emit a delta if its state changed. [`DomainError::NotFound`]
    /// if the slot is empty or out of range.
    ///
    /// Plans at most 2 frames; `tx` is sized to [`TX_CAPACITY`] so a shared
    /// buffer also survives the `command_group`/`tick` worst cases.
    pub fn command_shade(
        &mut self,
        id: ShadeId,
        cmd: ShadeCommand,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, 32>,
    ) -> Result<(), DomainError> {
        let shade = self.registry.shade_mut(id).ok_or(DomainError::NotFound)?;
        let mut local: Vec<PlannedTx, 4> = Vec::new();
        shade.handle(cmd, now_ms, &mut local);
        Self::drain(&local, tx);
        self.emit_if_changed(id, deltas);
        Ok(())
    }

    /// Fan a command out to every member of a group. [`DomainError::NotFound`]
    /// if the group slot is empty or out of range; an existing but empty group
    /// is `Ok(())` with no work.
    ///
    /// `tx` is sized to [`TX_CAPACITY`] = 32 members x 2 frames per
    /// [`Shade::handle`] = 64, the structural worst case of a full group
    /// commanded at once — overflow is impossible, no frame is ever dropped.
    pub fn command_group(
        &mut self,
        g: GroupId,
        cmd: ShadeCommand,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, 32>,
    ) -> Result<(), DomainError> {
        if !self.registry.group_exists(g) {
            return Err(DomainError::NotFound);
        }
        // A group holds at most SOMFY_MAX_GROUPED_SHADES (= MAX_SHADES) members;
        // collect the ids so the fan-out below can take `&mut self`.
        let members: Vec<ShadeId, { crate::registry::MAX_SHADES }> =
            self.registry.group_shades(g).collect();
        for id in members {
            self.command_shade(id, cmd, now_ms, tx, deltas)?;
        }
        Ok(())
    }

    /// Route a decoded RX frame to the shade that owns its address (own or a
    /// linked remote), tracking the estimate without retransmitting. Repeats of
    /// the same press within [`RX_DEDUPE_WINDOW_MS`] and frames from unknown
    /// addresses are ignored.
    pub fn on_rx_frame(&mut self, frame: &Frame, now_ms: u64, deltas: &mut Vec<StateDelta, 32>) {
        // `RxDeduper` keys on a u32 monotonic clock (Plan 1 API). Truncating the
        // u64 is safe here: the deduper's arithmetic is wrapping and the window
        // (2 s) is far shorter than the ~49.7-day u32 ms rollover.
        if !self.dedupe.accept(frame, now_ms as u32) {
            return;
        }
        let Some(id) = self.registry.shade_by_address(frame.address) else {
            return;
        };
        if let Some(shade) = self.registry.shade_mut(id) {
            shade.apply_overheard(frame.command, now_ms);
        }
        self.emit_if_changed(id, deltas);
    }

    /// Advance every shade to `now_ms`: plan any arrival-stop frames and emit a
    /// delta for each shade whose state changed since its last reported one.
    ///
    /// Plans at most 32 frames (32 shades x at most 1 arrival-stop each from
    /// [`Shade::tick`]); `tx` is sized to [`TX_CAPACITY`] = 64 so overflow is
    /// impossible even in the larger `command_group` worst case.
    pub fn tick(
        &mut self,
        now_ms: u64,
        tx: &mut Vec<PlannedTx, TX_CAPACITY>,
        deltas: &mut Vec<StateDelta, 32>,
    ) {
        let ids: Vec<ShadeId, { crate::registry::MAX_SHADES }> =
            self.registry.shades().map(|(id, _)| id).collect();
        for id in ids {
            if let Some(shade) = self.registry.shade_mut(id) {
                let mut local: Vec<PlannedTx, 4> = Vec::new();
                shade.tick(now_ms, &mut local);
                Self::drain(&local, tx);
            }
            self.emit_if_changed(id, deltas);
        }
    }
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}
