# somfy-rs Plan 4a: Firmware Foundation + TX Path

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust `no_std` firmware on the ESP32-S3 that transmits a Somfy RTS frame through a CC1101 and physically moves a shade.

**Architecture:** Three host-testable layers below one hardware layer. `somfy-rts` gains a repeat-aware `encode80` and a public pulse-merge function. A new `no_std` crate `somfy-rmt` turns merged pulses into RMT symbols with compile-time size checks — pure data, host-tested. The `firmware` crate (its own workspace, espup toolchain) owns the CC1101 SPI driver and the `esp-hal` RMT channel, and does nothing but convert `somfy-rmt` symbols into `esp_hal::rmt::PulseCode` and clock them out.

**Tech Stack:** Rust stable (host crates), espup `esp` channel (firmware), `esp-hal` 1.1.x, `heapless`, `embedded-hal` 1.0, `embedded-hal-mock` (dev), `defmt` + `esp-println`.

**Spec:** [`docs/specs/2026-08-15-plan4-firmware-radio-design.md`](../specs/2026-08-15-plan4-firmware-radio-design.md)

## Global Constraints

- Every commit message: conventional commits (`feat:`, `test:`, `fix:`, `docs:`, `chore:`). **No attribution footers.**
- `cargo test --workspace` at the repo root must stay green **without any ESP toolchain installed**. The `firmware` crate is excluded from the root workspace for exactly this reason.
- `somfy-rts` must remain hardware-free: **no GPIO, timer, RMT, or radio type may appear in its API.** This is documented in its crate docs and is load-bearing — that is why RMT symbol packing lives in `somfy-rmt`, not in `somfy-rts`.
- **Source comments must not reference the C++ reference implementation.** somfy-rs is an independent project; its code stands on its own. Comments explain *what the code does and why*, in this project's own terms — never "ported from X" or a `Somfy.cpp:NNN` citation.
- **Never invent behaviour.** Deriving a constant or an algorithm still means reading the reference at `/home/dhia/Sources/personal/ESPSomfy-RTS/src/Somfy.cpp` rather than guessing — the prohibition is on *citing* it in code, not on *verifying against* it. Record the derivation in `docs/provenance.md` (one row: what was derived, from where, when verified), so the value stays auditable without the code advertising its ancestry.
- Timing constants are already ported and validated against real hardware captures; **do not change `TIMINGS`**.
- RMT tick resolution is **1 µs** (80 MHz source, `clk_divider = 80`). RMT length fields are **15 bits** (max 32767 ticks); `INTER_FRAME_GAP` is 27,434 µs and must fit.
- RMT base clock **must be 80 MHz on ESP32 and ESP32-S2** (esp-hal constraint).
- Verified hardware pin map (matches the user's working production device): `SCK=12 MOSI=11 MISO=13 CSN=10 GDO0(TX)=3 GDO2(RX)=4`, 433.42 MHz, deviation 47.60 kHz, RX bandwidth 99.97 kHz, TX power 10, OOK/ASK, async serial packet format.
- No default chip feature. A bare `cargo build` in `crates/firmware` must fail with a `compile_error!` naming the four options.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/somfy-rts/src/frame.rs` (modify) | `encode80` grows a `repeat` parameter; C++-exact tails |
| `crates/somfy-rts/src/pulse.rs` (modify) | add public `merge_pulses` |
| `crates/somfy-rmt/Cargo.toml` (create) | new `no_std` crate, root-workspace member |
| `crates/somfy-rmt/src/lib.rs` (create) | `RmtSymbol`, `pack`, sizing constants + const asserts |
| `crates/somfy-rmt/tests/pack.rs` (create) | host tests for merge→pack, worst-case sizing |
| `crates/firmware/Cargo.toml` (create) | own workspace; chip features |
| `crates/firmware/rust-toolchain.toml` (create) | pins espup `esp` channel |
| `crates/firmware/src/chip.rs` (create) | per-chip constants; `compile_error!` guard |
| `crates/firmware/src/radio/cc1101.rs` (create) | minimal CC1101 SPI driver |
| `crates/firmware/src/radio/rmt_tx.rs` (create) | `RmtSymbol` → `PulseCode` → RMT channel |
| `crates/firmware/src/main.rs` (create) | bring-up binary: transmit one command |
| `.github/workflows/ci.yml` (modify) | 4-chip clippy+build matrix |

---

### Task 1: Repeat-aware `encode80`

The reference re-encodes the 80-bit tail per repeat. Today's `encode80` is repeat-less and emits a **placeholder** tail for base commands (see the comment at `frame.rs:191-201`). This task makes it C++-exact. Nothing may transmit 80-bit frames until it lands.

**Files:**
- Modify: `crates/somfy-rts/src/frame.rs`
- Modify: `crates/somfy-rts/src/lib.rs`
- Test: `crates/somfy-rts/tests/frame80.rs`

**Interfaces:**
- Consumes: `Frame`, `Command`, `checksum`, `obfuscate` (existing, unchanged)
- Produces: `pub fn encode80(f: &Frame, repeat: u8) -> [u8; 10]` — **breaking change**, all existing call sites take a new second argument. `pub fn calc80_checksum(b7: u8, b8: u8, b9: u8) -> u8` stays as-is.

C++ source of truth, read before implementing:
- `encode80Byte7` — `Somfy.cpp:259-262`
- `encode80BitFrame` — `Somfy.cpp:263-331`
- `calc80Checksum` — `Somfy.cpp:119-126`

- [ ] **Step 1: Write the failing tests**

Add to `crates/somfy-rts/tests/frame80.rs`:

```rust
use somfy_rts::{decode80, encode80, Command, Frame};

fn frame(command: Command) -> Frame {
    Frame { key: 0xA7, command, rolling_code: 0x1234, address: 0x00C0DE }
}

/// `encode80Byte7(196, repeat)` = `196 + 4*repeat`, cycling by -15 whenever
/// the sum would exceed 255 (Somfy.cpp:259-262).
#[test]
fn byte7_progresses_by_four_per_repeat_and_wraps_at_15() {
    let b7 = |r: u8| {
        let mut b = encode80(&frame(Command::Up), r);
        somfy_rts::deobfuscate_for_test(&mut b);
        b[7]
    };
    assert_eq!(b7(0), 196);
    assert_eq!(b7(1), 200);
    assert_eq!(b7(14), 252);
    // repeat 15 would be 256 -> repeat -= 15 -> 0 -> back to 196.
    assert_eq!(b7(15), 196);
    assert_eq!(b7(16), 200);
}

/// Favorite and Stop flip 196 -> 132 on any repeat > 0 (Somfy.cpp:284, 291).
#[test]
fn favorite_and_stop_flip_byte7_on_later_repeats() {
    for cmd in [Command::Favorite, Command::Stop] {
        let mut first = encode80(&frame(cmd), 0);
        let mut later = encode80(&frame(cmd), 1);
        somfy_rts::deobfuscate_for_test(&mut first);
        somfy_rts::deobfuscate_for_test(&mut later);
        assert_eq!(first[7], 196, "{cmd:?} first frame");
        assert_eq!(later[7], 132, "{cmd:?} repeat frame");
    }
}

/// Base-command tails, verbatim from Somfy.cpp:304-326.
#[test]
fn base_command_tails_match_cpp() {
    let cases = [
        (Command::Up, 32u8, 0x00u8),
        (Command::Down, 44, 0x80),
        (Command::My, 0x00, 0x10),
    ];
    for (cmd, b8, b9_hi) in cases {
        let mut b = encode80(&frame(cmd), 0);
        somfy_rts::deobfuscate_for_test(&mut b);
        assert_eq!(b[8], b8, "{cmd:?} byte 8");
        assert_eq!(b[9] & 0xF0, b9_hi, "{cmd:?} byte 9 high nibble");
    }
}

#[test]
fn roundtrips_at_every_repeat() {
    for cmd in [Command::Up, Command::Down, Command::My, Command::Stop, Command::Favorite] {
        for repeat in 0..=16u8 {
            let bytes = encode80(&frame(cmd), repeat);
            let got = decode80(&bytes).expect("decode");
            assert_eq!(got.command, cmd, "cmd {cmd:?} repeat {repeat}");
            assert_eq!(got.address, 0x00C0DE);
            assert_eq!(got.rolling_code, 0x1234);
        }
    }
}
```

Add this test-only helper to `crates/somfy-rts/src/lib.rs` so tests can inspect un-obfuscated bytes:

```rust
/// Test-only: expose de-obfuscation so integration tests can assert on the
/// raw wire bytes (the C++ tail map is defined pre-obfuscation).
#[doc(hidden)]
pub fn deobfuscate_for_test(b: &mut [u8; 10]) {
    frame::deobfuscate_slice(b)
}
```

and in `frame.rs` make the existing de-obfuscation reachable:

```rust
pub(crate) fn deobfuscate_slice(b: &mut [u8; 10]) {
    deobfuscate(b)
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test frame80`
Expected: FAIL — `encode80` takes 1 argument, not 2.

- [ ] **Step 3: Implement**

In `crates/somfy-rts/src/frame.rs`, replace `encode80` and `encode80_tail`:

```rust
/// Byte 7 progression across repeats (`encode80Byte7`, Somfy.cpp:259-262):
///
/// ```c
/// while((repeat * 4) + start > 255) repeat -= 15;
/// return start + (repeat * 4);
/// ```
///
/// The subtraction cycles the sequence with period 15 rather than saturating.
fn encode80_byte7(start: u8, repeat: u8) -> u8 {
    let mut r = repeat as i32;
    let s = start as i32;
    while (r * 4) + s > 255 {
        r -= 15;
    }
    (s + r * 4) as u8
}

/// Encode an 80-bit frame for a given `repeat` index (0 = first frame).
///
/// The reference re-encodes the tail per repeat, so a transmitter MUST call
/// this once per frame it sends with the matching index — see
/// `encode80BitFrame` (Somfy.cpp:263-331).
pub fn encode80(f: &Frame, repeat: u8) -> [u8; 10] {
    let mut b = [0u8; 10];
    b[0] = f.key;
    b[1] = f.command.nibble() << 4;
    b[2] = (f.rolling_code >> 8) as u8;
    b[3] = f.rolling_code as u8;
    b[4] = (f.address >> 16) as u8;
    b[5] = (f.address >> 8) as u8;
    b[6] = f.address as u8;
    encode80_tail(&mut b, f.command, repeat);

    b[1] |= checksum(&b[..7]);
    obfuscate(&mut b);
    b
}

/// Fill un-obfuscated tail bytes 7-9 plus the `calc80Checksum` low nibble of
/// byte 9, verbatim from `encode80BitFrame` (Somfy.cpp:263-331).
///
/// `stepSize` is fixed at 1 here: the C++ defaults it to 1 when unset
/// (Somfy.cpp:268, 276) and this crate has no step-size field. Callers needing
/// a non-default step must extend `Frame` first.
fn encode80_tail(b: &mut [u8; 10], cmd: Command, repeat: u8) {
    const STEP_SIZE: u8 = 1;
    match cmd {
        // Somfy.cpp:266-273. Byte 1 high nibble is rewritten to StepDown on the
        // first frame only; decode80 reverses this via the byte-8 selector.
        Command::StepUp => {
            if repeat == 0 {
                b[1] = (Command::StepDown.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = 132;
            b[8] = ((STEP_SIZE & 0x70) >> 4) | 0x38;
            b[9] = (STEP_SIZE & 0x0F) << 4;
        }
        // Somfy.cpp:274-281.
        Command::StepDown => {
            if repeat == 0 {
                b[1] = (Command::StepDown.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = 132;
            b[8] = ((STEP_SIZE & 0x70) >> 4) | 0x30;
            b[9] = (STEP_SIZE & 0x0F) << 4;
        }
        // Somfy.cpp:282-288.
        Command::Favorite => {
            if repeat == 0 {
                b[1] = (Command::My.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = if repeat > 0 { 132 } else { 196 };
            b[8] = 44;
            b[9] = 0x90;
        }
        // Somfy.cpp:289-295.
        Command::Stop => {
            if repeat == 0 {
                b[1] = (Command::My.nibble() << 4) | (b[1] & 0x0F);
            }
            b[7] = if repeat > 0 { 132 } else { 196 };
            b[8] = 47;
            b[9] = 0xF0;
        }
        // Somfy.cpp:304-309.
        Command::Up => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 32;
            b[9] = 0x00;
        }
        // Somfy.cpp:310-315.
        Command::Down => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 44;
            b[9] = 0x80;
        }
        // My and the multi-button family share one tail (Somfy.cpp:316-326).
        _ => {
            b[7] = encode80_byte7(196, repeat);
            b[8] = 0x00;
            b[9] = 0x10;
        }
    }
    b[9] |= calc80_checksum(b[7], b[8], b[9]);
}
```

Then fix every existing `encode80(&f)` call site to `encode80(&f, 0)`. Find them with:

```sh
grep -rn "encode80(" crates/ --include=*.rs
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p somfy-rts`
Expected: PASS, including the pre-existing `frame80` and property tests.

- [ ] **Step 5: Delete the stale placeholder comment**

Remove the `PLACEHOLDER tail` block at the old `frame.rs:191-201` — it is now false. Replace the crate-level note in `crates/somfy-rts/src/lib.rs` that says `encode80` is repeat-less, and update the "Contracts for later plans" bullet in the top-level `README.md` that records this obligation as outstanding.

- [ ] **Step 6: Commit**

```bash
git add crates/somfy-rts/ README.md
git commit -m "feat: repeat-aware encode80 with C++-exact 80-bit tails"
```

---

### Task 2: Public `merge_pulses`

A `CHANGE`-interrupt receiver and an RMT channel both want edge-to-edge pulses, not unmerged half-symbols. `golden.rs` already hand-rolls this merge for its synthetic fixture; make it a first-class function so `somfy-rmt` and the tests share one implementation.

**Files:**
- Modify: `crates/somfy-rts/src/pulse.rs`
- Modify: `crates/somfy-rts/src/lib.rs` (re-export)
- Modify: `crates/somfy-rts/tests/pulse_tx.rs`

**Interfaces:**
- Produces: `pub fn merge_pulses(input: &[Pulse], out: &mut heapless::Vec<Pulse, 320>)` — collapses runs of same-level pulses by summing durations. Order preserved; output strictly alternates level.

- [ ] **Step 1: Write the failing test**

Append to `crates/somfy-rts/tests/pulse_tx.rs`:

```rust
use heapless::Vec;
use somfy_rts::{merge_pulses, Pulse};

#[test]
fn merges_adjacent_same_level_runs() {
    let input = [
        Pulse { high: true, micros: 640 },
        Pulse { high: true, micros: 640 },
        Pulse { high: false, micros: 640 },
        Pulse { high: true, micros: 640 },
    ];
    let mut out: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&input, &mut out);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0], Pulse { high: true, micros: 1280 });
    assert_eq!(out[1], Pulse { high: false, micros: 640 });
    assert_eq!(out[2], Pulse { high: true, micros: 640 });
}

#[test]
fn merged_output_strictly_alternates() {
    let f = somfy_rts::Frame {
        key: 0xA7,
        command: somfy_rts::Command::Up,
        rolling_code: 0x000A,
        address: 0x00C0DE,
    };
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    somfy_rts::render_pulses(&somfy_rts::encode56(&f).unwrap(), somfy_rts::FrameKind::First, &mut rendered);

    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);

    assert!(!merged.is_empty());
    for pair in merged.windows(2) {
        assert_ne!(pair[0].high, pair[1].high, "merged stream must alternate");
    }
    let total_in: u32 = rendered.iter().map(|p| p.micros).sum();
    let total_out: u32 = merged.iter().map(|p| p.micros).sum();
    assert_eq!(total_in, total_out, "merging must preserve total duration");
}

/// An all-ones payload is the worst case: Manchester renders `1` as (low, high)
/// so no adjacent halves share a level and nothing merges.
#[test]
fn all_ones_payload_does_not_shrink() {
    let bytes = [0xFFu8; 7];
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    somfy_rts::render_pulses(&bytes, somfy_rts::FrameKind::First, &mut rendered);
    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);
    // Only the sync run and the gap boundary can merge; the 112 data halves cannot.
    assert!(merged.len() >= 112, "got {}", merged.len());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p somfy-rts --test pulse_tx`
Expected: FAIL — `merge_pulses` not found.

- [ ] **Step 3: Implement**

Append to `crates/somfy-rts/src/pulse.rs`:

```rust
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
```

Re-export in `crates/somfy-rts/src/lib.rs`:

```rust
pub use pulse::{merge_pulses, render_pulses, FrameKind, Pulse, TIMINGS};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p somfy-rts`
Expected: PASS.

- [ ] **Step 5: Dedupe the golden.rs helper**

`crates/somfy-rts/tests/golden.rs` merges by hand inside `synthetic_up_pulses`. Replace that inline loop with a call to `merge_pulses`. Re-run `cargo test -p somfy-rts --test golden` — **all five tests must still pass**, including the three real-hardware captures.

- [ ] **Step 6: Commit**

```bash
git add crates/somfy-rts/
git commit -m "feat: public merge_pulses; dedupe golden fixture helper"
```

---

### Task 3: `somfy-rmt` — RMT symbol packing

RMT packs **two** (level, duration) pairs into one 32-bit symbol with 15-bit lengths. This crate does that conversion as pure data so it is host-testable; the firmware only maps `RmtSymbol` onto `esp_hal::rmt::PulseCode`.

**Files:**
- Create: `crates/somfy-rmt/Cargo.toml`, `crates/somfy-rmt/src/lib.rs`, `crates/somfy-rmt/tests/pack.rs`
- Modify: `Cargo.toml` (root — add workspace member)

**Interfaces:**
- Consumes: `somfy_rts::{Pulse, merge_pulses, render_pulses, FrameKind}`
- Produces:
  - `pub struct RmtSymbol { pub level1: bool, pub length1: u16, pub level2: bool, pub length2: u16 }`
  - `pub const MAX_SYMBOLS: usize = 96;`
  - `pub const TICK_US: u32 = 1;`
  - `pub const MAX_TICKS: u32 = 32_767;`
  - `pub enum PackError { TooLong { micros: u32 }, TooManySymbols { needed: usize } }`
  - `pub fn pack(merged: &[Pulse], out: &mut heapless::Vec<RmtSymbol, MAX_SYMBOLS>) -> Result<(), PackError>`

- [ ] **Step 1: Create the crate manifest**

`crates/somfy-rmt/Cargo.toml`:

```toml
[package]
name = "somfy-rmt"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
description = "no_std packing of Somfy OOK pulse trains into ESP32 RMT symbols"

[dependencies]
heapless = "0.8"
somfy-rts = { path = "../somfy-rts" }
```

Add to the root `Cargo.toml` members list:

```toml
members = ["crates/somfy-rts", "crates/somfy-domain", "crates/somfy-api", "crates/somfy-migrate", "crates/somfy-rmt"]
```

- [ ] **Step 2: Write the failing tests**

`crates/somfy-rmt/tests/pack.rs`:

```rust
use heapless::Vec;
use somfy_rmt::{pack, PackError, RmtSymbol, MAX_SYMBOLS, MAX_TICKS};
use somfy_rts::{encode56, encode80, merge_pulses, render_pulses, Command, Frame, FrameKind, Pulse};

fn frame(command: Command) -> Frame {
    Frame { key: 0xA7, command, rolling_code: 0x000A, address: 0x00C0DE }
}

fn merged_for(bytes: &[u8], kind: FrameKind) -> Vec<Pulse, 320> {
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(bytes, kind, &mut rendered);
    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);
    merged
}

#[test]
fn packs_two_pulses_per_symbol() {
    let input = [
        Pulse { high: true, micros: 100 },
        Pulse { high: false, micros: 200 },
        Pulse { high: true, micros: 300 },
        Pulse { high: false, micros: 400 },
    ];
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&input, &mut out).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0], RmtSymbol { level1: true, length1: 100, level2: false, length2: 200 });
    assert_eq!(out[1], RmtSymbol { level1: true, length1: 300, level2: false, length2: 400 });
}

