//! The shade table a running controller works from, and what it takes to
//! change one.
//!
//! # What this owns that the registry does not
//!
//! [`somfy_domain::Registry`] holds the shades the controller can command. It
//! does not hold their rolling-code seeds, and it does not hold the one fact a
//! removal needs: **which shades this device has already published Home
//! Assistant entities for**. Both live in the persisted record, and this is
//! what keeps the record and the registry saying the same thing.
//!
//! Here rather than in the firmware because none of it is hardware. Every rule
//! below is a statement about ids, bits and timestamps, and every one of them
//! is checked on the host — which is the same reason [`ShadeRecord`] is here
//! and the flash I/O is not.
//!
//! # The ordering that makes a removal safe
//!
//! Removing a shade has to clear its retained discovery configs from the
//! broker, and clearing them needs its id — an id that, once the shade is gone,
//! nothing else in the system can produce. So the order is not a convention,
//! it is what the durable state expresses:
//!
//! 1. The shade leaves the registry and the table, and the record is written
//!    **with its announced bit still set**. From that moment the record names
//!    an orphan: a shade that was announced and does not exist.
//! 2. The broker session clears the entities.
//! 3. Only then is the bit cleared, and the record written again.
//!
//! A power cut anywhere in that sequence leaves the orphan named in flash, and
//! the next boot finds it and clears it. The failure this replaces — clear the
//! record first, then discover the orphans — is unrecoverable by the device at
//! all: the requirements behind it were written after deleting 49 retained
//! topics by hand.
//!
//! # Why the writes are debounced and rolling codes are not
//!
//! Opposite obligations. A rolling code must be durable **before** the frame
//! that uses it goes out, so its write is synchronous and sits immediately
//! before a transmission — a crash after transmitting with an unsaved code
//! desynchronises the motor. A shade table has no such deadline: nothing on the
//! air depends on it, and the cost of writing it is a flash sector erase with
//! interrupts disabled, during which the receiver hears nothing. Coalescing a
//! burst of edits into one write is therefore free correctness-wise and buys
//! back both the erase cycles and the deaf window.
//!
//! [`Catalog::due_at`] is the whole of the debounce policy, and it is pure.

use heapless::Vec;
use somfy_domain::{DomainError, Registry, ShadeConfig, ShadeId};
use somfy_rts::RollingCode;

use crate::shade::{
    Announced, LinkedRemote, ShadeRecord, StoredShade, MAX_LINKS, SHADE_TABLE_CAPACITY,
};

/// How long a change waits for its neighbours before it is written.
///
/// Two seconds is long enough that adding four shades from a UI is one write
/// rather than four, and short enough that a person who pulls the power after
/// pressing a button has almost certainly lost nothing. A policy figure chosen
/// for those two properties, not a measurement.
pub const DEBOUNCE_MS: u64 = 2_000;

/// The longest a change may wait, however many follow it.
///
/// Without a ceiling a steady trickle of edits — a UI that saves on every
/// keystroke — would postpone the write forever, and a power cut would then
/// lose all of them. Ten seconds bounds the loss.
pub const MAX_DEFER_MS: u64 = 10_000;

/// The shade table as the controller believes it, beside the registry that
/// commands it.
pub struct Catalog {
    /// Every shade's persisted rolling-code seed, indexed by registry id. A
    /// hole is a slot no shade occupies — the same shape the registry has, so
    /// the two cannot disagree about which id a row is.
    ///
    /// Only the seed, because it is the only field of a stored shade the
    /// registry does not hold. Everything else is read back off the registry
    /// when the record is built, so the two cannot drift.
    seeds: Vec<Option<RollingCode>, SHADE_TABLE_CAPACITY>,
    /// Which shades have entities on the broker. See this module's docs.
    announced: Announced,
    /// When the oldest unwritten change was made, and when the newest was.
    /// `None` when there is nothing to write.
    pending: Option<Pending>,
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog::new()
    }
}

/// The two timestamps a debounce needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pending {
    /// When the first change in this batch happened.
    first_ms: u64,
    /// When the most recent one did.
    last_ms: u64,
}

