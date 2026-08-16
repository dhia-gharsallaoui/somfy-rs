//! The persistence seam: [`RollingCodeStore`].

use somfy_rts::RollingCode;

/// Persistent storage for per-address rolling codes.
///
/// The stored value is **next-to-send**, matching [`RollingCode`]'s own
/// semantics. Implementations own their storage medium and nothing else: no
/// frame building, no transmission, no ordering policy. Ordering is
/// [`crate::transmit`]'s job.
///
/// # Missing records are reported, never invented
///
/// [`load`](RollingCodeStore::load) returns `Ok(None)` for an address with no
/// stored record and `Err` for a storage failure. An implementation MUST NOT
/// substitute a starting value for either case — a store that answers `0` when
/// the flash read failed will silently replay codes the motor has already seen
/// and desync the pairing. Seeding a brand-new address is an explicit
/// [`commit`](RollingCodeStore::commit) by the caller, which is visible, rather
/// than a default the store applies on the caller's behalf.
///
/// This mirrors requirement R1 in
/// `docs/specs/2026-08-15-config-integrity-requirements.md`: configuration that
/// is missing, unreadable, or invalid must surface as a distinct state rather
/// than quietly become a compiled-in default.
///
/// # Commit is the durability point
///
/// [`commit`](RollingCodeStore::commit) must not return `Ok` until the value
/// would survive a power loss. Returning `Ok` from a write that is still
/// buffered defeats the entire ordering guarantee: the frame goes on the air
/// believing the code is safe when it is not.
pub trait RollingCodeStore {
    /// Storage failure. Deliberately opaque to this crate — it only ever
    /// propagates one, and a failed store operation always stops the
    /// transmission whatever it says.
    type Error;

    /// Read the next-to-send code for `address`.
    ///
    /// `Ok(None)` means "no record for this address", which is a different
    /// fact from `Err(_)` ("could not tell") and from `Ok(Some(RollingCode(0)))`
    /// ("the stored value is zero"). Keep the three distinct.
    fn load(&mut self, address: u32) -> Result<Option<RollingCode>, Self::Error>;

    /// Durably store `code` as the next-to-send value for `address`.
    ///
    /// Committing without transmitting is always safe — it skips a code
    /// forward, which a motor accepts. Transmitting without committing is the
    /// failure this crate exists to prevent.
    fn commit(&mut self, address: u32, code: RollingCode) -> Result<(), Self::Error>;
}