/// An odd pulse count leaves the second half of the last symbol empty. A
/// zero-length entry is RMT's end marker, so this is exactly what we want.
#[test]
fn odd_pulse_count_zero_pads_final_symbol() {
    let input = [
        Pulse { high: true, micros: 100 },
        Pulse { high: false, micros: 200 },
        Pulse { high: true, micros: 300 },
    ];
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&input, &mut out).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[1].length1, 300);
    assert_eq!(out[1].length2, 0, "zero length terminates the transmission");
}

#[test]
fn rejects_a_pulse_longer_than_the_15_bit_field() {
    let input = [Pulse { high: true, micros: MAX_TICKS + 1 }];
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    match pack(&input, &mut out) {
        Err(PackError::TooLong { micros }) => assert_eq!(micros, MAX_TICKS + 1),
        other => panic!("expected TooLong, got {other:?}"),
    }
}

/// The longest real pulse is INTER_FRAME_GAP. If this ever fails, the timing
/// model outgrew the RMT length field and rmt_tx must switch to wrap-around
/// refill (design spec §5.2).
#[test]
fn inter_frame_gap_fits_the_length_field() {
    assert!(somfy_rts::TIMINGS::INTER_FRAME_GAP <= MAX_TICKS);
}

#[test]
fn every_frame_shape_fits_in_max_symbols() {
    let cases = [
        (encode56(&frame(Command::Up)).unwrap().to_vec(), FrameKind::First),
        (encode56(&frame(Command::Up)).unwrap().to_vec(), FrameKind::Repeat),
        (encode80(&frame(Command::Up), 0).to_vec(), FrameKind::First),
        (encode80(&frame(Command::Up), 1).to_vec(), FrameKind::Repeat),
    ];
    for (bytes, kind) in cases {
        let merged = merged_for(&bytes, kind);
        let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
        pack(&merged, &mut out).unwrap_or_else(|e| panic!("{kind:?} {} bytes: {e:?}", bytes.len()));
    }
}