/// Why a change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// The domain refused it: a duplicate address, a name that does not fit, a
    /// full registry, an id past the end, an eighth remote on one shade.
    Domain(DomainError),
    /// The record's shared linked-remote pool is full. The per-shade bound is
    /// the domain's and is not what ran out — see [`MAX_LINKS`].
    LinksFull,
}

impl From<DomainError> for CatalogError {
    fn from(error: DomainError) -> Self {
        CatalogError::Domain(error)
    }
}

impl core::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CatalogError::Domain(error) => write!(f, "the shade was refused: {error:?}"),
            CatalogError::LinksFull => write!(
                f,
                "the record holds {MAX_LINKS} linked remotes across the whole table and they \
                 are all taken; unlink one first"
            ),
        }
    }
}

impl core::error::Error for CatalogError {}

/// What [`Catalog::record`] could not fit.
///
/// Returned beside the record rather than instead of it, because the record is
/// still the best thing to write: dropping a link is a wall remote that stops
/// correcting a shade, and dropping the *whole table* is every shade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dropped {
    /// Linked remotes that did not fit the pool.
    pub links: usize,
    /// Shades the registry holds that this table has no rolling-code seed for,
    /// and which were therefore written with a seed of zero.
    ///
    /// **Unreachable, and reported rather than asserted.** Every path that puts
    /// a shade in the registry also calls [`Catalog::place`] or
    /// [`Catalog::add`]. If one ever did not, the consequence is delayed and
    /// severe: the zero is ignored for as long as the rolling-code store holds
    /// a code for that address, and is planted the moment that region is lost —
    /// at which point the motor, which is at some high code, stops obeying and
    /// only a physical re-pairing fixes it.
    pub seeds: usize,
}

impl Catalog {
    /// An empty table. Shades are put in with [`Catalog::place`] as the boot
    /// loader walks the record.
    pub fn new() -> Catalog {
        Catalog {
            seeds: Vec::new(),
            announced: Announced::NONE,
            pending: None,
        }
    }

    /// Record the rolling-code seed of a shade the boot loader has just placed
    /// in the registry.
    ///
    /// **Marks nothing dirty**, because nothing changed: this is the table that
    /// was read, being remembered. A boot that marked itself dirty would write
    /// the record back on every single start, which is a flash sector erase per
    /// boot for no change at all.
    pub fn place(&mut self, id: ShadeId, seed: RollingCode) {
        let slot = id.0 as usize;
        if slot >= SHADE_TABLE_CAPACITY {
            return;
        }
        while self.seeds.len() <= slot {
            // Cannot fail: bounded by the capacity a line above.
            if self.seeds.push(None).is_err() {
                return;
            }
        }
        self.seeds[slot] = Some(seed);
    }

    /// Adopt the announced set the record carried. Marks nothing dirty, for the
    /// same reason [`Catalog::place`] does not.
    pub fn adopt_announced(&mut self, announced: Announced) {
        self.announced = announced;
    }

    /// Which shades have entities on the broker.
    pub fn announced(&self) -> Announced {
        self.announced
    }

    /// Ids that were announced and no longer exist: the orphans whose retained
    /// entities are still on the broker with nothing behind them.
    ///
    /// **This is the only thing in the system that can name them.** Once a
    /// shade is out of the registry and out of the table, its id is a number
    /// nothing else remembers.
    pub fn orphans<'a>(&'a self, registry: &'a Registry) -> impl Iterator<Item = ShadeId> + 'a {
        self.announced
            .ids()
            .filter(move |id| registry.shade(*id).is_none())
    }

    /// Note that `id`'s entities are now on the broker.
    pub fn mark_announced(&mut self, id: ShadeId, now_ms: u64) {
        if self.announced.contains(id) {
            return;
        }
        self.announced = self.announced.with(id);
        self.touch(now_ms);
    }

    /// Note that `id`'s entities have been cleared from the broker.
    ///
    /// Called **after** the tombstones have been acknowledged, never before:
    /// clearing the bit first would mean a power cut between the two lost the
    /// only record that the entities exist.
    pub fn mark_retired(&mut self, id: ShadeId, now_ms: u64) {
        if !self.announced.contains(id) {
            return;
        }
        self.announced = self.announced.without(id);
        self.touch(now_ms);
    }

