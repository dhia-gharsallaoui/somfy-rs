//! Where received pulses come from, and a host implementation that replays
//! them from memory.

use somfy_rts::Pulse;

/// A stream of measured OOK pulses, in the form a receiver actually sees them.
///
/// ## The contract
///
/// Pulses MUST be **merged edge-to-edge**: one entry per level change, never
/// one per Manchester half-symbol. That is not a preference — it is the only
/// representation both ends of this seam already speak. The RMT peripheral
/// records edges, and `somfy_rts::RxDecoder` accepts edge-derived streams, so a
/// source that honours it needs no adaptation layer in either direction. (The
/// unmerged form `somfy_rts::render_pulses` emits is a transmit-side detail;
/// `somfy_rts::merge_pulses` converts to this one.)
///
/// ## Why this exists
///
/// The receive path's recorded risk is that the RMT peripheral turns out to be
/// unable to capture RTS reliably, with interrupt timestamping as the
/// contingency. Consuming pulses through a trait makes that swap a change of
/// type parameter instead of a redesign under pressure on hardware. It also
/// buys the second thing the firmware badly needs: the radio task becomes
/// host-testable, because [`ReplayPulseSource`] can stand in for a radio in a
/// crate that a host compiler can actually build.
///
/// That is the whole brief. This is not a general-purpose stream abstraction
/// and should not grow into one.
pub trait PulseSource {
    /// Wait for the next pulse, or report that there will not be another.
    ///
    /// ## Why this is `async`
    ///
    /// A receiver spends almost all of its time waiting, and the wait is
    /// unbounded — a shade may go untouched for hours. The RMT driver's
    /// blocking receive busy-polls a status register with no deadline and no
    /// yield, so a synchronous version of this method would pin the executor
    /// for that entire silence, starving the state task and every queued
    /// transmit alongside it. Awaiting instead costs one `.await` at the call
    /// site and keeps the radio task's timing its own business, which is the
    /// reason the tasks are split at all.
    ///
    /// ## Why one pulse rather than a burst
    ///
    /// The RMT peripheral hands back a whole transaction at a time, so a
    /// burst-shaped method would look like the closer fit. But the decoder
    /// consumes exactly one pulse per call, and it needs no help at a
    /// transaction boundary: whatever state a truncated burst leaves it in, the
    /// next burst opens with the hardware-sync preamble, whose 2560 µs
    /// half-pulses match neither timing family the data phase accepts and so
    /// reset it. Concatenated bursts decode as cleanly as one continuous
    /// stream. Exposing the batching would put a boundary concept into the
    /// trait that no consumer has a use for, and would force every
    /// implementation to be batched.
    ///
    /// ## `None`
    ///
    /// `None` means this source is **finished**: it will never yield another
    /// pulse, and a caller should stop pumping it. It does not mean "nothing
    /// available just now" — that case is what awaiting is for. A radio source
    /// therefore returns `None` only if its hardware is gone for good; a
    /// single failed or corrupt capture is a dropped burst, not the end of the
    /// stream, and belongs to the implementation to retry.
    // No `Send` bound on the returned future: the executor that polls it is
    // single-threaded, so the bound would buy nothing, and it would rule out
    // implementations holding a peripheral handle across an await — which the
    // RMT one has to.
    #[allow(async_fn_in_trait)]
    async fn next_pulse(&mut self) -> Option<Pulse>;
}

/// A [`PulseSource`] that replays pulses already in memory.
///
/// This is how a captured transmission gets fed to code that expects a radio,
/// which is what makes the receive path testable on a host at all. It yields
/// the slice verbatim — no filtering, no merging, no synthesised timing — so
/// whatever a test hands it is exactly what the code under test sees, and a
/// real capture stays a real capture all the way to the decoder.
///
/// It never pends, so a test can drive it with a bare poll loop.
pub struct ReplayPulseSource<'a> {
    /// The pulses not yet handed out. The remaining slice *is* the cursor:
    /// there is no index to run past the end, and an exhausted source is the
    /// empty slice, which keeps yielding `None` for free.
    remaining: &'a [Pulse],
}

impl<'a> ReplayPulseSource<'a> {
    /// Replay `pulses` in order, once.
    pub fn new(pulses: &'a [Pulse]) -> Self {
        Self { remaining: pulses }
    }
}

impl PulseSource for ReplayPulseSource<'_> {
    async fn next_pulse(&mut self) -> Option<Pulse> {
        let (next, rest) = self.remaining.split_first()?;
        self.remaining = rest;
        Some(*next)
    }
}