/// Worst case for symbol count: a payload where no adjacent halves merge.
#[test]
fn worst_case_80bit_payload_fits() {
    let merged = merged_for(&[0xFFu8; 10], FrameKind::First);
    let mut out: Vec<RmtSymbol, MAX_SYMBOLS> = Vec::new();
    pack(&merged, &mut out).expect("worst-case 80-bit frame must fit MAX_SYMBOLS");
    assert!(out.len() <= MAX_SYMBOLS, "needed {}", out.len());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p somfy-rmt`
Expected: FAIL — crate has no `lib.rs` contents yet.

- [ ] **Step 4: Implement**

`crates/somfy-rmt/src/lib.rs`:

```rust
//! # somfy-rmt
//!
//! Packs Somfy OOK pulse trains into ESP32 RMT symbols.
//!
//! The RMT peripheral stores **two** (level, duration) pairs per 32-bit symbol,
//! each duration a 15-bit tick count. This crate performs that packing as pure
//! data so it is testable on the host: `somfy-rts` must stay free of hardware
//! types, and the `firmware` crate cannot be compiled for the host at all.
//! The firmware's only job is mapping [`RmtSymbol`] onto `esp_hal::rmt::PulseCode`.
//!
//! Ticks are 1 µs (80 MHz RMT source clock with `clk_divider = 80`).

#![cfg_attr(not(test), no_std)]

use heapless::Vec;
use somfy_rts::Pulse;

/// Tick period in microseconds. 80 MHz / 80 = 1 MHz.
pub const TICK_US: u32 = 1;

/// Maximum ticks in one RMT duration field (15 bits).
pub const MAX_TICKS: u32 = 32_767;

/// Upper bound on symbols for any single Somfy frame.
///
/// Worst case is an 80-bit first frame with a payload where no adjacent
/// Manchester halves merge: 2 wake-up + 24 hardware-sync + 2 software-sync +
/// 160 data = 188 pulses = 94 symbols. Rounded to 96 for headroom.
pub const MAX_SYMBOLS: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RmtSymbol {
    pub level1: bool,
    pub length1: u16,
    pub level2: bool,
    pub length2: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// A single pulse exceeds the 15-bit RMT duration field.
    TooLong { micros: u32 },
    /// The frame needs more symbols than [`MAX_SYMBOLS`].
    TooManySymbols { needed: usize },
}

/// Pack merged, edge-to-edge pulses into RMT symbols.
///
/// Input MUST already be merged (see `somfy_rts::merge_pulses`); packing
/// unmerged half-symbols wastes half the symbol budget and can overflow.
///
/// An odd pulse count leaves the trailing half zero-length, which is RMT's
/// end-of-transmission marker — the desired behaviour, not padding.
pub fn pack(merged: &[Pulse], out: &mut Vec<RmtSymbol, MAX_SYMBOLS>) -> Result<(), PackError> {
    out.clear();
    let needed = merged.len().div_ceil(2);
    if needed > MAX_SYMBOLS {
        return Err(PackError::TooManySymbols { needed });
    }
    for p in merged {
        if p.micros > MAX_TICKS {
            return Err(PackError::TooLong { micros: p.micros });
        }
    }
    for chunk in merged.chunks(2) {
        let first = chunk[0];
        let second = chunk.get(1).copied();
        out.push(RmtSymbol {
            level1: first.high,
            length1: (first.micros / TICK_US) as u16,
            level2: second.map(|p| p.high).unwrap_or(false),
            length2: second.map(|p| (p.micros / TICK_US) as u16).unwrap_or(0),
        })
        .map_err(|_| PackError::TooManySymbols { needed })?;
    }
    Ok(())
}

/// Compile-time guard: the longest Somfy pulse must fit the RMT length field.
/// If the timing model ever grows past this, the build fails here rather than
/// silently truncating a pulse on air.
const _: () = assert!(somfy_rts::TIMINGS::INTER_FRAME_GAP <= MAX_TICKS);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p somfy-rmt`
Expected: PASS. If `worst_case_80bit_payload_fits` reports a count above 96, **do not raise `MAX_SYMBOLS` silently** — record the real number in the design spec §5.2 and reconsider the two-memory-block assumption.

- [ ] **Step 6: Verify the whole workspace and no_std**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p somfy-rmt --target thumbv7em-none-eabihf
```

Add `somfy-rmt` to the `no_std check` step in `.github/workflows/ci.yml` alongside the other four crates.

- [ ] **Step 7: Commit**

```bash
git add crates/somfy-rmt/ Cargo.toml .github/workflows/ci.yml
git commit -m "feat: somfy-rmt crate packing pulse trains into RMT symbols"
```

---

### Task 4: Firmware crate skeleton and CI matrix

**Files:**
- Create: `crates/firmware/Cargo.toml`, `crates/firmware/rust-toolchain.toml`, `crates/firmware/.cargo/config.toml`, `crates/firmware/src/main.rs`, `crates/firmware/src/chip.rs`
- Modify: `Cargo.toml` (root — exclude), `.github/workflows/ci.yml`

**Interfaces:**
- Produces: `mod chip` exposing `pub const RMT_CLOCK_MHZ: u32`, `pub const SCK: u8`, `MOSI`, `MISO`, `CSN`, `GDO0_TX`, `GDO2_RX`.

- [ ] **Step 1: Exclude firmware from the root workspace**

In the root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/somfy-rts", "crates/somfy-domain", "crates/somfy-api", "crates/somfy-migrate", "crates/somfy-rmt"]
exclude = ["crates/firmware"]
```

This is what keeps `cargo test --workspace` working with no ESP toolchain installed.

- [ ] **Step 2: Create the firmware manifest and toolchain**

`crates/firmware/Cargo.toml`:

```toml
[package]
name = "firmware"
version = "0.1.0"
edition = "2021"
license = "GPL-3.0-only"
description = "somfy-rs ESP32 firmware"

[dependencies]
esp-hal = { version = "1.1.1", features = ["unstable"] }
esp-println = { version = "0.13", features = ["log"] }
heapless = "0.8"
embedded-hal = "1.0"
somfy-rts = { path = "../somfy-rts" }
somfy-rmt = { path = "../somfy-rmt" }

[dev-dependencies]
embedded-hal-mock = { version = "0.11", features = ["eh1"] }

[features]
default = []
chip-esp32 = ["esp-hal/esp32", "esp-println/esp32"]
chip-s2 = ["esp-hal/esp32s2", "esp-println/esp32s2"]
chip-s3 = ["esp-hal/esp32s3", "esp-println/esp32s3"]
chip-c3 = ["esp-hal/esp32c3", "esp-println/esp32c3"]

[profile.release]
opt-level = "s"
lto = true
codegen-units = 1
```

`crates/firmware/rust-toolchain.toml`:

```toml
# espup's `esp` channel carries the Xtensa targets (ESP32/S2/S3) and the
# RISC-V ones (C3). The repo root stays on stable for the host crates.
[toolchain]
channel = "esp"
```

`crates/firmware/.cargo/config.toml`:

```toml
[unstable]
build-std = ["core"]

[env]
ESP_LOG = "info"
```

- [ ] **Step 3: Write the chip module with the no-default guard**

`crates/firmware/src/chip.rs`:

```rust
//! Per-chip constants. Exactly one `chip-*` feature must be enabled; esp-hal's
//! own chip features are mutually exclusive, so "all four chips" means four
//! separate builds, never one.

#[cfg(not(any(
    feature = "chip-esp32",
    feature = "chip-s2",
    feature = "chip-s3",
    feature = "chip-c3"
)))]
compile_error!(
    "no chip selected: build with exactly one of \
     --features chip-esp32 | chip-s2 | chip-s3 | chip-c3"
);

