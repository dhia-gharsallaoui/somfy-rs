//! Replace the payload of a captured RTS pulse train while keeping the
//! transmitter's measured timing.
//!
//! # Why this exists
//!
//! A `.pulses` capture holds no address in plain text, but the pulse train *is*
//! the frame: run one through `somfy_rts::RxDecoder` and `decode56` and out
//! comes the transmitting remote's 24-bit address and its rolling code at the
//! moment of capture. That makes a captured file as publishable as a key, and
//! this repository is public.
//!
//! Deleting the captures was the alternative and it costs more than it looks.
//! `somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US` is *measured from them* —
//! `somfy-rts/tests/measured.rs` re-derives it on every run and
//! `somfy-rmt` sizes its RMT idle threshold from it at compile time. Delete the
//! evidence and a firmware constant becomes a number nobody can account for.
//!
//! So this tool keeps the timing and throws away the payload.
//!
//! # What it preserves, and why each is safe to publish
//!
//! - **The preamble, verbatim.** The wake-up pulse, the silence after it, the
//!   hardware-sync halves and the software-sync HIGH are the same durations in
//!   every frame a remote sends; they encode nothing. They are also the most
//!   load-bearing part of the file: a real remote's post-wake-up gap is ~17.7 ms
//!   where this crate's transmit constant says 7357 µs, and that discrepancy is
//!   the whole reason the constant above exists.
//! - **The key byte and the command.** The key byte is `0xA0 | n` for a counter
//!   the remote increments per press; it names no remote. The command is what
//!   the golden tests assert.
//! - **The half-symbol timing error, as measurements rather than as
//!   positions.** Every duration in the rewritten body is `640 µs` plus a
//!   deviation this very capture produced on one of its own *single*
//!   half-symbol segments, applied in the order they were measured and cycled
//!   to cover the frame.
//!
//! # Why the deviations are re-ordered rather than kept in place
//!
//! This is the part that is easy to get wrong, so it is spelled out.
//!
//! The obvious method — keep each half-symbol's own deviation at its own index
//! and merge under the new bit pattern — **puts the original payload back into
//! the file**. A merged segment is two half-symbols, and only their *sum* was
//! ever observed, so keeping position means splitting that sum: the two halves
//! come out equal to within one microsecond. Wherever the new bit pattern then
//! separates them, the file carries two adjacent segments with near-identical
//! deviations, and scanning for those twins reconstructs which halves used to be
//! merged — which is exactly the sequence of bit transitions, which is the
//! original 56 bits, which is the address.
//!
//! That is measured, not feared: built both ways and attacked, the
//! position-preserving variant gave up **13 of the `up` capture's 30 merged
//! pairs at 100% precision** from the anonymised file alone, and this one gave
//! up none. The figures and the attack are in the fixtures README.
//!
//! Cycling a pool of independently measured single-half deviations has no such
//! structure to recover: the pool is a multiset with no position in it. What it
//! still discloses is the pool's *size*, and through it the original's edge
//! count. That reveals how many bit transitions the original payload had —
//! about three bits of information about a 40-bit secret, and the residual is
//! recorded in `crates/somfy-rts/tests/fixtures/README.md` rather than
//! pretended away.
//!
//! Only *single*-half segments enter the pool. A merged segment's deviation
//! cannot be attributed to one half or the other without an assumption, and an
//! assumption is what this tool is trying not to make. Discarding them costs
//! nothing measurable: on all three captures the merged segments' *pair* sums
//! span the same range as the singles do, which is the cross-check that the
//! singles are representative.
//!
//! # What is unavoidably lost
//!
//! The bits are this project's encoder's now, so a rewritten capture can no
//! longer prove that our checksum and de-obfuscation agree with Somfy's. It
//! still pins them, because the bytes are frozen at rewrite time and a later
//! change to either would stop the file decoding — but that is regression
//! cover, not interoperability evidence. Said plainly in the fixtures README
//! too, because it is the one claim these files can no longer make.

use std::path::Path;

use somfy_rts::{decode56, encode56, Command, Frame, Pulse, RxDecoder, TIMINGS};

/// Sub-448 µs entries are glitches the capture ISR logs but never counts (see
/// the fixtures README). The captures this tool was written for contain none,
/// and one appearing would break the "every body segment is one or two
/// half-symbols" model below, so it is refused rather than filtered.
const GLITCH_MIN_US: u32 = 448;

