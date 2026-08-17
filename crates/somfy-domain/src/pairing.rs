//! The controller's own virtual-remote identity, and the addresses it hands
//! to shades.
//!
//! # Why a controller needs an identity of its own
//!
//! A Somfy RTS motor learns *remotes*, not controllers. Every frame carries a
//! 24-bit remote address and a rolling code, and the motor stores the last code
//! it accepted **per address** and rejects anything at or below it as a replay.
//!
//! So two controllers transmitting at one address is not a cosmetic clash. Each
//! keeps its own counter, neither knows what the other has sent, and the first
//! one to fall behind starts sending codes the motor has already seen. The
//! motor stops responding to it, and stays that way until somebody
//! re-synchronises it by hand at the shade.
//!
//! A controller that borrows another's identity is therefore not "working with
//! a caveat"; it is a controller that will stop working, at a time nobody
//! chooses. [`RemoteIdentity`] is the fix: this controller invents addresses no
//! other controller allocates, and pairing teaches the motor about them.
//!
//! # What the address is derived from, and what it deliberately is not
//!
//! The device-unique half of the MAC — its last three bytes. **Not** the first
//! three, which are the vendor OUI: those are a property of the chip maker and
//! identical on every board of one make, so a derivation reading them gives
//! every controller in the world the same addresses. That is not a hypothetical
//! — it was measured on two boards from one bench, which produced byte-for-byte
//! identical allocations. `docs/provenance.md` records the measurement.
//!
//! Every address also carries [`RemoteIdentity::SPACE_START`] in its top bits,
//! which makes the separation structural rather than probabilistic: a scheme
//! deriving from a 20-bit vendor prefix cannot reach this space at all, whatever
//! MAC it is fed, so an installation running both allocators cannot collide even
//! by accident.

use crate::registry::MAX_SHADES;
use crate::{DomainError, Registry, ShadeConfig, ShadeId};

/// Repeat frames that follow the first frame of a pairing burst.
///
/// **This number is part of the command's meaning, not a redundancy setting.**
/// A remote's PROG button pairs when tapped and *erases the remote from the
/// motor* when held, and a controller has no button — the length of the burst
/// is what stands in for how long the button was held. Seven repeats is a hold,
/// and a hold on a working shade unpairs it, which costs a walk to the shade
/// and a fresh pairing.
///
/// Two is a tap: the same burst length an ordinary command uses, which is what
/// a short press produces. It is carried as
/// [`Repeats::Exactly`](crate::Repeats::Exactly) rather than as the configured
/// default precisely so that a controller configured to transmit generously
/// cannot turn a pairing into an unpairing.
///
/// There is deliberately no unpairing command here. Nothing needs one, and the
/// cost of getting one wrong is paid at the shade rather than at the keyboard.
pub const PAIR_REPEATS: u8 = 2;

/// The bit every address this controller allocates carries.
///
/// Bit 23. Chosen because the RTS address is 24 bits with no field structure of
/// its own, so the top bit is free to carry a meaning this project gives it, and
/// because it puts the whole of this allocator's output above everything a
/// 20-bit-prefix derivation can produce. See the module docs.
const OWN_SPACE: u32 = 0x80_0000;

/// Bits of the address the device-unique part occupies, below [`OWN_SPACE`].
///
/// Twenty rather than the twenty-three that would fit, so that adding a shade
/// id and a collision probe to the base can never carry past the 24-bit
/// ceiling: the widest address this can produce is
/// `OWN_SPACE + DEVICE_BITS + u8::MAX + MAX_SHADES`, comfortably below
/// `0xFF_FFFF` — which [`ShadeConfig::new`](crate::ShadeConfig::new) refuses as
/// an "unset" sentinel.
const DEVICE_BITS: u32 = 0x0F_FFFF;

/// This controller's virtual-remote identity: the base address every shade's
/// own address is allocated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteIdentity {
    base: u32,
}

impl RemoteIdentity {
    /// The lowest address any controller using this scheme can allocate.
    ///
    /// Public because it is the non-collision guarantee, and a guarantee a
    /// caller cannot check is a guarantee it has to take on trust.
    pub const SPACE_START: u32 = OWN_SPACE;