/// RMT source clock. **Must** be 80 MHz on ESP32 and ESP32-S2 (esp-hal
/// constraint); the others are configured the same for one tick model.
pub const RMT_CLOCK_MHZ: u32 = 80;

/// Divider giving 1 µs ticks from `RMT_CLOCK_MHZ`.
pub const RMT_CLK_DIVIDER: u8 = 80;

// Pin map verified against the reference production device's live
// /controller response on 2026-08-15.
#[cfg(feature = "chip-s3")]
pub mod pins {
    pub const SCK: u8 = 12;
    pub const MOSI: u8 = 11;
    pub const MISO: u8 = 13;
    pub const CSN: u8 = 10;
    /// CC1101 GDO0 — TX data in. NOTE: GPIO3 is an S3 strapping pin
    /// (JTAG source select); proven in production but the first suspect
    /// for any boot anomaly.
    pub const GDO0_TX: u8 = 3;
    /// CC1101 GDO2 — RX data out.
    pub const GDO2_RX: u8 = 4;
}

// Defaults for the other chips follow the C++ reference's per-chip defaults
// (Somfy.cpp:4886-4923). They are UNVERIFIED on hardware — the S3 map above is
// the only one confirmed against a working device.
#[cfg(feature = "chip-esp32")]
pub mod pins {
    pub const SCK: u8 = 18;
    pub const MOSI: u8 = 23;
    pub const MISO: u8 = 19;
    pub const CSN: u8 = 5;
    pub const GDO0_TX: u8 = 13;
    pub const GDO2_RX: u8 = 12;
}