/// A 56-bit frame is 56 bits of two half-symbols, plus the software sync's LOW
/// tail, less the final bit's second half — which no capture contains, because
/// the receive ISR stops recording the moment the last bit is stored.
const BODY_HALVES: usize = 1 + 2 * 56 - 1;

pub struct Args {
    pub input: String,
    pub output: String,
    pub address: u32,
    pub rolling_code: u16,
    pub captured: String,
}

/// Rewrite one capture. Returns the summary line printed to the operator.
///
/// Nothing recovered from the input's payload beyond the key byte and the
/// command is returned, printed or written: the address and rolling code this
/// exists to remove never leave this function.
pub fn run(args: &Args) -> Result<String, String> {
    let text =
        std::fs::read_to_string(&args.input).map_err(|error| format!("{}: {error}", args.input))?;
    let durations = parse(&text)?;
    let (preamble, body) = split(&durations)?;
    let halves = halves_of(body)?;

    let original = decode_body(&halves)?;
    // Deliberately partial: `original.address` and `original.rolling_code` are
    // dropped here and are never read again.
    let rewritten = Frame {
        key: original.key,
        command: original.command,
        rolling_code: args.rolling_code,
        address: args.address,
    };
    let bytes = encode56(&rewritten).map_err(|error| format!("{}: {error:?}", args.input))?;

    let pool = single_half_deviations(body);
    if pool.is_empty() {
        return Err(format!(
            "{}: no single half-symbol segments to measure",
            args.input
        ));
    }
    let new_body = render_body(&bytes, &pool);

    let header = header(args, &original, preamble, body, &new_body, &pool);
    let mut out = header;
    for duration in preamble.iter().chain(new_body.iter()) {
        out.push_str(&duration.to_string());
        out.push('\n');
    }
    std::fs::write(&args.output, &out).map_err(|error| format!("{}: {error}", args.output))?;

    // Prove the rewritten file decodes to what the header claims, through the
    // same decoder the tests use, before the operator is told it worked.
    verify(&out, &rewritten)?;

    Ok(format!(
        "{} -> {}: {} pulses (was {}), body {} µs (was {}), pool of {} measured deviations \
         in {}..{} µs",
        Path::new(&args.input)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        Path::new(&args.output)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        preamble.len() + new_body.len(),
        durations.len(),
        new_body.iter().sum::<u32>(),
        body.iter().sum::<u32>(),
        pool.len(),
        pool.iter().min().copied().unwrap_or_default(),
        pool.iter().max().copied().unwrap_or_default(),
    ))
}

fn parse(text: &str) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.contains(',') {
            return Err("only the durations-only format is supported".to_string());
        }
        let micros: u32 = line
            .parse()
            .map_err(|_| format!("not a duration: '{line}'"))?;
        if micros < GLITCH_MIN_US {
            return Err(format!(
                "glitch entry {micros} µs: this tool needs a glitch-free capture"
            ));
        }
        out.push(micros);
    }
    Ok(out)
}

/// Within `±25%`, the same window `somfy_rts::RxDecoder` accepts.
fn within(actual: u32, expected: u32) -> bool {
    actual >= expected - expected / 4 && actual <= expected + expected / 4
}

/// Cut the capture at the software-sync HIGH: everything up to and including it
/// is preamble and is kept verbatim, everything after it is payload.
fn split(durations: &[u32]) -> Result<(&[u32], &[u32]), String> {
    let sync = durations
        .iter()
        .position(|d| within(*d, TIMINGS::SW_SYNC_HIGH))
        .ok_or("no software-sync pulse found")?;
    // Levels alternate from HIGH, so an even index is a HIGH — which the
    // software sync must be.
    if sync % 2 != 0 {
        return Err(format!(
            "the software sync is at index {sync}, which is a LOW"
        ));
    }
    let hw = &durations[2..sync];
    if hw.is_empty() || !hw.iter().all(|d| within(*d, TIMINGS::HW_SYNC_HALF)) {
        return Err(
            "the segments between the wake-up gap and the software sync are not \
                    all hardware-sync halves"
                .to_string(),
        );
    }
    Ok(durations.split_at(sync + 1))
}

