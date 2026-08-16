//! The radio loop, driven on the host: real wall-remote captures in one
//! direction, a recording transmitter in the other.
//!
//! No hardware and no executor. [`somfy_rmt::ReplayPulseSource`] stands in for
//! the RMT receiver and never pends, so `embassy_futures::block_on` completes
//! every step in one poll; the transmit side is a double that records what it
//! was asked to do, which is how the *shape* of a burst — key on, first frame,
//! repeats, park whatever happened — becomes a testable claim.
//!
//! What this cannot establish is that a real CC1101 hears anything, or that a
//! real motor accepts what goes out. That is on-air bring-up's job, and nothing
//! here should be read as standing in for it.

use core::cell::RefCell;
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use somfy_rmt::ReplayPulseSource;
use somfy_rts::{
    encode56, encode80, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse,
};
use somfy_store::{FrameBits, TransmitRequest};
use somfy_tasks::{FrameChannel, RadioEvent, RadioLoop, Transmitter};

// The captures live in `somfy-rts`, next to the decoder they were taken to
// pin. Read-only here, as everywhere.
#[path = "../../somfy-rts/tests/support/mod.rs"]
mod capture;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../somfy-rts/tests/fixtures");

const ADDRESS: u32 = 0x00_1234;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Keyed {
    On,
    Frame { bytes: Vec<u8>, kind: FrameKind },
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RadioDead;

/// A transmitter that records the sequence it was driven through, and can be
/// made to fail at a chosen point.
struct MockTransmitter<'a> {
    log: &'a RefCell<Vec<Keyed>>,
    fail_key_on: bool,
    /// Frame index (0 = first frame) at which `send_frame` starts failing.
    fail_frame_at: Option<u8>,
    fail_key_off: bool,
    frames: u8,
}

impl<'a> MockTransmitter<'a> {
    fn new(log: &'a RefCell<Vec<Keyed>>) -> Self {
        Self {
            log,
            fail_key_on: false,
            fail_frame_at: None,
            fail_key_off: false,
            frames: 0,
        }
    }
}

impl Transmitter for MockTransmitter<'_> {
    type Error = RadioDead;

    fn key_on(&mut self) -> Result<(), RadioDead> {
        if self.fail_key_on {
            return Err(RadioDead);
        }
        self.log.borrow_mut().push(Keyed::On);
        self.frames = 0;
        Ok(())
    }

    async fn send_frame(&mut self, bytes: &[u8], kind: FrameKind) -> Result<(), RadioDead> {
        let index = self.frames;
        self.frames += 1;
        if self.fail_frame_at == Some(index) {
            return Err(RadioDead);
        }
        self.log.borrow_mut().push(Keyed::Frame {
            bytes: bytes.to_vec(),
            kind,
        });
        Ok(())
    }

    fn key_off(&mut self) -> Result<(), RadioDead> {
        if self.fail_key_off {
            return Err(RadioDead);
        }
        self.log.borrow_mut().push(Keyed::Off);
        Ok(())
    }
}

fn request(bits: FrameBits, repeats: u8) -> TransmitRequest {
    TransmitRequest {
        frame: Frame {
            key: 0xA7,
            command: Command::Up,
            rolling_code: 7,
            address: ADDRESS,
        },
        bits,
        repeats,
    }
}

/// The transmit channel's producer end is unreachable from outside
/// `somfy-tasks` without a committed ticket, which is the point — so a test
/// that only wants the *radio* half uses a plain channel it owns.
type Requests = embassy_sync::channel::Channel<NoopRawMutex, TransmitRequest, 4>;

#[test]
fn a_real_capture_replays_into_a_decoded_frame() {
    let pulses = capture::load_fixture(FIXTURES, "up_56bit_1.pulses");
    let requests: Requests = Requests::new();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&pulses),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    let event = block_on(radio.step());

    let RadioEvent::Received(frame) = event else {
        panic!("expected a decoded frame, got {event:?}");
    };
    assert_eq!(frame.command, Command::Up);
    // And it reached the state task's end of the channel, not just the return
    // value: publishing is the half of this that the firmware depends on.
    assert_eq!(frames.try_receive().expect("published"), frame);
}