    /// Add a shade the caller has just placed in the registry, with the
    /// rolling-code seed its first transmission will start from.
    pub fn add(&mut self, id: ShadeId, seed: RollingCode, now_ms: u64) {
        self.place(id, seed);
        self.touch(now_ms);
    }

    /// Remove a shade from the registry and the table.
    ///
    /// **The announced bit is deliberately left alone.** After this the record
    /// names an orphan, which is exactly what a removal has to leave behind
    /// until the broker has been told — see this module's docs for the ordering
    /// and what a power cut in the middle of it costs.
    pub fn remove(
        &mut self,
        registry: &mut Registry,
        id: ShadeId,
        now_ms: u64,
    ) -> Result<(), CatalogError> {
        registry.remove_shade(id)?;
        if let Some(slot) = self.seeds.get_mut(id.0 as usize) {
            *slot = None;
        }
        self.touch(now_ms);
        Ok(())
    }

    /// Link a wall remote to a shade.
    ///
    /// The registry's own rules decide — a sentinel address, a duplicate, an
    /// eighth remote — and are reached rather than restated. The record's
    /// shared pool is the one bound this adds, and it is checked **before** the
    /// registry is touched, so a refusal leaves nothing half-applied.
    pub fn link(
        &mut self,
        registry: &mut Registry,
        id: ShadeId,
        address: u32,
        now_ms: u64,
    ) -> Result<(), CatalogError> {
        if link_count(registry) >= MAX_LINKS {
            return Err(CatalogError::LinksFull);
        }
        registry
            .shade_mut(id)
            .ok_or(DomainError::NotFound)?
            .link_remote(address)?;
        self.touch(now_ms);
        Ok(())
    }

    /// Forget a wall remote.
    pub fn unlink(
        &mut self,
        registry: &mut Registry,
        id: ShadeId,
        address: u32,
        now_ms: u64,
    ) -> Result<(), CatalogError> {
        registry
            .shade_mut(id)
            .ok_or(DomainError::NotFound)?
            .unlink_remote(address)?;
        self.touch(now_ms);
        Ok(())
    }

    /// Replace one shade's configuration, and note that the table has to be
    /// written again.
    ///
    /// # Why the address and the id are taken from the shade rather than from
    /// `config`
    ///
    /// Because neither is editable, and a function that *accepts* them would be
    /// a function that can change them. A motor obeys an address; nothing in
    /// RTS can tell it the address moved, and nothing can ask it what it
    /// learned — so a shade whose address changed is a shade that stops
    /// responding, looks exactly like a dead motor, and is fixed only by
    /// walking to it. The id is the registry slot and the Home Assistant
    /// entity's identity, and moving it orphans every automation pointing at
    /// it.
    ///
    /// So the incoming `config` supplies the name, the kind, the tilt mode and
    /// the three travel times, and its `address` field is **overwritten** with
    /// the one the shade already has. That is deliberately silent rather than
    /// an error: the caller that matters builds `config` from the shade's own
    /// current configuration, so the field it holds is already right, and
    /// refusing a request over a field the caller never meant to set would
    /// reject correct edits.
    ///
    /// # What this does not do
    ///
    /// It does not touch the announced bit, and it must not: the entities still
    /// exist and still belong to this shade. A renamed shade needs its
    /// discovery config re-published, which is the caller's to arrange — the
    /// bit means "there are entities on the broker", not "they are current".
    pub fn reconfigure(
        &mut self,
        registry: &mut Registry,
        id: ShadeId,
        mut config: ShadeConfig,
        now_ms: u64,
    ) -> Result<(), CatalogError> {
        let shade = registry.shade_mut(id).ok_or(DomainError::NotFound)?;
        config.address = shade.config.address;
        shade.config = config;
        self.touch(now_ms);
        Ok(())
    }