/// How many half-symbols each body segment spans. A capture's body is made only
/// of one- and two-half segments; anything else means the file is not the shape
/// this tool models.
fn halves_of(body: &[u32]) -> Result<Vec<usize>, String> {
    let mut counts = Vec::with_capacity(body.len());
    for duration in body {
        let k = ((*duration + TIMINGS::HALF_SYMBOL / 2) / TIMINGS::HALF_SYMBOL) as usize;
        if k != 1 && k != 2 {
            return Err(format!("body segment {duration} µs is {k} half-symbols"));
        }
        counts.push(k);
    }
    let total: usize = counts.iter().sum();
    if total != BODY_HALVES {
        return Err(format!(
            "body spans {total} half-symbols, expected {BODY_HALVES}"
        ));
    }
    Ok(counts)
}

/// The deviations from nominal of the body's *single* half-symbol segments, in
/// the order they were measured. Merged segments are skipped: see the module
/// header.
fn single_half_deviations(body: &[u32]) -> Vec<i32> {
    body.iter()
        .filter(|d| **d < TIMINGS::HALF_SYMBOL * 3 / 2)
        .map(|d| *d as i32 - TIMINGS::HALF_SYMBOL as i32)
        .collect()
}

/// Decode the original body through the shipping decoder, so the frame this
/// tool rewrites is the frame the tests would have read.
fn decode_body(halves: &[usize]) -> Result<Frame, String> {
    let mut decoder = RxDecoder::new();
    // The decoder needs the sync it was cut off from; feed it a nominal one.
    for _ in 0..2 {
        decoder.push(Pulse {
            high: true,
            micros: TIMINGS::HW_SYNC_HALF,
        });
        decoder.push(Pulse {
            high: false,
            micros: TIMINGS::HW_SYNC_HALF,
        });
    }
    decoder.push(Pulse {
        high: true,
        micros: TIMINGS::SW_SYNC_HIGH,
    });

    let mut high = false;
    let mut frame = None;
    for k in halves {
        let pulse = Pulse {
            high,
            micros: TIMINGS::HALF_SYMBOL * *k as u32,
        };
        if let Some(raw) = decoder.push(pulse) {
            frame = Some(raw);
        }
        high = !high;
    }
    let raw = frame.ok_or("the capture's body does not decode to a frame")?;
    let bytes: &[u8; 7] = raw
        .bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("decoded {} bits, expected 56", raw.bit_length))?;
    decode56(bytes).map_err(|error| format!("the capture does not decode: {error:?}"))
}

/// Lay the new frame's bits over the measured deviations and merge to the
/// edge-to-edge form a `CHANGE`-interrupt receiver produces.
fn render_body(bytes: &[u8; 7], pool: &[i32]) -> Vec<u32> {
    // Half-symbol levels: the software sync's LOW tail, then each bit as
    // (!bit, bit), truncated where the capture's recording stops.
    let mut levels = Vec::with_capacity(BODY_HALVES);
    levels.push(false);
    'bits: for byte in bytes {
        for shift in (0..8).rev() {
            let one = (byte >> shift) & 1 == 1;
            for level in [!one, one] {
                if levels.len() == BODY_HALVES {
                    break 'bits;
                }
                levels.push(level);
            }
        }
    }

    let mut merged: Vec<(bool, u32)> = Vec::new();
    for (index, level) in levels.iter().enumerate() {
        let micros = (TIMINGS::HALF_SYMBOL as i32 + pool[index % pool.len()]) as u32;
        match merged.last_mut() {
            Some(last) if last.0 == *level => last.1 += micros,
            _ => merged.push((*level, micros)),
        }
    }
    merged.into_iter().map(|(_, micros)| micros).collect()
}

/// Read the file back the way the golden tests do and confirm it carries the
/// frame the header claims. A rewritten capture that does not decode is a
/// silent regression in a file nobody re-reads.
fn verify(contents: &str, expected: &Frame) -> Result<(), String> {
    let durations = parse(contents)?;
    let mut decoder = RxDecoder::new();
    let mut got = None;
    for (index, micros) in durations.iter().enumerate() {
        let pulse = Pulse {
            high: index % 2 == 0,
            micros: *micros,
        };
        if let Some(raw) = decoder.push(pulse) {
            got = Some(raw);
        }
    }
    let raw = got.ok_or("the rewritten capture does not decode")?;
    let bytes: &[u8; 7] = raw
        .bytes
        .as_slice()
        .try_into()
        .map_err(|_| "the rewritten capture is not 56 bits".to_string())?;
    let frame = decode56(bytes).map_err(|error| format!("rewritten capture: {error:?}"))?;
    if frame != *expected {
        return Err("the rewritten capture decodes to a different frame".to_string());
    }
    Ok(())
}