#[cfg(feature = "chip-s2")]
pub mod pins {
    pub const SCK: u8 = 36;
    pub const MOSI: u8 = 35;
    pub const MISO: u8 = 37;
    pub const CSN: u8 = 34;
    pub const GDO0_TX: u8 = 15;
    pub const GDO2_RX: u8 = 14;
}

#[cfg(feature = "chip-c3")]
pub mod pins {
    pub const SCK: u8 = 15;
    pub const MOSI: u8 = 16;
    pub const MISO: u8 = 17;
    pub const CSN: u8 = 14;
    pub const GDO0_TX: u8 = 13;
    pub const GDO2_RX: u8 = 12;
}
```

- [ ] **Step 4: Minimal main that proves the toolchain**

`crates/firmware/src/main.rs`:

```rust
#![no_std]
#![no_main]

mod chip;

use esp_hal::main;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    loop {}
}

#[main]
fn entry() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    esp_println::println!(
        "somfy-rs firmware: RMT {} MHz / div {} -> 1us ticks; CSN={} GDO0={} GDO2={}",
        chip::RMT_CLOCK_MHZ,
        chip::RMT_CLK_DIVIDER,
        chip::pins::CSN,
        chip::pins::GDO0_TX,
        chip::pins::GDO2_RX,
    );
    loop {}
}
```

- [ ] **Step 5: Verify each chip builds**

Install the toolchain once: `cargo install espup && espup install`.

```bash
cd crates/firmware
cargo build --features chip-s3   --target xtensa-esp32s3-none-elf
cargo build --features chip-esp32 --target xtensa-esp32-none-elf
cargo build --features chip-s2   --target xtensa-esp32s2-none-elf
cargo build --features chip-c3   --target riscv32imc-unknown-none-elf
cargo build   # expected: FAILS with the compile_error! naming the four options
```

Expected: four successes, one deliberate failure. `esp-hal` 1.1.x APIs move between minor versions — if `esp_hal::init` or the `#[main]` attribute differ, **check the docs for the pinned version** rather than pinning an older esp-hal.

