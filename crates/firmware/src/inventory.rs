//! The shades the MQTT session announces, as a snapshot taken at boot.
//!
//! # Why a snapshot and not the registry
//!
//! The registry belongs to the state task, and nothing may reach across that
//! boundary — that separation is what keeps a broker from being able to affect
//! radio control at all, and a shared registry behind a mutex would be the
//! first crack in it: the MQTT task would then hold a lock the state task needs
//! to plan an arrival stop.
//!
//! So the ids and names are copied once, in `main`, before either task exists,
//! and the MQTT session works from the copy. Positions are **not** in the
//! snapshot: they change, and they arrive on the delta channel, which is the
//! seam that exists for exactly that.
//!
//! # Where the shades come from
//!
//! The `shades` flash region, read by [`crate::shades`] into the registry
//! before either task exists — so by the time this snapshot is taken, the
//! registry holds exactly what was provisioned, and a board with that region
//! erased still announces availability and no entities, which is the ordinary
//! state of a freshly flashed device.
//!
//! # It is a snapshot, and it is kept up to date
//!
//! Taken once at boot, and then **changed by message** rather than re-read: the
//! state task tells the broker session what it did to the table, and
//! [`Inventory::insert`] and [`Inventory::remove`] apply it here. That is what
//! makes a shade an operator has just confirmed appear in Home Assistant
//! without waiting for a reconnect, without either task holding a lock the
//! other needs.
//!
//! # It holds fewer shades than the registry does, deliberately
//!
//! Only the ones an operator has reported working. A shade that has been
//! created and not yet confirmed has an address this controller invented, which
//! means **no motor has ever heard it** — so its entities would appear in Home
//! Assistant, accept Open and Close, and drive nothing. That is the failure
//! this integration's requirements were written after, and the shade's absence
//! from this snapshot is what prevents it. `somfy_domain::PairingState` carries
//! the argument for why "an operator reported it" is the strongest claim
//! available in a one-way protocol.
//!
//! # It counts the ones it leaves out
//!
//! Excluding them is right and being *silent* about them is not. From Home
//! Assistant's side a setup started and abandoned half-way is invisible: no
//! cover, no button, nothing pending, no difference at all from a controller
//! nobody has touched. The web UI puts it at the top of its dashboard, which
//! helps whoever opens the web UI, and the operator this matters to is the one
//! who does not.
//!
//! So [`Inventory::awaiting_setup`] is carried alongside the ids and published
//! as `somfy_mqtt::DeviceEntity::AwaitingSetup`. It is a **number about the
//! controller**, not an entity for any shade, and the distinction is the whole
//! reason it is allowed to exist: a control on a shade no motor has heard would
//! transmit and move nothing, while a count claims only what this device
//! genuinely knows about its own table.
//!
//! # What used to be missing
//!
//! Nothing recorded which shades had been **announced**, so a shade removed
//! between two boots left a retained discovery config the next boot could not
//! name and therefore could not retire —
//! `somfy_mqtt::MqttConfig::retire_shade` was written, host-tested, and had no
//! caller. The persisted `announced` set closed that, and it lives with the
//! table in `somfy_config::Catalog` rather than here: this is a view for one
//! broker session, and the fact has to survive a reboot.

use core::fmt::Write as _;

use heapless::{String, Vec};
use somfy_domain::{Registry, RemoteIdentity, ShadeId, MAX_SHADES};
use somfy_mqtt::Pairing;

use crate::edits::MAX_NAME_LEN;

/// One shade as the broker session sees it.
struct Entry {
    id: ShadeId,
    name: String<MAX_NAME_LEN>,
    /// Whether this controller allocated its address, and so whether it owns a
    /// pairing button. See [`somfy_mqtt::Pairing`] for what a button on an
    /// imported shade would offer to do.
    pairing: Pairing,
}

/// One boot's view of which shades exist, what they are called, which of them
/// this controller can pair, and how many are still being set up.
pub struct Inventory {
    /// Kept as a slice because that is what `somfy-mqtt`'s plan builders take.
    ids: Vec<ShadeId, MAX_SHADES>,
    /// Parallel to `ids`. A separate vector rather than a field on one struct
    /// so `ids()` can hand out a `&[ShadeId]` without a copy.
    entries: Vec<Entry, MAX_SHADES>,
    /// Shades that exist and that nobody has reported working — the ones this
    /// snapshot deliberately leaves out of `ids`.
    ///
    /// **A boot figure, and only a boot figure.** It seeds the broker session's
    /// live counter and is never updated here: the runtime figure arrives on
    /// `crate::edits::ShadeEvent::AwaitingSetup`, because the registry belongs
    /// to the state task and this is a copy taken before that task existed.
    awaiting_setup: u8,
}

