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
use crate::ShadeId;

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