- [ ] **Step 6: Add the CI matrix**

In `.github/workflows/ci.yml`, add a second job:

```yaml
  firmware:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        include:
          - feature: chip-esp32
            target: xtensa-esp32-none-elf
          - feature: chip-s2
            target: xtensa-esp32s2-none-elf
          - feature: chip-s3
            target: xtensa-esp32s3-none-elf
          - feature: chip-c3
            target: riscv32imc-unknown-none-elf
    steps:
      - uses: actions/checkout@v4
      - uses: esp-rs/xtensa-toolchain@v1.5
        with:
          default: true
          buildtargets: esp32,esp32s2,esp32s3
          ldproxy: false
      - name: clippy (${{ matrix.feature }})
        working-directory: crates/firmware
        run: cargo clippy --features ${{ matrix.feature }} --target ${{ matrix.target }} --all-targets -- -D warnings
      - name: build (${{ matrix.feature }})
        working-directory: crates/firmware
        run: cargo build --features ${{ matrix.feature }} --target ${{ matrix.target }} --release
```

- [ ] **Step 7: Commit**

```bash
git add crates/firmware/ Cargo.toml .github/workflows/ci.yml
git commit -m "feat: firmware crate skeleton with four-chip clippy+build matrix"
```

---

### Task 5: CC1101 driver

**Files:**
- Create: `crates/firmware/src/radio/mod.rs`, `crates/firmware/src/radio/cc1101.rs`
- Test: `crates/firmware/src/radio/cc1101.rs` (`#[cfg(test)]` module, `embedded-hal-mock`)

**Interfaces:**
- Consumes: `embedded_hal::spi::SpiDevice`
- Produces:
  - `pub struct Cc1101<SPI> { spi: SPI }`
  - `pub fn new(spi: SPI) -> Self`
  - `pub fn init(&mut self) -> Result<(), Cc1101Error>` — reset, verify PARTNUM/VERSION, write the OOK async-serial register set
  - `pub fn set_tx(&mut self) -> Result<(), Cc1101Error>` / `pub fn set_idle(&mut self)`
  - `pub enum Cc1101Error { Spi, BadVersion(u8) }`