    /// The record to write, built from the registry so that what is persisted
    /// is what the controller is actually running.
    ///
    /// The rolling-code seed comes from this table rather than from the
    /// registry, because the registry has never held one: a seed is read once,
    /// on the boot that first sees an address, and `somfy_store::seed_if_absent`
    /// ignores it ever after. Re-deriving it here would write a value the store
    /// has long since moved past — which is harmless while the store keeps its
    /// own copy, and catastrophic on the boot after that copy is lost.
    ///
    /// `seq` is left at zero: the ring stamps its own, which is the only number
    /// that orders records correctly.
    pub fn record(&self, registry: &Registry) -> (ShadeRecord, Dropped) {
        let mut shades: Vec<StoredShade, SHADE_TABLE_CAPACITY> = Vec::new();
        let mut links: Vec<LinkedRemote, MAX_LINKS> = Vec::new();
        let mut dropped = Dropped::default();

        for (id, shade) in registry.shades() {
            let seed = match self.seeds.get(id.0 as usize).and_then(|held| *held) {
                Some(seed) => seed,
                None => {
                    dropped.seeds += 1;
                    RollingCode(0)
                }
            };
            let row = shades.len();
            let entry = StoredShade {
                config: shade.config.clone(),
                initial_code: seed,
            };
            if shades.push(entry).is_err() {
                break;
            }
            for address in shade.linked() {
                if links
                    .push(LinkedRemote {
                        shade: ShadeId(row as u8),
                        address: *address,
                    })
                    .is_err()
                {
                    dropped.links += 1;
                }
            }
        }

        (
            ShadeRecord {
                seq: 0,
                announced: self.announced,
                shades,
                links,
            },
            dropped,
        )
    }

    /// Note that something changed at `now_ms`.
    fn touch(&mut self, now_ms: u64) {
        self.pending = Some(match self.pending {
            None => Pending {
                first_ms: now_ms,
                last_ms: now_ms,
            },
            Some(pending) => Pending {
                first_ms: pending.first_ms,
                last_ms: now_ms,
            },
        });
    }

    /// Whether anything is waiting to be written.
    pub fn is_dirty(&self) -> bool {
        self.pending.is_some()
    }

    /// When the pending changes should be written, or `None` if there are none.
    ///
    /// The whole of the debounce policy, and pure so that it is testable
    /// without a clock: [`DEBOUNCE_MS`] after the most recent change, but never
    /// more than [`MAX_DEFER_MS`] after the first — so a steady trickle of
    /// edits cannot postpone the write forever.
    pub fn due_at(&self) -> Option<u64> {
        let pending = self.pending?;
        Some((pending.last_ms + DEBOUNCE_MS).min(pending.first_ms + MAX_DEFER_MS))
    }

    /// Note that everything pending has been written durably.
    pub fn written(&mut self) {
        self.pending = None;
    }
}

