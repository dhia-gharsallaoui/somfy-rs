//! Bounded exponential backoff, as a value rather than as a variable somebody
//! remembers to reset.
//!
//! It lives here, host-tested, for the same reason the loops above do: this is
//! arithmetic, it has three edges that are easy to get wrong, and the firmware
//! crate cannot be compiled for a host at all. The three edges are a retry that
//! never grows (a tight loop against an access point that is refusing), one
//! that grows without limit (a router rebooted at 2 a.m. and rejoined at
//! lunchtime), and one that forgets to reset after a success (the second
//! outage is punished for the first).

/// A delay that doubles on each failure and is reset by a success.
///
/// Construct it with the range it may take, then alternate [`Backoff::fail`]
/// and [`Backoff::succeed`]. The current delay is always within the range, and
/// the first one is always the minimum — a first attempt is not a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    min_ms: u32,
    max_ms: u32,
    current_ms: u32,
}

impl Backoff {
    /// A backoff that starts at `min_ms` and never exceeds `max_ms`.
    ///
    /// A `max_ms` below `min_ms` is raised to it rather than refused: this is
    /// called from a path whose whole purpose is to keep running, and there is
    /// no sensible interpretation of an empty range other than "wait the
    /// minimum".
    pub const fn new(min_ms: u32, max_ms: u32) -> Self {
        let max_ms = if max_ms < min_ms { min_ms } else { max_ms };
        Self {
            min_ms,
            max_ms,
            current_ms: min_ms,
        }
    }

    /// How long to wait before the next attempt.
    pub const fn delay_ms(&self) -> u32 {
        self.current_ms
    }

    /// Record a failed attempt and double the delay, up to the ceiling.
    ///
    /// Returns the delay to wait *now*, which is the one that was in force
    /// before the doubling: the first failure waits the minimum, not twice it.
    pub const fn fail(&mut self) -> u32 {
        let waiting = self.current_ms;
        // `saturating_mul` matters: `max_ms` bounds the result but not the
        // intermediate, and a minimum near `u32::MAX` would otherwise wrap to
        // a delay of nearly zero — turning the ceiling into a tight loop,
        // which is the exact failure this type exists to prevent.
        self.current_ms = self.current_ms.saturating_mul(2);
        if self.current_ms > self.max_ms {
            self.current_ms = self.max_ms;
        }
        waiting
    }

    /// Record a success, so the next failure starts from the minimum again.
    pub const fn succeed(&mut self) {
        self.current_ms = self.min_ms;
    }

    /// Record a success **only if it lasted**, and report whether it counted.
    ///
    /// ## The failure this exists for
    ///
    /// "Succeeded" and "worked" are not the same event, and treating them as
    /// one defeats the ceiling in exactly the case that needs it most. A Wi-Fi
    /// station associates, and *then* the access point drops it — a captive
    /// portal, a MAC policy check, band steering, a network with no DHCP
    /// server. Each attempt reports success, so an unconditional
    /// [`Backoff::succeed`] resets the delay every time, and the retry settles
    /// into a permanent cycle at the minimum that never escalates. Bounded, in
    /// the sense that nothing overflows; useless, in the sense that the device
    /// hammers a network that is refusing it, forever.
    ///
    /// So a success counts only once the thing it produced has lasted
    /// `stable_ms`. Below that it is treated as a failure that happened to
    /// begin well, and the delay keeps growing towards the ceiling.
    pub const fn succeed_after(&mut self, lasted_ms: u32, stable_ms: u32) -> bool {
        if lasted_ms >= stable_ms {
            self.succeed();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_wait_is_the_minimum_and_it_doubles_from_there() {
        let mut backoff = Backoff::new(1_000, 60_000);
        assert_eq!(backoff.fail(), 1_000);
        assert_eq!(backoff.fail(), 2_000);
        assert_eq!(backoff.fail(), 4_000);
        assert_eq!(backoff.fail(), 8_000);
    }

    /// The ceiling is the whole point: without it a device that lost its
    /// network overnight would still be waiting hours later, long after the
    /// network came back.
    #[test]
    fn the_delay_stops_at_the_ceiling_and_stays_there() {
        let mut backoff = Backoff::new(1_000, 5_000);
        for _ in 0..64 {
            backoff.fail();
        }
        assert_eq!(backoff.delay_ms(), 5_000);
        assert_eq!(backoff.fail(), 5_000);
    }

    /// Forgetting this is the subtle one: a device that reconnected an hour
    /// ago would answer its next brief outage with the delay it had reached
    /// during the last one.
    #[test]
    fn a_success_returns_the_delay_to_the_minimum() {
        let mut backoff = Backoff::new(1_000, 60_000);
        for _ in 0..8 {
            backoff.fail();
        }
        assert_eq!(backoff.delay_ms(), 60_000);
        backoff.succeed();
        assert_eq!(backoff.delay_ms(), 1_000);
        assert_eq!(backoff.fail(), 1_000);
    }

    /// Doubling near the top of the range must not wrap. A wrapped delay is a
    /// delay of almost nothing, which is a tight retry loop wearing a ceiling.
    #[test]
    fn doubling_near_the_top_of_the_range_saturates_rather_than_wrapping() {
        let mut backoff = Backoff::new(u32::MAX - 1, u32::MAX);
        assert_eq!(backoff.fail(), u32::MAX - 1);
        assert_eq!(backoff.delay_ms(), u32::MAX);
    }

    #[test]
    fn an_inverted_range_collapses_to_the_minimum() {
        let mut backoff = Backoff::new(5_000, 1_000);
        assert_eq!(backoff.fail(), 5_000);
        assert_eq!(backoff.delay_ms(), 5_000);
    }

    /// The whole point of `succeed_after`: an association that is dropped
    /// again immediately must **not** reset the delay, or a network that
    /// accepts and then refuses is retried at the minimum forever.
    #[test]
    fn a_success_that_did_not_last_does_not_reset_the_delay() {
        let mut backoff = Backoff::new(1_000, 60_000);
        let mut waited = 0;
        for _ in 0..8 {
            // Associated, then dropped after two seconds, over and over.
            assert!(!backoff.succeed_after(2_000, 10_000));
            waited = backoff.fail();
        }
        assert_eq!(waited, 60_000, "the ceiling was never reached");
    }

    #[test]
    fn a_success_that_lasted_resets_the_delay() {
        let mut backoff = Backoff::new(1_000, 60_000);
        for _ in 0..8 {
            backoff.fail();
        }
        assert_eq!(backoff.delay_ms(), 60_000);
        assert!(
            backoff.succeed_after(10_000, 10_000),
            "exactly at the bound"
        );
        assert_eq!(backoff.delay_ms(), 1_000);
    }

    /// A `stable_ms` of zero makes every success count, which is the
    /// unconditional behaviour — worth pinning so the two cannot drift.
    #[test]
    fn a_stability_bound_of_zero_accepts_any_success() {
        let mut backoff = Backoff::new(1_000, 60_000);
        backoff.fail();
        backoff.fail();
        assert!(backoff.succeed_after(0, 0));
        assert_eq!(backoff.delay_ms(), 1_000);
    }

    /// Every delay handed out must lie inside the declared range, whatever
    /// order successes and failures arrive in.
    #[test]
    fn every_delay_stays_inside_the_range() {
        let mut backoff = Backoff::new(250, 30_000);
        for step in 0..200u32 {
            let waited = if step.is_multiple_of(7) {
                backoff.succeed();
                backoff.delay_ms()
            } else {
                backoff.fail()
            };
            assert!(
                (250..=30_000).contains(&waited),
                "delay {waited} left the range at step {step}",
            );
        }
    }
}