/// Each of the three captures is one press, so the loop yields one frame and
/// then reports the source finished.
#[test]
fn a_spent_source_reports_finished_once_its_frame_is_out() {
    for (name, expected) in [
        ("up_56bit_1.pulses", Command::Up),
        ("down_56bit_1.pulses", Command::Down),
        ("my_56bit_1.pulses", Command::My),
    ] {
        let pulses = capture::load_fixture(FIXTURES, name);
        let requests: Requests = Requests::new();
        let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
        let log = RefCell::new(Vec::new());
        let mut radio = RadioLoop::new(
            ReplayPulseSource::new(&pulses),
            MockTransmitter::new(&log),
            requests.receiver(),
            frames.sender(),
        );

        match block_on(radio.step()) {
            RadioEvent::Received(frame) => assert_eq!(frame.command, expected, "{name}"),
            other => panic!("{name}: expected a frame, got {other:?}"),
        }
        assert!(
            matches!(block_on(radio.step()), RadioEvent::SourceFinished),
            "{name}: source should be spent"
        );
    }
}

/// Overflow costs the frame and says so, rather than holding the radio task
/// until the state task catches up.
#[test]
fn a_full_frame_channel_drops_the_frame_and_reports_it() {
    let pulses = capture::load_fixture(FIXTURES, "up_56bit_1.pulses");
    let requests: Requests = Requests::new();
    // Depth 1, already full.
    let frames: FrameChannel<NoopRawMutex, 1> = FrameChannel::new();
    frames
        .try_send(Frame {
            key: 0,
            command: Command::My,
            rolling_code: 0,
            address: 1,
        })
        .unwrap();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&pulses),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    assert!(matches!(
        block_on(radio.step()),
        RadioEvent::ReceiveQueueFull(_)
    ));
}

/// Garbage on the air is counted, not escalated: a marginal signal that
/// completes 56 bits and fails its checksum is an ordinary event for a
/// receiver, and the loop must carry on.
#[test]
fn a_corrupted_burst_is_reported_and_the_loop_survives_it() {
    // A well-formed 56-bit transmission whose payload does not check out: one
    // bit flipped in the encoded bytes before they were rendered. The decoder
    // still collects 56 bits — it is the checksum that refuses it, which is the
    // case a receiver actually meets on a marginal signal.
    let mut bytes = encode56(&request(FrameBits::Bits56, 0).frame).unwrap();
    bytes[6] ^= 0x01;
    let mut rendered: heapless::Vec<Pulse, 320> = heapless::Vec::new();
    render_pulses(&bytes, FrameKind::First, &mut rendered);
    let mut pulses: heapless::Vec<Pulse, 320> = heapless::Vec::new();
    merge_pulses(&rendered, &mut pulses);

    let requests: Requests = Requests::new();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&pulses),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    match block_on(radio.step()) {
        RadioEvent::Undecodable { bit_length } => assert_eq!(bit_length, 56),
        RadioEvent::Received(_) => panic!("a flipped bit should not still verify"),
        other => panic!("expected an undecodable burst, got {other:?}"),
    }
    assert!(frames.try_receive().is_err(), "nothing may be published");
    // Still alive: the next step reaches the end of the capture rather than
    // wedging.
    assert!(matches!(block_on(radio.step()), RadioEvent::SourceFinished));
}

#[test]
fn a_burst_is_keyed_on_once_around_all_its_frames() {
    let requests: Requests = Requests::new();
    requests.try_send(request(FrameBits::Bits56, 2)).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    let event = block_on(radio.step());

    assert_eq!(
        event,
        RadioEvent::Transmitted {
            rolling_code: 7,
            frames: 3
        }
    );
    let bytes = encode56(&request(FrameBits::Bits56, 2).frame)
        .unwrap()
        .to_vec();
    assert_eq!(
        log.into_inner(),
        vec![
            Keyed::On,
            Keyed::Frame {
                bytes: bytes.clone(),
                kind: FrameKind::First
            },
            Keyed::Frame {
                bytes: bytes.clone(),
                kind: FrameKind::Repeat
            },
            Keyed::Frame {
                bytes,
                kind: FrameKind::Repeat
            },
            Keyed::Off,
        ]
    );
}

