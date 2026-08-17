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
//! # The one thing a snapshot cannot do
//!
//! It records the shades that **exist**, and nothing anywhere records which
//! were **announced**. So a shade removed from the table between two boots
//! leaves a retained discovery config the next boot has no way to learn of and
//! therefore cannot retire — `somfy_mqtt::MqttConfig::retire_shade` is written
//! and host-tested and still has no caller. Closing it needs the *announced*
//! set persisted, which is a record-format decision and belongs with Plan 6.
//! The same applies to a shade whose id moved: see `somfy_config`'s shade
//! record for why appending is safe and reordering is not.

use core::fmt::Write as _;

use heapless::{String, Vec};
use somfy_domain::{Registry, ShadeId, MAX_SHADES};
use somfy_mqtt::MAX_NAME_LEN;

/// One boot's view of which shades exist and what they are called.
pub struct Inventory {
    /// Kept as a slice because that is what `somfy-mqtt`'s plan builders take.
    ids: Vec<ShadeId, MAX_SHADES>,
    /// Parallel to `ids`. A separate vector rather than a tuple so `ids()` can
    /// hand out a `&[ShadeId]` without a copy.
    names: Vec<String<MAX_NAME_LEN>, MAX_SHADES>,
}

impl Inventory {
    /// Copy what the registry holds right now.
    pub fn snapshot(registry: &Registry) -> Inventory {
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for (id, shade) in registry.shades() {
            // The two vectors stay the same length, and that is the invariant
            // `name` depends on — it looks a name up by the *index* of an id.
            // It holds because both have capacity `MAX_SHADES`, which is the
            // registry's own, and because the name is only pushed once the id
            // has been: at that point the names vector is one shorter and its
            // push cannot fail either.
            //
            // Failures are ignored rather than `expect`ed because a panic here
            // would take the radio off the air over a shade's name.
            if ids.push(id).is_ok() {
                let mut name: String<MAX_NAME_LEN> = String::new();
                // `push_str` is all-or-nothing in `heapless`, so a name that
                // does not fit leaves this **empty** rather than truncated —
                // and an empty `name` in a discovery payload is an entity Home
                // Assistant labels with its object id. The two capacities are
                // equal today (`somfy_mqtt::MAX_NAME_LEN` and
                // `somfy_domain::ShadeConfig::name`), so this cannot happen;
                // nothing ties them together, so the fallback is here anyway
                // and it names the shade rather than nothing.
                if name.push_str(&shade.config.name).is_err() {
                    let _ = write!(&mut name, "Shade {}", id.0);
                }
                let _ = names.push(name);
            }
        }
        Inventory { ids, names }
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
        let index = self.ids.iter().position(|held| *held == id)?;
        self.names.get(index).map(String::as_str)
    }

    /// How many shades this boot found.
    pub fn len(&self) -> usize {
        self.ids.len()
    }
}