Register values must be taken from the reference configuration the C++ firmware applies (`Somfy.cpp:4983-5020`): 433.42 MHz, deviation 47.60 kHz, RX bandwidth 99.97 kHz, TX power 10, `setModulation(2)` = ASK/OOK, `setPktFormat(3)` = asynchronous serial, `setCrc(0)`, `setSyncMode(4)`, `setAdrChk(0)`.

- [ ] **Step 1: Write the failing test**

In `crates/firmware/src/radio/cc1101.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal_mock::eh1::spi::{Mock as SpiMock, Transaction as SpiTransaction};

    /// VERSION (0x31) is a burst-read status register: address | 0xC0.
    #[test]
    fn init_rejects_an_unexpected_version() {
        let expectations = [
            SpiTransaction::transaction_start(),
            SpiTransaction::transfer_in_place(vec![0xF1, 0x00], vec![0x00, 0x99]),
            SpiTransaction::transaction_end(),
        ];
        let mut spi = SpiMock::new(&expectations);
        let mut radio = Cc1101::new(&mut spi);
        assert!(matches!(radio.read_version(), Ok(0x99)));
        spi.done();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/firmware && cargo test --features chip-s3 --lib`
Expected: FAIL — `Cc1101` not defined.

Note: host `cargo test` on this crate only compiles the pure-logic modules. If the crate cannot build for the host at all because `main.rs` pulls in `esp-hal`, move the driver behind `src/lib.rs` with `#![cfg_attr(not(test), no_std)]` and have `main.rs` depend on the lib — this keeps the driver host-testable, which is the whole point.

- [ ] **Step 3: Implement**

```rust
//! Minimal CC1101 driver: only the registers this project actually uses.
//!
//! Configured for asynchronous serial mode — the CC1101 is a dumb OOK modem
//! and the ESP supplies the raw bitstream on GDO0. Register values mirror the
//! reference firmware's setup (Somfy.cpp:4983-5020).

use embedded_hal::spi::{Operation, SpiDevice};

const READ_BURST: u8 = 0xC0;
const REG_VERSION: u8 = 0x31;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cc1101Error {
    Spi,
    BadVersion(u8),
}

pub struct Cc1101<SPI> {
    spi: SPI,
}

impl<SPI: SpiDevice> Cc1101<SPI> {
    pub fn new(spi: SPI) -> Self {
        Self { spi }
    }

    pub fn read_version(&mut self) -> Result<u8, Cc1101Error> {
        let mut buf = [REG_VERSION | READ_BURST, 0x00];
        self.spi
            .transaction(&mut [Operation::TransferInPlace(&mut buf)])
            .map_err(|_| Cc1101Error::Spi)?;
        Ok(buf[1])
    }

    pub fn write_register(&mut self, addr: u8, value: u8) -> Result<(), Cc1101Error> {
        self.spi
            .transaction(&mut [Operation::Write(&[addr, value])])
            .map_err(|_| Cc1101Error::Spi)
    }
}
```

Then add `init()`. It must, in order: strobe SRES (0x30) and wait for the chip
to settle, read PARTNUM (0x30) and VERSION (0x31) and fail with
`Cc1101Error::BadVersion` on an unexpected value, then write these registers:

| Register | Addr | Controls |
|---|---|---|
| `IOCFG2` | 0x00 | GDO2 = serial data out (RX) |
| `IOCFG0` | 0x02 | GDO0 = serial data in (TX) |
| `FREQ2/1/0` | 0x0D–0x0F | 433.42 MHz carrier |
| `MDMCFG4/3` | 0x10–0x11 | data rate + RX bandwidth 99.97 kHz |
| `MDMCFG2` | 0x12 | modulation ASK/OOK, sync mode 4 |
| `DEVIATN` | 0x15 | deviation 47.60 kHz |
| `PKTCTRL0` | 0x08 | packet format 3 (async serial), CRC off |
| `ADDR`/`PKTCTRL1` | 0x09/0x07 | address check off |
| `FREND0`/`PATABLE` | 0x22/0x3E | TX power 10 dBm, OOK power table |

**Derive every numeric value from the CC1101 datasheet formulas for the
parameters above and put the derivation in a comment next to it** — e.g. the
FREQ registers come from `f_carrier = (f_xosc / 2^16) * FREQ` with a 26 MHz
crystal. Do not copy magic numbers from another driver without showing where
they came from; this project's rule is that unexplained constants are treated
as fabricated. Cross-check the resulting behaviour against the reference
firmware's settings (`Somfy.cpp:4990-5020`), which is the known-working
configuration for these exact modules.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crates/firmware && cargo test --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/firmware/src/radio/
git commit -m "feat: minimal CC1101 SPI driver for OOK async-serial mode"
```

---

### Task 6: RMT TX

**Files:**
- Create: `crates/firmware/src/radio/rmt_tx.rs`
- Modify: `crates/firmware/src/main.rs`

**Interfaces:**
- Consumes: `somfy_rmt::{RmtSymbol, PackError, pack, MAX_SYMBOLS}`, `somfy_rts::{merge_pulses, render_pulses, encode56, encode80, FrameKind, Pulse}`
- Produces:
  - `pub enum TxError { Pack(PackError), Rmt }` with `impl From<PackError> for TxError`
  - `pub fn build_symbols(bytes: &[u8], kind: FrameKind, out: &mut heapless::Vec<RmtSymbol, MAX_SYMBOLS>) -> Result<(), TxError>` — pure, host-testable
  - `pub fn to_pulse_code(sym: RmtSymbol) -> esp_hal::rmt::PulseCode`
  - `pub struct RmtTx<CH>` with `pub fn new(channel: CH) -> Self` and `pub fn transmit_frame(&mut self, bytes: &[u8], kind: FrameKind) -> Result<(), TxError>`

- [ ] **Step 1: Write the conversion test**

`to_pulse_code` is pure and host-testable if `PulseCode` construction is; if it is not host-constructible, test the field mapping through a local mirror struct and assert the four fields. Either way the test must pin: `level1/length1` come from the first pulse, `level2/length2` from the second, and a zero `length2` survives as zero.

- [ ] **Step 2: Implement the transmit path**

```rust
//! Clocks packed RMT symbols out to the CC1101's GDO0 pin.
//!
//! 1 µs ticks: RMT source 80 MHz with clk_divider 80 (chip::RMT_CLK_DIVIDER).
//! One transaction per frame; repeats are separate transactions because the
//! reference re-encodes the 80-bit tail per repeat (Somfy.cpp:263-331) and the
//! 56-bit form carries its inter-frame gap inside its own pulse train.