    /// Derive this controller's identity from its factory MAC.
    ///
    /// Only `mac[3..6]` reach the address. The first three bytes are the vendor
    /// OUI and carry no device identity at all — see the module docs for what
    /// including them costs.
    ///
    /// All twenty-four device-unique bits contribute, folded into twenty. Plain
    /// truncation would drop the top nibble of `mac[3]` outright, so two boards
    /// differing only there would share every address they allocate — a case
    /// that costs one XOR to remove and is not hypothetical, since consecutive
    /// serials from one production run differ in exactly the low bytes.
    ///
    /// **The fold is still sixteen-to-one, and that is unavoidable rather than
    /// an oversight.** Twenty-four bits of identity cannot be injective into a
    /// twenty-four-bit address that must also carry a marker bit, a shade
    /// offset and a collision probe. So a shared base is a coincidence with
    /// probability around one in a million for two arbitrary boards — **not**
    /// evidence that the derivation lost the device-unique half. What
    /// distinguishes the two: under this derivation two boards differing
    /// anywhere in `mac[3..6]` *usually* differ; under an OUI derivation two
    /// boards of one make **always** collide. A single coincidence is not the
    /// defect; a hundred percent collision rate is.
    pub fn from_mac(mac: [u8; 6]) -> RemoteIdentity {
        let unique = ((mac[3] as u32) << 16) | ((mac[4] as u32) << 8) | mac[5] as u32;
        RemoteIdentity {
            base: OWN_SPACE | ((unique ^ (unique >> 20)) & DEVICE_BITS),
        }
    }

    /// Whether `address` is one this project's allocator produced.
    ///
    /// The whole test is the marker bit, and it is exact in one direction: no
    /// address below `0x80_0000` can have come from
    /// [`address_for`](RemoteIdentity::address_for), because
    /// every base carries the bit and nothing added to a base can clear it. So
    /// a `false` here means "this address came from somewhere else" — an
    /// imported table, a wall remote — with certainty.
    ///
    /// **It is a fact about the scheme, not about this board.** A tighter test
    /// — `address` inside `self.base ..= self.base + MAX_SHADES + u8::MAX` —
    /// would additionally say "*this* controller allocated it", and it is
    /// deliberately not what this is. A board whose table was restored onto
    /// different hardware, or whose base moved, would then report its own
    /// allocated addresses as foreign, and the caller that matters — the one
    /// deciding whether to offer a pairing button — would remove a button that
    /// works. The direction of the error is what settles it: over-reporting
    /// ownership offers a pairing action for a motor this controller may not
    /// have paired, which does nothing until somebody puts that motor into
    /// programming mode; under-reporting hides the action that is the only way
    /// to get the motor paired at all.
    ///
    /// A named function rather than a mask at each call site, because the mask
    /// is a claim about this allocator that a reader of the call site cannot
    /// check.
    ///
    /// ```
    /// use somfy_domain::RemoteIdentity;
    ///
    /// // Allocated here: the marker bit is set.
    /// assert!(RemoteIdentity::is_allocated(RemoteIdentity::SPACE_START));
    /// // Anything a 20-bit-prefix derivation can produce is not.
    /// assert!(!RemoteIdentity::is_allocated(0x0F_C0DE));
    /// ```
    pub const fn is_allocated(address: u32) -> bool {
        address & OWN_SPACE != 0
    }

    /// The address shade zero would take in an empty table.
    ///
    /// Exposed for diagnostics — a controller that prints this at boot is one an
    /// operator can tell apart from another controller without decoding a frame.
    pub fn base(&self) -> u32 {
        self.base
    }

    /// The address to give `shade`, skipping any the table already holds.
    ///
    /// One address per shade, offset by the shade's id, so a table provisioned
    /// in order gets consecutive addresses and a shade keeps its address when
    /// its neighbours change.
    ///
    /// `taken` answers whether an address is already spoken for. It exists
    /// because a real table is rarely all ours: a setup imported from another
    /// controller carries that controller's addresses, and allocating over one
    /// of them would produce exactly the two-controllers-one-identity failure
    /// this module exists to end. The probe walks upward on a clash, which is
    /// the same shape the reference allocator uses.
    ///
    /// `None` means every candidate was refused. That cannot happen for a
    /// predicate backed by a registry: a registry holds at most [`MAX_SHADES`]
    /// addresses and this probes `MAX_SHADES + 1` distinct candidates, so one is
    /// always free. It is reported rather than asserted because the predicate is
    /// the caller's, and a caller that supplies one answering `true` to
    /// everything deserves an answer rather than a hang.
    pub fn address_for(&self, shade: ShadeId, taken: impl Fn(u32) -> bool) -> Option<u32> {
        let first = self.base + shade.0 as u32;
        (0..=MAX_SHADES as u32)
            .map(|probe| first + probe)
            .find(|address| !taken(*address))
    }
}