fn command_name(command: Command) -> &'static str {
    match command {
        Command::My => "My",
        Command::Up => "Up",
        Command::Down => "Down",
        other => {
            // Every capture this tool has been run on is one of the three
            // above; anything else still needs a name in the header.
            match other {
                Command::MyUp => "MyUp",
                Command::MyDown => "MyDown",
                Command::UpDown => "UpDown",
                Command::MyUpDown => "MyUpDown",
                Command::Prog => "Prog",
                Command::SunFlag => "SunFlag",
                Command::Flag => "Flag",
                Command::StepDown => "StepDown",
                Command::Toggle => "Toggle",
                Command::Sensor => "Sensor",
                _ => "RtwProto",
            }
        }
    }
}

/// The header is the point of the exercise: a file that does not say which of
/// its numbers are measured and which are invented is worse than no file.
fn header(
    args: &Args,
    original: &Frame,
    preamble: &[u32],
    body: &[u32],
    new_body: &[u32],
    pool: &[i32],
) -> String {
    let command = command_name(original.command);
    let hw_syncs = preamble.len() - 3;
    format!(
        "# Anonymised wall-remote capture — derived from a real one, not itself one.\n\
         #\n\
         # REAL, verbatim from a capture taken {captured} from a physical Somfy wall remote,\n\
         # received by an ESP32-S3 + CC1101 running ESPSomfy-RTS v2.5.6:\n\
         #   - the first {preamble_len} durations below: the wake-up HIGH, the silence after it,\n\
         #     {hw_syncs} hardware-sync halves and the software-sync HIGH, exactly as measured;\n\
         #   - the key byte 0x{key:02X} and the command ({command}), neither of which names a remote;\n\
         #   - every half-symbol deviation from the nominal {half} µs used in the body: each is\n\
         #     one of the {pool_len} deviations this same capture produced on its own single\n\
         #     half-symbol segments ({pool_min}..{pool_max} µs), applied in measured order and\n\
         #     cycled to cover the frame.\n\
         #\n\
         # SYNTHETIC, because the original's pulse train encoded a real remote's address and\n\
         # its rolling code at the moment of capture:\n\
         #   - address 0x{address:06X} — this project's bring-up address, not a remote's;\n\
         #   - rolling code {rolling};\n\
         #   - and so, downstream of those, the checksum, the obfuscation chain, the 56 bits\n\
         #     themselves, and the merged-segment structure that follows from them.\n\
         #\n\
         # WHAT THIS FILE THEREFORE CANNOT SHOW: that our checksum and de-obfuscation agree\n\
         # with Somfy's. Those bits are ours now. It still pins them against *change*, since\n\
         # the bytes are frozen here, but that is regression cover, not interoperability.\n\
         #\n\
         # Rewritten {rewritten} by `cargo run -p xtask -- anonymise-capture`. The method, and\n\
         # what it does and does not preserve, is in fixtures/README.md. Do not hand-edit:\n\
         # the durations are evidence and the original is gone.\n\
         #\n\
         # command={command} bits=56 hwsync={hw_halves} pulseCount={new_len} (the original was\n\
         # {old_len}; an edge count follows from the payload bits, so it moved with them)\n\
         # body total {new_us} µs against the original's {old_us} µs\n\
         # durations-only, one per line; see fixtures/README.md\n",
        captured = args.captured,
        preamble_len = preamble.len(),
        hw_syncs = hw_syncs / 2,
        hw_halves = hw_syncs,
        key = original.key,
        command = command,
        half = TIMINGS::HALF_SYMBOL,
        pool_len = pool.len(),
        pool_min = pool.iter().min().copied().unwrap_or_default(),
        pool_max = pool.iter().max().copied().unwrap_or_default(),
        address = args.address,
        rolling = args.rolling_code,
        rewritten = today(),
        new_len = preamble.len() + new_body.len(),
        old_len = preamble.len() + body.len(),
        new_us = new_body.iter().sum::<u32>(),
        old_us = body.iter().sum::<u32>(),
    )
}

/// The rewrite date, from the environment rather than from a clock crate: this
/// tool runs once per capture and the value only has to be honest.
fn today() -> String {
    std::env::var("SOMFY_ANONYMISE_DATE").unwrap_or_else(|_| {
        let out = std::process::Command::new("date").arg("+%Y-%m-%d").output();
        match out {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => "unknown date".to_string(),
        }
    })
}
