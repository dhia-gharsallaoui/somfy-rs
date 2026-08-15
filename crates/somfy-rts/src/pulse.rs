use heapless::Vec;

/// One OOK pulse: carrier on (`high`) or off for `micros` microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pulse {
    pub high: bool,
    pub micros: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    First,
    Repeat,
}

/// Timing constants in µs used by the transmit path. Widely-cited "folklore"
/// values for this protocol (9415 / 89565 / 4550 / 604 / 30415) circulate
/// online but do NOT match what a real transmitter emits — the values below
/// are the ones actually produced when a frame is sent, and the wake-up and
/// sync counts have been cross-checked against real hardware captures (see
/// `docs/provenance.md`).
#[allow(non_snake_case)]
pub mod TIMINGS {
    /// Wake-up pulse HIGH, 10920µs. The commonly cited 9415µs figure is only
    /// a receiver-side detection threshold, not the duration a transmitter
    /// actually holds the line high for.
    pub const WAKEUP_HIGH: u32 = 10_920;
    /// Silence after the wake-up pulse, 7357µs. The commonly cited 89565µs
    /// figure is only a receiver-side tolerance window, not the silence a
    /// transmitter actually produces.
    pub const WAKEUP_LOW: u32 = 7357;
    /// Hardware sync half-pulse, 2560µs — four half-symbol widths
    /// (4 * 640µs).
    pub const HW_SYNC_HALF: u32 = 2560;
    /// Software sync HIGH pulse, 4850µs. Earlier firmware revisions used
    /// 4450/4550µs for this pulse; 4850µs is the value a current transmitter
    /// emits.
    pub const SW_SYNC_HIGH: u32 = 4850;
    /// Manchester half-symbol width, 640µs — the base unit each encoded bit
    /// is built from.
    pub const HALF_SYMBOL: u32 = 640;
    /// Inter-frame silence for 56-bit frames, 27434µs. Produced as two
    /// consecutive 13717µs delays rather than one, because a single delay
    /// call is capped below that range. A commonly cited 30415µs figure
    /// exists for this gap but is not what gets emitted in practice.
    pub const INTER_FRAME_GAP: u32 = 27_434;
}

/// Render an encoded frame to OOK pulses. The frame size selects the protocol:
/// 7 bytes = 56-bit, 10 bytes = 80-bit.
///
/// Manchester polarity: bit `1` = low half then high half, bit `0` = high
/// half then low half, bits sent MSB-first.
///
/// Sync counts and the trailing gap are byte-length driven:
/// - **56-bit**: first frame = wake-up + 2 hardware syncs, repeat = 7 syncs;
///   every frame ends with the inter-frame gap. Hardware-verified: a captured
///   first frame reported 2 hardware syncs and a captured repeat reported 7
///   (see `docs/provenance.md`).
/// - **80-bit**: first frame = wake-up + 12 hardware syncs, repeat = 6 syncs;
///   the inter-frame gap is **suppressed** for this protocol. The wake-up
///   still fires on the first frame regardless of which sync count applies.
///
/// Adjacent same-level half-symbols are intentionally NOT merged here; that is
/// the concern of a later RMT-encoding layer.
pub fn render_pulses(bytes: &[u8], kind: FrameKind, out: &mut Vec<Pulse, 320>) {
    let is_80 = bytes.len() == 10;
    let hw_syncs = match kind {
        FrameKind::First => {
            out.push(Pulse {
                high: true,
                micros: TIMINGS::WAKEUP_HIGH,
            })
            .unwrap();
            out.push(Pulse {
                high: false,
                micros: TIMINGS::WAKEUP_LOW,
            })
            .unwrap();
            if is_80 {
                12
            } else {
                2
            }
        }
        FrameKind::Repeat => {
            if is_80 {
                6
            } else {
                7
            }
        }
    };
    for _ in 0..hw_syncs {
        out.push(Pulse {
            high: true,
            micros: TIMINGS::HW_SYNC_HALF,
        })
        .unwrap();
        out.push(Pulse {
            high: false,
            micros: TIMINGS::HW_SYNC_HALF,
        })
        .unwrap();
    }
    out.push(Pulse {
        high: true,
        micros: TIMINGS::SW_SYNC_HIGH,
    })
    .unwrap();
    out.push(Pulse {
        high: false,
        micros: TIMINGS::HALF_SYMBOL,
    })
    .unwrap();

    for byte in bytes {
        for bit in (0..8).rev() {
            let one = (byte >> bit) & 1 == 1;
            out.push(Pulse {
                high: !one,
                micros: TIMINGS::HALF_SYMBOL,
            })
            .unwrap();
            out.push(Pulse {
                high: one,
                micros: TIMINGS::HALF_SYMBOL,
            })
            .unwrap();
        }
    }
    // Inter-frame silence for 56-bit only; suppressed for 80-bit.
    if !is_80 {
        out.push(Pulse {
            high: false,
            micros: TIMINGS::INTER_FRAME_GAP,
        })
        .unwrap();
    }
}

/// Collapse runs of same-level pulses into single edge-to-edge segments.
///
/// [`render_pulses`] emits one entry per Manchester half-symbol, so a `1`
/// followed by a `0` produces two adjacent HIGH halves. Both the RMT
/// transmitter and a `CHANGE`-interrupt receiver see edges, not halves — this
/// converts between the two representations. Total duration is preserved.
pub fn merge_pulses(input: &[Pulse], out: &mut Vec<Pulse, 320>) {
    out.clear();
    for p in input {
        match out.last_mut() {
            Some(last) if last.high == p.high => last.micros += p.micros,
            _ => out.push(*p).unwrap(),
        }
    }
}