impl Inventory {
    /// Copy the shades an operator has reported working.
    ///
    /// **Not every shade in the registry**, and the omission is the whole
    /// point: a shade awaiting confirmation has an address no motor has heard,
    /// so an entity for it would appear in Home Assistant, accept commands and
    /// drive nothing. It is still in the registry, still commandable over the
    /// local API — which is how the setup flow gets it tested — and simply has
    /// no entities until somebody says it works.
    ///
    /// This is the boot half of the same gate `tasks::announce_shade` applies
    /// at runtime. Both are needed and neither is redundant: this one decides
    /// what a *fresh broker session* publishes, and that one decides what an
    /// *edit* publishes.
    pub fn snapshot(registry: &Registry) -> Inventory {
        let mut inventory = Inventory {
            ids: Vec::new(),
            entries: Vec::new(),
            awaiting_setup: crate::edits::awaiting_setup(registry),
        };
        for (id, shade) in registry.confirmed_shades() {
            // The pairing button is offered only for an address this controller
            // allocated. Pairing an imported shade would teach a motor an
            // address it already answers to — an action that does nothing, on
            // every shade of an imported estate.
            let pairing = if RemoteIdentity::is_allocated(shade.config.address) {
                Pairing::Offered
            } else {
                Pairing::Withheld
            };
            inventory.insert(id, &shade.config.name, pairing);
        }
        inventory
    }

    /// Add or replace one shade.
    ///
    /// Idempotent on the id: a second insert for an id already held replaces
    /// its name and pairing rather than growing a second row, so a repeated
    /// event cannot make `ids()` name the same shade twice.
    pub fn insert(&mut self, id: ShadeId, name: &str, pairing: Pairing) {
        let mut held: String<MAX_NAME_LEN> = String::new();
        // `push_str` is all-or-nothing in `heapless`, so a name that does not
        // fit leaves this **empty** rather than truncated — and an empty `name`
        // in a discovery payload is an entity Home Assistant labels with its
        // object id. The two capacities are equal today
        // (`crate::edits::MAX_NAME_LEN` and `somfy_domain::ShadeConfig::name`),
        // so this cannot happen; nothing ties them together, so the fallback is
        // here anyway and it names the shade rather than nothing.
        if held.push_str(name).is_err() {
            let _ = write!(&mut held, "Shade {}", id.0);
        }

        if let Some(index) = self.ids.iter().position(|other| *other == id) {
            if let Some(entry) = self.entries.get_mut(index) {
                entry.name = held;
                entry.pairing = pairing;
            }
            return;
        }

        // The two vectors stay the same length, and that is the invariant
        // `name` depends on — it looks a name up by the *index* of an id. It
        // holds because both have capacity `MAX_SHADES`, which is the
        // registry's own, and because the entry is only pushed once the id has
        // been: at that point the entries vector is one shorter and its push
        // cannot fail either.
        //
        // Failures are ignored rather than `expect`ed because a panic here
        // would take the radio off the air over a shade's name.
        if self.ids.push(id).is_ok() {
            let _ = self.entries.push(Entry {
                id,
                name: held,
                pairing,
            });
        }
    }

    /// Forget one shade. Returns whether it was there.
    ///
    /// **Removes it from the announcement, not from the broker.** Clearing what
    /// the broker holds is `MqttConfig::retire_shade`, and it has to happen
    /// before this — see `somfy_config::Catalog` for the ordering and what a
    /// power cut in the middle of it costs.
    pub fn remove(&mut self, id: ShadeId) -> bool {
        let Some(index) = self.ids.iter().position(|other| *other == id) else {
            return false;
        };
        // `remove` rather than `swap_remove`: the two vectors are kept parallel
        // by index, and the announcement walks them in registry order.
        self.ids.remove(index);
        self.entries.remove(index);
        true
    }

    /// The shades to announce, in registry order.
    pub fn ids(&self) -> &[ShadeId] {
        &self.ids
    }

    /// One shade's display name, as Home Assistant will show it.
    ///
    /// This is the only place a user's own spelling survives: `Salon /
    /// Porte-fenêtre` is unusable in a topic and perfectly good here, because
    /// `somfy-mqtt` builds every topic segment from the shade's id instead.
    pub fn name(&self, id: ShadeId) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.name.as_str())
    }

    /// Whether a shade owns a pairing button.
    ///
    /// `Withheld` for an id this inventory does not hold, which is the safe
    /// answer: an entity for a shade nothing knows about is one nothing can
    /// retire.
    pub fn pairing(&self, id: ShadeId) -> Pairing {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or(Pairing::Withheld, |entry| entry.pairing)
    }

    /// How many shades this session is announcing.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// How many shades existed at boot that nobody had reported working.
    ///
    /// The seed for the broker session's live figure, which the state task
    /// keeps current. See the field.
    pub fn awaiting_setup(&self) -> u8 {
        self.awaiting_setup
    }
}