/// Links every shade in `registry` holds, across the whole table.
fn link_count(registry: &Registry) -> usize {
    registry
        .shades()
        .map(|(_, shade)| shade.linked().len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use somfy_domain::{ShadeConfig, MAX_LINKED_REMOTES};

    fn registry_with(names: &[(&str, u32)]) -> (Registry, Catalog) {
        let mut registry = Registry::new();
        let mut catalog = Catalog::new();
        for (name, address) in names {
            let id = registry
                .add_shade(ShadeConfig::new(name, *address).unwrap())
                .unwrap();
            catalog.place(id, RollingCode(7));
        }
        (registry, catalog)
    }

    // -- the debounce -----------------------------------------------------

    #[test]
    fn a_clean_catalog_is_never_due() {
        assert_eq!(Catalog::new().due_at(), None);
    }

    #[test]
    fn one_change_is_due_a_debounce_later() {
        let mut catalog = Catalog::new();
        catalog.touch(1_000);
        assert_eq!(catalog.due_at(), Some(1_000 + DEBOUNCE_MS));
    }

    /// The point of a debounce: a burst is one write, at the end of the burst.
    #[test]
    fn a_burst_of_changes_is_one_write_after_the_last_of_them() {
        let mut catalog = Catalog::new();
        for at in [1_000, 1_100, 1_200, 1_300] {
            catalog.touch(at);
        }
        assert_eq!(catalog.due_at(), Some(1_300 + DEBOUNCE_MS));
    }

    /// And the ceiling: a trickle that never stops still gets written, because
    /// otherwise a power cut would take every change with it.
    #[test]
    fn a_steady_trickle_cannot_postpone_the_write_forever() {
        let mut catalog = Catalog::new();
        catalog.touch(0);
        let mut at = 0;
        while at < MAX_DEFER_MS * 2 {
            at += DEBOUNCE_MS / 2;
            catalog.touch(at);
            assert!(
                catalog.due_at().unwrap() <= MAX_DEFER_MS,
                "a change made at 0 must be written by {MAX_DEFER_MS}",
            );
        }
    }

    #[test]
    fn a_write_clears_the_debounce() {
        let mut catalog = Catalog::new();
        catalog.touch(1_000);
        assert!(catalog.is_dirty());
        catalog.written();
        assert!(!catalog.is_dirty());
        assert_eq!(catalog.due_at(), None);
    }

    /// Loading the table at boot must not schedule a write. A boot that marked
    /// itself dirty would erase a flash sector on every single start.
    #[test]
    fn loading_a_table_schedules_nothing() {
        let (_, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.adopt_announced(Announced::NONE.with(ShadeId(0)));
        assert!(!catalog.is_dirty());
        assert_eq!(catalog.due_at(), None);
    }

    // -- the ordering that makes a removal safe ---------------------------

    /// The orphan sweep: announced, and gone.
    #[test]
    fn a_removed_shade_stays_named_until_it_is_retired() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        let id = ShadeId(0);
        catalog.mark_announced(id, 0);
        catalog.written();
        assert_eq!(catalog.orphans(&registry).count(), 0);

        catalog.remove(&mut registry, id, 1_000).unwrap();
        assert_eq!(
            catalog.orphans(&registry).collect::<std::vec::Vec<_>>(),
            std::vec![id],
            "a removed shade must stay named until the broker has been told",
        );
        assert!(catalog.announced().contains(id));
        assert!(catalog.is_dirty());

        catalog.mark_retired(id, 2_000);
        assert_eq!(catalog.orphans(&registry).count(), 0);
        assert!(!catalog.announced().contains(id));
    }

    /// A shade that exists and was announced is not an orphan, however many
    /// times the sweep runs.
    #[test]
    fn a_live_announced_shade_is_never_swept() {
        let (registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.mark_announced(ShadeId(0), 0);
        for _ in 0..3 {
            assert_eq!(catalog.orphans(&registry).count(), 0);
        }
    }

    /// A shade that was never announced leaves nothing behind, so removing it
    /// does not schedule a retirement.
    #[test]
    fn removing_a_shade_that_was_never_announced_leaves_no_orphan() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.remove(&mut registry, ShadeId(0), 0).unwrap();
        assert_eq!(catalog.orphans(&registry).count(), 0);
    }

    /// Re-adding a shade at an id whose orphan has not been cleared yet must
    /// not sweep the new shade: the sweep is "announced and *not live*", and
    /// the new one is live.
    #[test]
    fn re_adding_a_shade_at_an_orphaned_id_does_not_sweep_it() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.mark_announced(ShadeId(0), 0);
        catalog.remove(&mut registry, ShadeId(0), 1).unwrap();
        assert_eq!(catalog.orphans(&registry).count(), 1);

        let id = registry
            .add_shade(ShadeConfig::new("Salon", 0x00_1002).unwrap())
            .unwrap();
        assert_eq!(id, ShadeId(0), "the freed slot is reused");
        catalog.add(id, RollingCode(1), 2);
        assert_eq!(
            catalog.orphans(&registry).count(),
            0,
            "the live shade at that id must not be retired out from under itself",
        );
    }

    // -- the record -------------------------------------------------------

    /// The record is built from the registry, so what is persisted is what the
    /// controller is running — and the seed comes from the table, because the
    /// registry has never held one.
    #[test]
    fn the_record_carries_the_registrys_shades_and_the_tables_seeds() {
        let mut registry = Registry::new();
        let mut catalog = Catalog::new();
        let id = registry
            .add_shade(ShadeConfig::new("Kitchen", 0x00_1001).unwrap())
            .unwrap();
        catalog.place(id, RollingCode(77));
        catalog.link(&mut registry, id, 0x00_2001, 0).unwrap();

        let (record, dropped) = catalog.record(&registry);
        assert_eq!(dropped, Dropped::default());
        assert_eq!(record.shades.len(), 1);
        assert_eq!(record.shades[0].initial_code, RollingCode(77));
        assert_eq!(record.shades[0].config.name.as_str(), "Kitchen");
        assert_eq!(
            record.links.as_slice(),
            &[LinkedRemote {
                shade: ShadeId(0),
                address: 0x00_2001,
            }],
        );
    }

    /// The seed fallback is reported rather than silent.
    ///
    /// It is unreachable through this type — every path that puts a shade in
    /// the registry also records its seed — so the only way to reach it is to
    /// go round the back, which is what this does. It matters because the
    /// consequence is delayed: a seed of zero is ignored for as long as the
    /// rolling-code store holds a code for that address, and planted the moment
    /// that region is lost, at which point the motor stops obeying.
    #[test]
    fn a_shade_with_no_recorded_seed_is_reported_rather_than_written_as_zero_in_silence() {
        let mut registry = Registry::new();
        let catalog = Catalog::new();
        registry
            .add_shade(ShadeConfig::new("Kitchen", 0x00_1001).unwrap())
            .unwrap();

        let (record, dropped) = catalog.record(&registry);
        assert_eq!(dropped.seeds, 1);
        assert_eq!(record.shades[0].initial_code, RollingCode(0));
    }

    /// A removal reaches the record, and the announced bit does not.
    #[test]
    fn a_removal_reaches_the_record_and_leaves_the_bit() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.mark_announced(ShadeId(0), 0);
        catalog.remove(&mut registry, ShadeId(0), 1).unwrap();

        let (record, _) = catalog.record(&registry);
        assert!(record.shades.is_empty());
        assert!(record.announced.contains(ShadeId(0)));
    }

    /// **The round trip that matters**: whatever this builds, the record must
    /// be able to read back — otherwise a runtime write produces a table the
    /// next boot refuses, and every shade vanishes.
    #[test]
    fn every_record_this_builds_decodes() {
        let mut registry = Registry::new();
        let mut catalog = Catalog::new();
        for index in 0..SHADE_TABLE_CAPACITY {
            let id = registry
                .add_shade(ShadeConfig::new("S", 0x00_1001 + index as u32).unwrap())
                .unwrap();
            catalog.add(id, RollingCode(index as u16), index as u64);
            catalog.mark_announced(id, index as u64);
        }
        // As many links as the pool holds, spread so no shade exceeds seven.
        let mut address = 0x00_5000u32;
        'fill: for index in 0..SHADE_TABLE_CAPACITY {
            for _ in 0..MAX_LINKED_REMOTES {
                if catalog
                    .link(&mut registry, ShadeId(index as u8), address, 0)
                    .is_err()
                {
                    break 'fill;
                }
                address += 1;
            }
        }

        let (record, dropped) = catalog.record(&registry);
        assert_eq!(dropped, Dropped::default());
        assert_eq!(record.links.len(), MAX_LINKS);
        assert_eq!(ShadeRecord::decode(&record.encode()), Ok(record));
    }

    /// The pool is shared, so it can run out while every shade is still under
    /// the domain's own bound. Refused at the call rather than dropped at the
    /// write: a dropped link is a wall remote that silently stops correcting a
    /// shade's position.
    #[test]
    fn a_full_link_pool_refuses_rather_than_dropping() {
        let mut registry = Registry::new();
        let mut catalog = Catalog::new();
        for index in 0..SHADE_TABLE_CAPACITY {
            let id = registry
                .add_shade(ShadeConfig::new("S", 0x00_1001 + index as u32).unwrap())
                .unwrap();
            catalog.place(id, RollingCode(1));
        }
        let mut address = 0x00_5000u32;
        let mut placed = 0;
        'fill: for index in 0..SHADE_TABLE_CAPACITY {
            for _ in 0..MAX_LINKED_REMOTES {
                match catalog.link(&mut registry, ShadeId(index as u8), address, 0) {
                    Ok(()) => {
                        placed += 1;
                        address += 1;
                    }
                    Err(error) => {
                        assert_eq!(error, CatalogError::LinksFull);
                        break 'fill;
                    }
                }
            }
        }
        assert_eq!(placed, MAX_LINKS);
        let (record, dropped) = catalog.record(&registry);
        assert_eq!(dropped, Dropped::default(), "nothing was silently dropped");
        assert_eq!(record.links.len(), MAX_LINKS);
    }

    /// A remote already linked is refused by the domain, and the refusal is not
    /// restated here — it is the same error the registry raises.
    #[test]
    fn a_duplicate_link_is_the_domains_refusal() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog
            .link(&mut registry, ShadeId(0), 0x00_2001, 0)
            .unwrap();
        assert_eq!(
            catalog.link(&mut registry, ShadeId(0), 0x00_2001, 0),
            Err(CatalogError::Domain(DomainError::DuplicateAddress)),
        );
    }

    /// And unlinking works, which is what a user does when a wall remote is
    /// taken off the wall.
    #[test]
    fn a_remote_can_be_unlinked_again() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog
            .link(&mut registry, ShadeId(0), 0x00_2001, 0)
            .unwrap();
        catalog
            .unlink(&mut registry, ShadeId(0), 0x00_2001, 1)
            .unwrap();
        let (record, _) = catalog.record(&registry);
        assert!(record.links.is_empty());
        assert_eq!(
            catalog.unlink(&mut registry, ShadeId(0), 0x00_2001, 2),
            Err(CatalogError::Domain(DomainError::NotFound)),
        );
    }

    /// An edit reaches the persisted record, and marks the table for writing —
    /// otherwise a corrected travel time would be live until the next reboot
    /// and then gone.
    #[test]
    fn reconfiguring_a_shade_reaches_the_record_and_schedules_a_write() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.written();
        assert!(!catalog.is_dirty());

        let mut edited = registry.shade(ShadeId(0)).unwrap().config.clone();
        edited.name = heapless::String::try_from("Cuisine").unwrap();
        edited.up_time_ms = 30_000;
        catalog
            .reconfigure(&mut registry, ShadeId(0), edited, 5_000)
            .unwrap();

        assert!(catalog.is_dirty());
        assert_eq!(catalog.due_at(), Some(5_000 + DEBOUNCE_MS));
        let (record, _) = catalog.record(&registry);
        assert_eq!(record.shades[0].config.name.as_str(), "Cuisine");
        assert_eq!(record.shades[0].config.up_time_ms, 30_000);
    }

    /// The address is the shade's, whatever the incoming configuration says.
    /// A motor obeys an address and cannot be told it moved, so this is the one
    /// field an edit must never carry through.
    #[test]
    fn reconfiguring_cannot_move_a_shades_address() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);

        let mut elsewhere = ShadeConfig::new("Kitchen", 0x00_9999).unwrap();
        elsewhere.up_time_ms = 30_000;
        catalog
            .reconfigure(&mut registry, ShadeId(0), elsewhere, 0)
            .unwrap();

        let shade = registry.shade(ShadeId(0)).unwrap();
        assert_eq!(shade.config.address, 0x00_1001);
        assert_eq!(shade.config.up_time_ms, 30_000, "the rest still applied");
    }

    /// A shade that is not there is the domain's refusal, and nothing is
    /// scheduled for writing over it.
    #[test]
    fn reconfiguring_a_missing_shade_is_refused_and_writes_nothing() {
        let (mut registry, mut catalog) = registry_with(&[("Kitchen", 0x00_1001)]);
        catalog.written();

        let config = ShadeConfig::new("Ghost", 0x00_2002).unwrap();
        assert_eq!(
            catalog.reconfigure(&mut registry, ShadeId(7), config, 0),
            Err(CatalogError::Domain(DomainError::NotFound)),
        );
        assert!(!catalog.is_dirty());
    }
}
