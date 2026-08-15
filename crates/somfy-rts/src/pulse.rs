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

/// Timing constants in µs, ported verbatim from the TX path of the C++
/// reference `ESPSomfy-RTS/src/Somfy.cpp` (`Transceiver::sendFrame`). The
/// widely-cited "folklore" values (9415 / 89565 / 4550 / 604 / 30415) do NOT
/// match what this transmitter actually emits — the authoritative TX values
/// are used here, each with its source line.
#[allow(non_snake_case)]
pub mod TIMINGS {
    /// Wake-up pulse HIGH. Somfy.cpp:4321 `delayMicroseconds(10920)`.
    /// (The online 9415 value is only an RX-detection reference, Somfy.cpp:4221.)
    pub const WAKEUP_HIGH: u32 = 10_920;
    /// Silence after wake-up pulse. Somfy.cpp:4328 `delayMicroseconds(7357)`.
    /// (Folklore 89565 is only an RX tolerance reference, Somfy.cpp:4224.)
    pub const WAKEUP_LOW: u32 = 7357;
    /// Hardware sync half. Somfy.cpp:4337/4339 `delayMicroseconds(4 * SYMBOL)`
    /// with `#define SYMBOL 640` (Somfy.cpp:23) -> 4 * 640 = 2560.
    pub const HW_SYNC_HALF: u32 = 2560;
    /// Software sync HIGH. Somfy.cpp:4344 `delayMicroseconds(4850)`
    /// (the commented-out 4450/4550 were earlier timings, Somfy.cpp:4343).
    pub const SW_SYNC_HIGH: u32 = 4850;
    /// Manchester half-symbol. `#define SYMBOL 640` (Somfy.cpp:23), emitted at
    /// Somfy.cpp:4347/4354/4356/4360/4362.
    pub const HALF_SYMBOL: u32 = 640;
    /// Inter-frame silence (56-bit). Somfy.cpp:4380-4381 emits two
    /// `delayMicroseconds(13717)` calls (split because the API caps at 16383),
    /// so 13717 + 13717 = 27434. (The named `tempo_if_gap = 30415` at
    /// Somfy.cpp:4235 is defined but never referenced in the TX path.)
    pub const INTER_FRAME_GAP: u32 = 27_434;
}

/// Render an encoded frame to OOK pulses. The frame size selects the protocol:
/// 7 bytes = 56-bit, 10 bytes = 80-bit.
///
/// Manchester polarity verified against Somfy.cpp:4351-4364: bit `1` = low
/// half then high half, bit `0` = high half then low half, bits sent MSB-first
/// (`frame[i/8] >> (7 - i%8)`, Somfy.cpp:4352).
///
/// Sync counts and the trailing gap are byte-length driven, mirroring the C++
/// callers and transmitter:
/// - **56-bit**: first frame = wake-up + 2 hardware syncs, repeat = 7 syncs
///   (Somfy.cpp:4000/4004/4014/4019); every frame ends with the inter-frame
///   gap.
/// - **80-bit**: first frame = wake-up + 12 hardware syncs, repeat = 6 syncs
///   (same callers, `bitLength == 80` arm); the inter-frame gap is **suppressed**
///   (`if(bitLength != 80)`, Somfy.cpp:4379). The wake-up still fires on the
///   first frame because `sendFrame` triggers it for `sync == 2 || sync == 12`
///   (Somfy.cpp:4314).
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
    // Inter-frame silence for 56-bit only; suppressed for 80-bit
    // (Somfy.cpp:4379).
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
