//! Replaying captured pulse trains through [`ReplayPulseSource`] into
//! `somfy_rts::RxDecoder`.
//!
//! The three `up`/`down`/`my` fixtures carry a physical Somfy wall remote's
//! **measured timing** — the wake-up, the gap, the sync structure and the
//! half-symbol jitter — under a payload this project substituted, because the
//! original payload was that remote's own address. See
//! `../../somfy-rts/tests/fixtures/README.md` for what is measured and what is
//! not. The timing is the half that matters here: these tests measure the trait
//! against durations hardware actually produced rather than against a stream
//! written to match the implementation, and a source that only ever replayed
//! this crate's own nominal pulses could not catch a mismatch between the two.
//!
//! The pump helper below is deliberately generic over [`PulseSource`]: the
//! radio task will be written the same way, so exercising the bound here is
//! what proves the trait is usable as one.

// Shared with `somfy-rts`' own golden tests so both crates reconstruct levels
// and drop glitches by identical rules — see that module's header.
#[path = "../../somfy-rts/tests/support/mod.rs"]
mod support;

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use somfy_rmt::{PulseSource, ReplayPulseSource};
use somfy_rts::{decode56, Command, Frame, Pulse, RxDecoder};

/// The captures live in `somfy-rts`, next to the decoder they were taken to
/// pin. They are read-only here — and must stay so: they are derived files that
/// cannot be regenerated, because the originals they came from were destroyed
/// on purpose.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../somfy-rts/tests/fixtures");

/// Drive a future to completion on the host.
///
/// [`PulseSource::next_pulse`] is `async` because the RMT implementation has to
/// hand the executor back while the air is silent. [`ReplayPulseSource`] never
/// pends, so a bare poll loop with a no-op waker completes it on the first poll
/// and keeps these tests free of an executor dependency.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

/// Drain a source into the decoder, returning every frame it completed.
///
/// Generic over the trait rather than over `ReplayPulseSource` on purpose: this
/// is the shape the radio task takes, so if the bound does not work here it
/// does not work there either.
fn pump<S: PulseSource>(source: &mut S) -> std::vec::Vec<Frame> {
    let mut rx = RxDecoder::new();
    let mut frames = std::vec::Vec::new();
    while let Some(pulse) = block_on(source.next_pulse()) {
        if let Some(raw) = rx.push(pulse) {
            assert_eq!(raw.bit_length, 56, "captures are 56-bit transmissions");
            let bytes = raw.bytes.as_slice().try_into().expect("56-bit payload");
            frames.push(decode56(bytes).expect("checksum must verify"));
        }
    }
    frames
}

fn replay(name: &str) -> std::vec::Vec<Frame> {
    let pulses = support::load_fixture(FIXTURES, name);
    pump(&mut ReplayPulseSource::new(&pulses))
}

/// Each capture is one button press recorded as a single transmission, so it
/// carries exactly one decodable frame.
fn replay_one(name: &str) -> Frame {
    let frames = replay(name);
    assert_eq!(frames.len(), 1, "{name} should carry exactly one frame");
    frames[0]
}

#[test]
fn captured_up_replays_as_up() {
    assert_eq!(
        replay_one("anonymised_up_56bit_1.pulses").command,
        Command::Up
    );
}

#[test]
fn captured_down_replays_as_down() {
    assert_eq!(
        replay_one("anonymised_down_56bit_1.pulses").command,
        Command::Down
    );
}

#[test]
fn captured_my_replays_as_my() {
    assert_eq!(
        replay_one("anonymised_my_56bit_1.pulses").command,
        Command::My
    );
}

/// All three fixtures were written with one substituted address, so they must
/// decode to one address here. Asserting they agree rather than pinning the
/// value: the point is to catch a replay path that corrupted or reordered
/// pulses, which would move the address as readily as it moved the command, and
/// the literal is pinned once already, in `somfy-rts`' own golden tests.
///
/// This test predates the anonymisation and was originally worded to keep a
/// *real* remote's address out of this source file. That reason is gone —
/// there is no real address left anywhere in the repository — but the
/// assertion is unchanged and still earns its place, so the note stays rather
/// than the history being tidied away.
#[test]
fn the_three_captures_replay_to_a_single_address() {
    let up = replay_one("anonymised_up_56bit_1.pulses");
    let down = replay_one("anonymised_down_56bit_1.pulses");
    let my = replay_one("anonymised_my_56bit_1.pulses");
    assert_eq!(up.address, down.address);
    assert_eq!(up.address, my.address);
}

/// The source must be transparent. Anything it did to the stream — dropping a
/// pulse, repeating one, reordering — would show up as a difference here even
/// where the decoder happened to shrug it off.
#[test]
fn replay_yields_the_capture_verbatim() {
    let pulses = support::load_fixture(FIXTURES, "anonymised_up_56bit_1.pulses");
    let mut source = ReplayPulseSource::new(&pulses);
    let mut seen: std::vec::Vec<Pulse> = std::vec::Vec::new();
    while let Some(pulse) = block_on(source.next_pulse()) {
        seen.push(pulse);
    }
    assert_eq!(seen, pulses);
}

/// Exhaustion is permanent: a pump loop that stops on the first `None` must not
/// be restarted by a later call handing out pulses again.
#[test]
fn an_exhausted_source_stays_exhausted() {
    let pulses = [Pulse {
        high: true,
        micros: 640,
    }];
    let mut source = ReplayPulseSource::new(&pulses);
    assert_eq!(block_on(source.next_pulse()), Some(pulses[0]));
    assert_eq!(block_on(source.next_pulse()), None);
    assert_eq!(block_on(source.next_pulse()), None);
}

#[test]
fn an_empty_source_yields_nothing() {
    let mut source = ReplayPulseSource::new(&[]);
    assert_eq!(block_on(source.next_pulse()), None);
}