/// What [`allocate_if_absent`] did, and therefore what the shade's address is
/// now.
///
/// `#[must_use]` for the same reason the rolling-code store's seeding outcome
/// is: the interesting case is [`Kept`](Allocated::Kept), which means the
/// caller asked to create a shade that already exists and got the existing one
/// back untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Allocated {
    /// The registry already held a shade at this id. **Nothing was allocated
    /// and nothing was changed** — not the address, not the name, not the
    /// travel times. This is the address that shade has always had.
    Kept(u32),
    /// The slot was free. A fresh address was allocated and the shade placed at
    /// it.
    Fresh(u32),
}

impl Allocated {
    /// The shade's address, whichever of the two happened.
    pub fn address(self) -> u32 {
        match self {
            Allocated::Kept(address) | Allocated::Fresh(address) => address,
        }
    }
}

/// Create the shade at `id` if that slot is free, allocating its radio address.
///
/// # Why this is not "add a shade, then set its address"
///
/// **A motor obeys an address, not a shade.** Pairing teaches one motor one
/// 24-bit address; nothing in the RTS protocol can tell the motor the address
/// changed, and nothing can ask it what it learned. So a controller that
/// reallocates an address a shade already had produces a shade that stops
/// responding, looks exactly like a dead motor or a dead radio, and is fixed
/// only by walking to the shade and pairing it again by hand.
///
/// That is the same class of loss the rolling-code store's `seed_if_absent`
/// exists to prevent, and this has the same shape for the same reason: it **reads
/// first**, and the allocation and the placement are both inside the branch
/// where the read found the slot empty. Reallocating is not a thing this
/// declines to do — there is no argument that reaches the other branch, and no
/// other function in this crate allocates at all.
///
/// # What it refuses
///
/// - [`DomainError::IdOutOfRange`] if `id` names no slot.
/// - [`DomainError::NameTooLong`], [`DomainError::InvalidAddress`] — the
///   ordinary [`ShadeConfig::new`] rules, so a shade created here is a shade
///   the registry and the persisted record both accept.
/// - [`DomainError::AddressUnavailable`] if every candidate address is already
///   in the table. A registry holds at most [`MAX_SHADES`] addresses and
///   [`RemoteIdentity::address_for`] probes `MAX_SHADES + 1` candidates, so
///   this cannot be reached through a registry — it is reported rather than
///   asserted because the alternative is a silent wrong answer.
///
/// ```
/// use somfy_domain::{allocate_if_absent, Allocated, Registry, RemoteIdentity, ShadeId};
///
/// let identity = RemoteIdentity::from_mac([0x00, 0x00, 0x00, 0x12, 0x34, 0x56]);
/// let mut registry = Registry::new();
///
/// let first = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Kitchen")?;
/// assert!(matches!(first, Allocated::Fresh(_)));
/// assert!(RemoteIdentity::is_allocated(first.address()));
///
/// // Asked again — with a different name, even — the slot is occupied, so the
/// // address is handed back unchanged and nothing is written.
/// let again = allocate_if_absent(&mut registry, &identity, ShadeId(0), "Renamed")?;
/// assert_eq!(again, Allocated::Kept(first.address()));
/// assert_eq!(registry.shade(ShadeId(0)).map(|s| s.config.name.as_str()), Some("Kitchen"));
/// # Ok::<(), somfy_domain::DomainError>(())
/// ```
pub fn allocate_if_absent(
    registry: &mut Registry,
    identity: &RemoteIdentity,
    id: ShadeId,
    name: &str,
) -> Result<Allocated, DomainError> {
    allocate_with(registry, identity, id, |address| {
        ShadeConfig::new(name, address)
    })
    .map_err(DomainError::from)
}