/// An 80-bit frame re-encodes its tail for every repeat index, so a burst that
/// encoded once and resent the same bytes would put the first frame's tail on
/// every repeat. Nothing on air would report it.
#[test]
fn an_80_bit_burst_re_encodes_every_repeat() {
    let requests: Requests = Requests::new();
    let request = TransmitRequest {
        frame: Frame {
            key: 0xA3,
            command: Command::Favorite,
            rolling_code: 3,
            address: ADDRESS,
        },
        bits: FrameBits::Bits80,
        repeats: 2,
    };
    requests.try_send(request).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    block_on(radio.step());

    let sent: Vec<Vec<u8>> = log
        .borrow()
        .iter()
        .filter_map(|entry| match entry {
            Keyed::Frame { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 3);
    for (index, bytes) in sent.iter().enumerate() {
        assert_eq!(
            bytes.as_slice(),
            &encode80(&request.frame, index as u8)[..],
            "repeat {index} must carry its own encoding"
        );
    }
    assert_ne!(sent[0], sent[1], "the tail must differ between repeats");
}

/// A burst that fails part-way still parks the radio. Leaving a synthesiser
/// running holds the band and stops the receiver hearing anything again — a
/// far worse outcome than the frame that was lost.
#[test]
fn a_failed_frame_still_parks_the_radio() {
    let requests: Requests = Requests::new();
    requests.try_send(request(FrameBits::Bits56, 2)).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut transmitter = MockTransmitter::new(&log);
    transmitter.fail_frame_at = Some(1);
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        transmitter,
        requests.receiver(),
        frames.sender(),
    );

    assert_eq!(
        block_on(radio.step()),
        RadioEvent::TransmitFailed(RadioDead)
    );
    let log = log.into_inner();
    assert_eq!(log.last(), Some(&Keyed::Off), "the radio must be parked");
    assert_eq!(
        log.iter()
            .filter(|entry| matches!(entry, Keyed::Frame { .. }))
            .count(),
        1,
        "the burst must stop at the failure, not push on"
    );
}

/// A transmit failure outranks a parking failure in the report: it is the one
/// that explains why nothing moved.
#[test]
fn a_send_failure_outranks_a_parking_failure() {
    let requests: Requests = Requests::new();
    requests.try_send(request(FrameBits::Bits56, 0)).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut transmitter = MockTransmitter::new(&log);
    transmitter.fail_frame_at = Some(0);
    transmitter.fail_key_off = true;
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        transmitter,
        requests.receiver(),
        frames.sender(),
    );

    assert_eq!(
        block_on(radio.step()),
        RadioEvent::TransmitFailed(RadioDead)
    );
}

/// A radio that will not key sends nothing at all — and in particular does not
/// clock symbols at a transmitter that is switched off. It is still put back to
/// receiving: keying goes through IDLE, so a failure part-way leaves the radio
/// in neither mode, and a controller that goes deaf on its first failed burst
/// is indistinguishable from a quiet house.
#[test]
fn a_radio_that_will_not_key_returns_to_receiving() {
    let requests: Requests = Requests::new();
    requests.try_send(request(FrameBits::Bits56, 2)).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut transmitter = MockTransmitter::new(&log);
    transmitter.fail_key_on = true;
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        transmitter,
        requests.receiver(),
        frames.sender(),
    );

    assert_eq!(
        block_on(radio.step()),
        RadioEvent::TransmitFailed(RadioDead)
    );
    assert_eq!(
        log.into_inner(),
        vec![Keyed::Off],
        "nothing keyed, nothing sent, and the radio put back to listening"
    );
}

/// A pending transmission takes precedence over pulses waiting to be read.
/// It has to: servicing it keys the radio out of receive, so the two are
/// exclusive, and a receiver that always won would starve the transmitter for
/// as long as somebody kept talking.
#[test]
fn a_pending_transmission_pre_empts_a_waiting_pulse() {
    let pulses = capture::load_fixture(FIXTURES, "up_56bit_1.pulses");
    let requests: Requests = Requests::new();
    requests.try_send(request(FrameBits::Bits56, 0)).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&pulses),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    assert_eq!(
        block_on(radio.step()),
        RadioEvent::Transmitted {
            rolling_code: 7,
            frames: 1
        }
    );
    assert!(frames.try_receive().is_err(), "nothing was received yet");
    // And the receive path is still there afterwards: the capture decodes on
    // the next step, from where the cancelled arm left off.
    assert!(matches!(block_on(radio.step()), RadioEvent::Received(_)));
}

