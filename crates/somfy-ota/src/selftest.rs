//! Which checks may fail an update, and which may only be reported.
//!
//! # The rule, and it is the whole of this module
//!
//! **A leg may only fail an update if its failure is this image's fault.**
//! Everything else is reported and nothing more.
//!
//! That sounds like a formality until it is applied to the network, which is
//! where design spec §7.5 points a self-test first. Consider a board that takes
//! an update at the moment its access point reboots. It comes up, cannot
//! associate, and retries with backoff — exactly as it is supposed to, and
//! exactly as this estate's board did on 2026-08-17 when its access point
//! disappeared for a stretch. If "did not associate inside the window" rolled
//! the update back, that board would have discarded a perfectly good release
//! because of somebody else's router, and it would do it again on the next
//! attempt, and the operator would have no way to tell the two causes apart —
//! because **there is no way to tell them apart**. A one-way radio controller
//! cannot distinguish "my Wi-Fi code is broken" from "there is no Wi-Fi".
//!
//! So association is [`SelfTest::associated`]: recorded, printed, and not a
//! trigger. What *is* a trigger is the part of the network that is local —
//! whether the driver accepted the configuration and the stack started at all
//! ([`Leg::Network`]). That fails when this image's network code is wrong and
//! succeeds when the air is merely empty, which is the distinction that was
//! missing.
//!
//! # What the window is actually for
//!
//! Not "how long to wait for the network" — nothing waits on the network any
//! more. It is a **soak**: time for the image to fall over on its own.
//!
//! Everything in this firmware that is likely to kill a bad release happens
//! early and in a known order — the radio comes up in the first second, the
//! stores are read before that, the station associates, and then the broker
//! session publishes a burst of retained discovery configs which is *the heap's
//! high-water mark for the entire boot*. An image that is going to exhaust the
//! heap does it there. The soak has to outlast that, which is why association
//! does not shorten it: confirming the moment the link came up would confirm
//! about ten seconds before the most dangerous thing this firmware does.
//!
//! [`WINDOW_MS`] carries the arithmetic.

/// A check that can fail an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// The CC1101's control path answered.
    ///
    /// **This is a real check and a weak one, and the weakness is the point.**
    /// It proves that a write to a configuration register reads back — the SPI
    /// bus, the chip select, the supply and the crystal. It proves **nothing**
    /// about GDO0 or GDO2, which are the two lines every frame actually travels
    /// on, and nothing at all about whether the antenna radiates: an
    /// initialised CC1101 with both data lines disconnected passes this and
    /// cannot send or receive a single frame. `somfy-cc1101`'s own
    /// documentation says the same thing about `Cc1101::init`.
    ///
    /// It is here because it is the strongest radio check available without
    /// transmitting, and an update that marks itself valid must not put a frame
    /// on the band to do it. What would strengthen it — a loopback through the
    /// RMT receiver, which this firmware is capable of — would key the
    /// transmitter on every boot after an update, and that is a worse trade
    /// than a check with a stated limit.
    Radio,
    /// The flash regions mounted and their newest records read back.
    ///
    /// A partition table the new image disagrees with, or a record format it
    /// cannot decode, lands here — and both are exactly the kind of thing a
    /// release changes.
    Stores,
    /// The Wi-Fi driver accepted the configuration and the network stack
    /// started.
    ///
    /// Deliberately **not** association. See the module docs.
    Network,
}

/// What one leg has to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegState {
    /// It has not been decided yet.
    Pending,
    /// It answered.
    Passed,
    /// It did not, and this image is why.
    Failed,
    /// It does not apply to this board. A board with no Wi-Fi credentials
    /// provisioned has no network to bring up, and refusing an update over that
    /// would make a radio-only controller unable to accept one.
    Skipped,
}

impl LegState {
    /// Whether this leg is settled one way or the other.
    const fn settled(self) -> bool {
        matches!(
            self,
            LegState::Passed | LegState::Failed | LegState::Skipped
        )
    }
}

/// How long the soak runs, in milliseconds.
///
/// **Ninety seconds, and it is two figures added together — one taken from this
/// repository and one a policy figure that says so.**
///
/// - **45,000 ms** is `somfy_config::trial::ASSOCIATE_DEADLINE_MS`, which this
///   project already uses as "long enough to associate on a network that is
///   working". It is not re-derived here, and it is not depended on as a
///   constant either: this crate models a decision and that one models a
///   credential trial, and tying them would mean a change to one silently
///   moving the other.
/// - **45,000 ms more** is a policy figure for everything that happens after
///   the link: the DHCP lease, the broker's TCP connect and CONNACK, and the
///   burst of retained discovery configs that is the heap's peak. Nobody has
///   measured the interval from `net: address` to `heap: session announced`, so
///   this is a reserve rather than a measurement, chosen equal to the half that
///   *is* measured rather than tuned to look precise.
///
/// The direction of error is one-sided and that is what makes a rough figure
/// acceptable: too long costs a good update ninety seconds before it says so on
/// the console, and too short costs a bad one its rollback.
pub const WINDOW_MS: u64 = 90_000;

/// The legs, as they stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfTest {
    /// See [`Leg::Radio`].
    pub radio: LegState,
    /// See [`Leg::Stores`].
    pub stores: LegState,
    /// See [`Leg::Network`].
    pub network: LegState,
    /// Whether the station has reached a configured address at any point during
    /// the soak.
    ///
    /// **Reported, never a trigger.** See the module docs.
    pub associated: bool,
}

impl SelfTest {
    /// Nothing decided yet.
    pub const fn new() -> SelfTest {
        SelfTest {
            radio: LegState::Pending,
            stores: LegState::Pending,
            network: LegState::Pending,
            associated: false,
        }
    }
}

impl Default for SelfTest {
    fn default() -> SelfTest {
        SelfTest::new()
    }
}

/// What the soak has concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfTestOutcome {
    /// Still soaking.
    Waiting,
    /// Every leg that applies passed and the window has run out.
    Pass {
        /// Whether the network came up while it did. Carried so the console
        /// line can say which kind of pass this was, since a board confirmed
        /// without a network is worth knowing about even though it is not worth
        /// refusing.
        associated: bool,
    },
    /// A leg failed. Roll back.
    Fail(Leg),
}

impl SelfTest {
    /// Decide, `elapsed_ms` into the soak.
    ///
    /// A failure is returned as soon as it is known — there is nothing to be
    /// gained by soaking an image whose radio did not answer — and a pass only
    /// once the whole window has run *and* every leg has actually reported. A
    /// leg still [`LegState::Pending`] at the end of the window is a leg that
    /// never ran, which is a different thing from one that failed, and it keeps
    /// the outcome at [`SelfTestOutcome::Waiting`] rather than being read as
    /// either.
    pub const fn poll(&self, elapsed_ms: u64) -> SelfTestOutcome {
        if matches!(self.radio, LegState::Failed) {
            return SelfTestOutcome::Fail(Leg::Radio);
        }
        if matches!(self.stores, LegState::Failed) {
            return SelfTestOutcome::Fail(Leg::Stores);
        }
        if matches!(self.network, LegState::Failed) {
            return SelfTestOutcome::Fail(Leg::Network);
        }
        if elapsed_ms < WINDOW_MS {
            return SelfTestOutcome::Waiting;
        }
        if self.radio.settled() && self.stores.settled() && self.network.settled() {
            SelfTestOutcome::Pass {
                associated: self.associated,
            }
        } else {
            SelfTestOutcome::Waiting
        }
    }
}
