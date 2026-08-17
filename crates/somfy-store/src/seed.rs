//! Giving a newly provisioned address its first rolling code — once, and never
//! again.
//!
//! ## Why this is not just a `commit`
//!
//! A provisioned shade has to start somewhere. The store deliberately refuses
//! to invent that starting value ([`RollingCodeStore`]'s docs say so, and
//! [`crate::transmit`] reports [`crate::TransmitError::NoStoredCode`] rather
//! than filling the gap), so the number comes from configuration — and
//! **configuration is re-read at every boot**.
//!
//! That is the whole hazard. A boot path that answered a shade record by
//! committing the record's starting code would move the counter *backwards*
//! every time the board restarted. The motor stores the last code it accepted
//! and rejects anything at or below it as a replay, so the shade would stop
//! responding, look exactly like a broken transmitter, and only a physical
//! re-pairing at the motor would fix it. It is the same loss
//! [`RollingCodeStore`]'s "never invent a value" rule exists to prevent,
//! arriving through the one door that rule leaves open.
//!
//! [`seed_if_absent`] is that door, with the hinge welded: it reads first, and
//! it writes **only** when the read said there is nothing stored for this
//! address. Committing over an existing code is not a thing it declines to do —
//! it is a thing it cannot express, because the commit is inside the `None`
//! branch and there is no parameter that reaches the other one.
//!
//! ## Why a damaged region refuses instead of seeding
//!
//! [`RollingCodeStore::load`] answers `Ok(None)` for two very different facts:
//! "this address is new" and "the newest readable record does not mention this
//! address". The second one is ordinary — a shade added to a controller that
//! already had others — but it is also what a region with damaged slots looks
//! like when the damage is the record that *did* mention it.
//!
//! So the caller passes what its own survey saw. With damage present a missing
//! code is [`Seeded::Refused`]: no write, a fact to report, and a shade that
//! answers [`crate::TransmitError::NoStoredCode`] until a person looks. That
//! is recoverable; planting a low code over a lost high one is not.

use crate::store::RollingCodeStore;
use somfy_rts::RollingCode;

/// What the caller's own scan of the rolling-code region found.
///
/// Passed in rather than inferred, because [`RollingCodeStore`] exposes one
/// address at a time and this judgement is about the whole region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionState {
    /// Every slot was either a readable record or erased.
    Intact,
    /// Some slots hold bytes that are neither. Whether they were once this
    /// address's rolling code is unknowable.
    Damaged {
        /// How many slots. Reported, not acted on beyond the refusal.
        slots: usize,
    },
}

impl RegionState {
    /// From a survey's damaged-slot count, which is how a caller has it.
    pub const fn from_damaged(damaged: usize) -> RegionState {
        match damaged {
            0 => RegionState::Intact,
            slots => RegionState::Damaged { slots },
        }
    }
}

/// What [`seed_if_absent`] did, and therefore what the address's code is now.
///
/// `#[must_use]` because the interesting outcomes are the two that are not
/// "worked as expected": a [`Kept`](Seeded::Kept) code is the ordinary second
/// boot, and a [`Refused`](Seeded::Refused) is a shade that will not transmit
/// until somebody is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Seeded {
    /// The store already held a code for this address; the configured starting
    /// value was ignored. **Nothing was written.**
    Kept(RollingCode),
    /// The store held no code for this address, and the configured starting
    /// value was durably committed.
    Planted(RollingCode),
    /// The store held no code for this address and the region reports damage,
    /// so an empty read may be lost data rather than a new address. Nothing
    /// was written and the address still has no code.
    Refused {
        /// Damaged slots the caller reported. See [`RegionState`].
        damaged: usize,
    },
}

/// Give `address` its first rolling code, if and only if it has none.
///
/// The one rule, restated because it is the one that costs a re-pairing when it
/// is broken: **an existing stored code is never overwritten**. Call this with
/// the same configured value at every boot; after the first, it writes nothing
/// at all.
///
/// Errors are the store's own and mean nothing was seeded.
///
/// ```
/// use somfy_rts::RollingCode;
/// use somfy_store::{seed_if_absent, RegionState, RollingCodeStore, Seeded};
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
/// let mut store = Store(None);
/// let seed = RollingCode(5);
///
/// // First boot: the address is new, so the configured value is planted.
/// assert_eq!(seed_if_absent(&mut store, 0x00_C0DE, seed, RegionState::Intact), Ok(Seeded::Planted(seed)));
///
/// // The controller then transmits, which advances the counter.
/// store.0 = Some(RollingCode(9));
///
/// // Second boot, same configuration: the stored code wins, untouched.
/// assert_eq!(
///     seed_if_absent(&mut store, 0x00_C0DE, seed, RegionState::Intact),
///     Ok(Seeded::Kept(RollingCode(9))),
/// );
/// assert_eq!(store.0, Some(RollingCode(9)));
/// ```
pub fn seed_if_absent<S>(
    store: &mut S,
    address: u32,
    code: RollingCode,
    region: RegionState,
) -> Result<Seeded, S::Error>
where
    S: RollingCodeStore,
{
    // Read first. Everything below is inside the branch where this said there
    // is nothing stored, which is what makes "never overwrite" structural
    // rather than a rule to remember.
    if let Some(stored) = store.load(address)? {
        return Ok(Seeded::Kept(stored));
    }
    if let RegionState::Damaged { slots } = region {
        return Ok(Seeded::Refused { damaged: slots });
    }
    store.commit(address, code)?;
    Ok(Seeded::Planted(code))
}