use heapless::Vec;
use somfy_rmt::{pack, PackError, RmtSymbol, MAX_SYMBOLS};
use somfy_rts::{merge_pulses, render_pulses, FrameKind, Pulse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxError {
    /// The frame could not be packed into RMT symbols.
    Pack(PackError),
    /// The RMT peripheral rejected or aborted the transmission.
    Rmt,
}

impl From<PackError> for TxError {
    fn from(e: PackError) -> Self {
        TxError::Pack(e)
    }
}

/// Render, merge and pack one frame into RMT symbols plus a terminating
/// zero-length entry.
///
/// Split out from the transmit call so it is testable without a channel: this
/// half is pure data and carries all the logic worth asserting on.
pub fn build_symbols(
    bytes: &[u8],
    kind: FrameKind,
    out: &mut Vec<RmtSymbol, MAX_SYMBOLS>,
) -> Result<(), TxError> {
    let mut rendered: Vec<Pulse, 320> = Vec::new();
    render_pulses(bytes, kind, &mut rendered);

    let mut merged: Vec<Pulse, 320> = Vec::new();
    merge_pulses(&rendered, &mut merged);

    pack(&merged, out)?;

    // RMT stops on a zero-length entry. `pack` already leaves one when the
    // merged pulse count is odd; when it is even we must append an explicit
    // terminator or the peripheral runs past the end of the frame.
    if merged.len() % 2 == 0 {
        out.push(RmtSymbol { level1: false, length1: 0, level2: false, length2: 0 })
            .map_err(|_| TxError::Pack(PackError::TooManySymbols { needed: out.len() + 1 }))?;
    }
    Ok(())
}
```

The channel-facing half then holds the configured channel and does nothing but
convert and send:

```rust
pub struct RmtTx<CH> {
    channel: CH,
}

impl<CH> RmtTx<CH> {
    pub fn new(channel: CH) -> Self {
        Self { channel }
    }
}
```

`transmit_frame` calls `build_symbols`, maps each `RmtSymbol` through
`to_pulse_code`, and hands the slice to the channel's transmit call, taking the
channel by value and returning it as esp-hal's transaction API requires (see
the RX example in the esp-hal 1.1.1 `rmt` module docs, where `transaction.wait()`
returns `(count, channel)`). Because `MAX_SYMBOLS` bounds the buffer, the whole
conversion is a single `heapless::Vec` with no allocation.

Configure the channel per esp-hal 1.1.x:

```rust
let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(chip::RMT_CLOCK_MHZ))?;
let mut channel = rmt
    .channel0
    .configure_tx(
        &TxChannelConfig::default()
            .with_clk_divider(chip::RMT_CLK_DIVIDER)
            .with_idle_output_level(Level::Low)
            .with_idle_output(true)
            .with_carrier_modulation(false),
    )?
    .with_pin(gdo0_pin);
```

- [ ] **Step 3: Measure the memory-block requirement**

Log `symbols.len()` for a worst-case 80-bit first frame on hardware and compare against the channel's memory-block capacity. Design spec §5.2 predicts 94 symbols against 48 per block on the S3, i.e. **two blocks with almost no headroom**. Record the measured number in the spec. If two blocks are insufficient, switch to RMT wrap-around refill before proceeding — do not shrink the frame.

- [ ] **Step 4: Bring-up binary**

Make `main.rs` initialise the CC1101, build an `Up` frame for a **test address that is not one of the user's real shades**, and transmit first-frame plus repeats on a button press or a fixed delay, logging each step over `esp-println`.

- [ ] **Step 5: Commit**

```bash
git add crates/firmware/src/
git commit -m "feat: RMT TX path rendering Somfy frames to the CC1101"
```

---

### Task 7: On-air validation

**Files:**
- Create: `docs/hardware-checklist.md`

- [ ] **Step 1: Verify against a receiver before touching a motor**

Put a second ESPSomfy-RTS device in listening mode (websocket room `join:0` on port 8080) and transmit from the Rust firmware. Assert the received `remoteFrame` reports the expected `address`, `command`, `bits == 56`, and `sync == 4` for a first frame / `14` for a repeat. **This is the check that catches a wrong pulse train before a motor ever hears it.**

- [ ] **Step 2: Compare against the C++ transmitter**

Capture the C++ firmware transmitting the same command and diff the `pulses[]` arrays against the Rust output. They should agree within the receiver's timing tolerance.

- [ ] **Step 3: Drive a real motor**

Only after steps 1-2 pass. Use a shade the user has designated for testing.

- [ ] **Step 4: Write the checklist and commit**

Record the procedure, the measured symbol count, and the observed RSSI in `docs/hardware-checklist.md`.

```bash
git add docs/hardware-checklist.md
git commit -m "docs: hardware bring-up checklist for the TX path"
```

---

## Not in this plan (Plan 4b)

RMT RX and the `PulseSource` trait, the Embassy radio and state tasks, the bounded `TransmitRequest` channel, the `RollingCodeStore` trait and its flash implementation, and the `transmit()` ordering helper that enforces persist-before-TX. Those complete design spec §6, §7 and the second half of §12.