/// A dead receiver is no reason to stop moving shades. Once the source is
/// finished the loop stops asking it — which is also what keeps it from
/// spinning the executor on a source that resolves instantly forever.
#[test]
fn a_finished_source_still_transmits() {
    let requests: Requests = Requests::new();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    assert!(matches!(block_on(radio.step()), RadioEvent::SourceFinished));

    requests.try_send(request(FrameBits::Bits56, 1)).unwrap();
    assert_eq!(
        block_on(radio.step()),
        RadioEvent::Transmitted {
            rolling_code: 7,
            frames: 2
        }
    );
}

/// Two bursts back to back go out as two bursts, each keyed on its own.
#[test]
fn consecutive_requests_are_keyed_separately() {
    let requests: Requests = Requests::new();
    requests.try_send(request(FrameBits::Bits56, 0)).unwrap();
    requests.try_send(request(FrameBits::Bits56, 0)).unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    block_on(radio.step());
    block_on(radio.step());

    let keying: Vec<Keyed> = log
        .into_inner()
        .into_iter()
        .filter(|entry| !matches!(entry, Keyed::Frame { .. }))
        .collect();
    assert_eq!(keying, vec![Keyed::On, Keyed::Off, Keyed::On, Keyed::Off]);
}

/// A command `somfy-rts` refuses to put in a 56-bit frame must not key the
/// radio at all — a carrier with nothing behind it is worse than silence.
#[test]
fn an_unencodable_request_never_keys_the_radio() {
    let requests: Requests = Requests::new();
    requests
        .try_send(TransmitRequest {
            frame: Frame {
                key: 0xA0,
                command: Command::Favorite,
                rolling_code: 1,
                address: ADDRESS,
            },
            // Favorite is an extended command; a 56-bit frame cannot carry it.
            bits: FrameBits::Bits56,
            repeats: 2,
        })
        .unwrap();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());
    let mut radio = RadioLoop::new(
        ReplayPulseSource::new(&[]),
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    assert!(matches!(block_on(radio.step()), RadioEvent::Unencodable(_)));
    assert!(log.into_inner().is_empty());
}

/// A burst arriving as two receptions still decodes: the loop keeps one
/// decoder across bursts rather than resetting at every boundary, which is
/// what the RMT receiver's seam between transactions actually looks like.
#[test]
fn a_capture_split_across_two_sources_still_decodes() {
    let pulses = capture::load_fixture(FIXTURES, "up_56bit_1.pulses");
    let split = pulses.len() / 2;
    let requests: Requests = Requests::new();
    let frames: FrameChannel<NoopRawMutex, 4> = FrameChannel::new();
    let log = RefCell::new(Vec::new());

    // `Chained` hands out the first half, then the second, then nothing —
    // exactly what one `PulseSource` refilling from a second reception looks
    // like from the loop's side.
    struct Chained<'a> {
        parts: Vec<&'a [Pulse]>,
        at: usize,
    }
    impl somfy_rmt::PulseSource for Chained<'_> {
        async fn next_pulse(&mut self) -> Option<Pulse> {
            while let Some(part) = self.parts.first_mut() {
                if let Some(pulse) = part.get(self.at) {
                    self.at += 1;
                    return Some(*pulse);
                }
                self.parts.remove(0);
                self.at = 0;
            }
            None
        }
    }

    let mut radio = RadioLoop::new(
        Chained {
            parts: vec![&pulses[..split], &pulses[split..]],
            at: 0,
        },
        MockTransmitter::new(&log),
        requests.receiver(),
        frames.sender(),
    );

    match block_on(radio.step()) {
        RadioEvent::Received(frame) => assert_eq!(frame.command, Command::Up),
        other => panic!("expected a frame across the seam, got {other:?}"),
    }
}