/// [`allocate_if_absent`], for a caller that builds the whole configuration.
///
/// # Why this exists rather than "add the shade, then set its fields"
///
/// A shade created with a name and then configured is a shade that briefly
/// held the factory travel times, and a device that is asked for its shades in
/// that instant reports them — as **uncalibrated**, which is what
/// `somfy_api::CalibrationSource` is for and is exactly the wrong answer about
/// a shade somebody has just typed measured values into. Building the
/// configuration before the shade exists removes the instant.
///
/// It also removes a whole class of half-applied failure. `describe` is
/// fallible and runs **before** anything is written, so a configuration the
/// caller's own rules refuse — a name over the limit, a travel time of zero,
/// a `kind` byte the domain does not model — leaves the registry untouched and
/// leaves the address unallocated. Had the shade been created first, refusing
/// the configuration afterwards would mean either keeping a shade nobody asked
/// for or removing one and burning its address, and an address that has been
/// burned is one the next shade does not get.
///
/// # What `describe` is handed and what it must not do
///
/// The address, and nothing else. It is chosen here — by probing past
/// everything the table already holds — because that is the decision no caller
/// is entitled to make: a motor obeys an address, and a controller that lets a
/// caller pick one produces the two-controllers-one-identity failure this
/// module exists to end. `describe` may reject the address (that is the whole
/// point of it returning a `Result`), but it cannot choose a different one.
///
/// The error type is the caller's, so a validation layer with its own
/// vocabulary — `somfy_api::ApiErrorCode`, which the web UI translates — can
/// report its own refusals rather than having them flattened into
/// [`DomainError`]. [`AllocateError`] keeps the two apart.
///
/// ```
/// use somfy_domain::{allocate_with, Allocated, Registry, RemoteIdentity, ShadeConfig, ShadeId};
///
/// let identity = RemoteIdentity::from_mac([0x00, 0x00, 0x00, 0x12, 0x34, 0x56]);
/// let mut registry = Registry::new();
///
/// let made = allocate_with(&mut registry, &identity, ShadeId(0), |address| {
///     let mut config = ShadeConfig::new("Kitchen", address)?;
///     config.up_time_ms = 30_000;
///     Ok::<_, somfy_domain::DomainError>(config)
/// })?;
/// assert!(matches!(made, Allocated::Fresh(_)));
/// assert_eq!(registry.shade(ShadeId(0)).map(|s| s.config.up_time_ms), Some(30_000));
/// # Ok::<(), somfy_domain::AllocateError<somfy_domain::DomainError>>(())
/// ```
pub fn allocate_with<E>(
    registry: &mut Registry,
    identity: &RemoteIdentity,
    id: ShadeId,
    describe: impl FnOnce(u32) -> Result<ShadeConfig, E>,
) -> Result<Allocated, AllocateError<E>> {
    if id.0 as usize >= MAX_SHADES {
        return Err(AllocateError::Domain(DomainError::IdOutOfRange));
    }
    // Read first. Everything below is inside the branch where this said the
    // slot is empty, which is what makes "an address never moves" structural
    // rather than a rule to remember.
    if let Some(shade) = registry.shade(id) {
        return Ok(Allocated::Kept(shade.config.address));
    }

    // `is_linked`, not an address comparison: a shade's linked wall remotes are
    // as taken as its own address. Allocating one of them would leave the
    // controller unable to tell its own frames from the remote's — every burst
    // it sent would come back as an overheard press and move the position
    // estimate a second time.
    let address = identity
        .address_for(id, |candidate| {
            registry
                .shades()
                .any(|(_, shade)| shade.is_linked(candidate))
        })
        .ok_or(AllocateError::Domain(DomainError::AddressUnavailable))?;

    // Before the registry is touched, so a refusal costs nothing.
    let config = describe(address).map_err(AllocateError::Description)?;
    registry
        .add_shade_with_id(id, config)
        .map_err(AllocateError::Domain)?;
    Ok(Allocated::Fresh(address))
}

/// Why [`allocate_with`] refused.
///
/// Two variants rather than one because the two failures belong to different
/// people. A [`Domain`](AllocateError::Domain) refusal is about the table — the
/// id is out of range, every candidate address is taken, the registry is full —
/// and nothing the caller sent would have changed it. A
/// [`Description`](AllocateError::Description) refusal is the caller's own
/// validation speaking, in the caller's own vocabulary, and is the one a person
/// can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocateError<E> {
    /// The registry or the allocator refused.
    Domain(DomainError),
    /// The caller's `describe` refused the address it was offered.
    Description(E),
}

impl From<AllocateError<DomainError>> for DomainError {
    /// Flatten, for a caller whose own validation already speaks
    /// [`DomainError`] — which is what [`allocate_if_absent`] is.
    fn from(error: AllocateError<DomainError>) -> DomainError {
        match error {
            AllocateError::Domain(error) | AllocateError::Description(error) => error,
        }
    }
}
