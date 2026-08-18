# somfy-rs Plan 1: Workspace Bootstrap + `somfy-rts` Protocol Engine

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `somfy-rs` Cargo workspace and a complete, host-tested `no_std` Somfy RTS protocol engine crate (frames, rolling codes, pulse rendering, RX decoding).

**Architecture:** Pure-logic `no_std` crate with zero hardware knowledge. Frames encode/decode to bytes; a pulse layer renders bytes to OOK level/duration pairs and decodes duration streams back to frames. A software TX→RX loopback property test proves internal consistency; golden captures from the C++ firmware prove real-world compatibility.

**Tech Stack:** Rust stable (host tests) + `no_std` lib, `heapless`, `proptest` (dev-dependency only).

**Plan series:** This is Plan 1 of 7 (spec §: `docs/superpowers/specs/2026-07-15-rust-rewrite-design.md` in the ESPSomfy-RTS repo). Later plans: domain, api/migrate, firmware radio, network, persistence/OTA, UI.

## Global Constraints

- New repository at `/home/dhia/Sources/personal/somfy-rs` (sibling of the C++ reference repo at `/home/dhia/Sources/personal/ESPSomfy-RTS`).
- `crates/somfy-rts` is `#![no_std]`, no allocator. Only runtime dependency: `heapless = "0.8"`. Dev-dependencies (host only) may use std.
- All timing constants MUST be ported from the C++ reference `src/Somfy.cpp` — do not trust folklore values; when a constant below disagrees with `Somfy.cpp`, the C++ file wins and the plan value must be corrected in code review.
- Rolling-code semantics must match C++ exactly: the code transmitted is the current stored value; storage is incremented after building the frame (see `SomfyRemote::sendCommand` in `src/Somfy.cpp`).
- Every commit message: conventional commits (`feat:`, `test:`, `chore:`...). No attribution footers.
- All tasks run on the host: `cargo test -p somfy-rts` must stay green; no ESP toolchain needed in this plan.

---

### Task 1: Workspace bootstrap

**Files:**
- Create: `/home/dhia/Sources/personal/somfy-rs/Cargo.toml`
- Create: `/home/dhia/Sources/personal/somfy-rs/crates/somfy-rts/Cargo.toml`
- Create: `/home/dhia/Sources/personal/somfy-rs/crates/somfy-rts/src/lib.rs`
- Create: `/home/dhia/Sources/personal/somfy-rs/rust-toolchain.toml`
- Create: `/home/dhia/Sources/personal/somfy-rs/.gitignore`
- Create: `/home/dhia/Sources/personal/somfy-rs/docs/` (copy spec + this plan in)

**Interfaces:**
- Consumes: nothing.
- Produces: a git repo where `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` pass.

- [ ] **Step 1: Create repo and workspace layout**

```bash
mkdir -p /home/dhia/Sources/personal/somfy-rs/crates/somfy-rts/src
cd /home/dhia/Sources/personal/somfy-rs
git init -b main
mkdir -p docs/specs docs/plans
cp /home/dhia/Sources/personal/ESPSomfy-RTS/docs/superpowers/specs/2026-07-15-rust-rewrite-design.md docs/specs/
cp /home/dhia/Sources/personal/ESPSomfy-RTS/docs/superpowers/plans/2026-07-15-somfy-rs-plan1-protocol-engine.md docs/plans/
```

- [ ] **Step 2: Write workspace files**

`Cargo.toml` (workspace root):

```toml
[workspace]
resolver = "2"
members = ["crates/somfy-rts"]

[workspace.package]
edition = "2021"
license = "GPL-3.0-only"
repository = "https://github.com/dhia-gharsallaoui/somfy-rs"
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`.gitignore`:

```
/target
```

`crates/somfy-rts/Cargo.toml`:

```toml
[package]
name = "somfy-rts"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
description = "no_std Somfy RTS protocol engine: frames, rolling codes, OOK pulse rendering and decoding"

[dependencies]
heapless = "0.8"

[dev-dependencies]
proptest = "1"
```

`crates/somfy-rts/src/lib.rs`:

```rust
#![cfg_attr(not(test), no_std)]
```

- [ ] **Step 3: Verify build and lints**

Run: `cd /home/dhia/Sources/personal/somfy-rs && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: all pass (0 tests).

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "chore: bootstrap somfy-rs workspace with somfy-rts crate"
```

---

### Task 2: Command enum + 56-bit frame encode/decode

**Files:**
- Create: `crates/somfy-rts/src/command.rs`
- Create: `crates/somfy-rts/src/frame.rs`
- Modify: `crates/somfy-rts/src/lib.rs`
- Test: `crates/somfy-rts/tests/frame56.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub enum Command` with `pub fn nibble(self) -> u8`, `pub fn from_nibble(n: u8) -> Option<Command>`, `pub fn is_extended(self) -> bool` (true for `StepUp`, `Favorite`, `Stop`).
  - `pub struct Frame { pub key: u8, pub command: Command, pub rolling_code: u16, pub address: u32 }`
  - `pub fn encode56(f: &Frame) -> [u8; 7]`
  - `pub fn decode56(bytes: &[u8; 7]) -> Result<Frame, FrameError>`
  - `pub enum FrameError { BadChecksum, UnknownCommand }`

Reference for the algorithm: C++ `src/Somfy.cpp` (search `encodeFrame` / checksum loop) and the shade command enum in `src/Somfy.h:31-52`.

- [ ] **Step 1: Write failing tests**

`crates/somfy-rts/tests/frame56.rs`:

```rust
use somfy_rts::{decode56, encode56, Command, Frame, FrameError};

fn sample() -> Frame {
    Frame { key: 0xA7, command: Command::Up, rolling_code: 42, address: 0x27_96_20 }
}

#[test]
fn roundtrip_56() {
    let f = sample();
    let bytes = encode56(&f);
    let back = decode56(&bytes).unwrap();
    assert_eq!(back.command, Command::Up);
    assert_eq!(back.rolling_code, 42);
    assert_eq!(back.address, 0x27_96_20);
}

#[test]
fn checksum_nibbles_xor_to_zero_before_obfuscation() {
    // encode56 obfuscates; deobfuscate manually and check the RTS invariant:
    // XOR of all 14 nibbles == 0.
    let bytes = encode56(&sample());
    let mut clear = bytes;
    for i in (1..7).rev() {
        clear[i] ^= clear[i - 1];
    }
    let x = clear.iter().fold(0u8, |acc, b| acc ^ (b >> 4) ^ (b & 0x0F));
    assert_eq!(x & 0x0F, 0);
}

#[test]
fn corrupted_frame_rejected() {
    let mut bytes = encode56(&sample());
    bytes[3] ^= 0xFF;
    assert!(matches!(decode56(&bytes), Err(FrameError::BadChecksum)));
}

#[test]
fn command_nibble_mapping_matches_cpp_enum() {
    // Values from ESPSomfy-RTS src/Somfy.h:31-52
    assert_eq!(Command::My.nibble(), 0x1);
    assert_eq!(Command::Up.nibble(), 0x2);
    assert_eq!(Command::Down.nibble(), 0x4);
    assert_eq!(Command::Prog.nibble(), 0x8);
    assert_eq!(Command::StepDown.nibble(), 0xB);
    assert!(Command::StepUp.is_extended());
    assert!(Command::Favorite.is_extended());
    assert!(Command::Stop.is_extended());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test frame56`
Expected: FAIL to compile — `encode56` etc. not found.

- [ ] **Step 3: Implement**

`crates/somfy-rts/src/command.rs`:

```rust
/// Somfy commands. Discriminants mirror ESPSomfy-RTS src/Somfy.h:31-52.
/// Values > 0xF are "extended" commands that require 80-bit frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    My = 0x1,
    Up = 0x2,
    MyUp = 0x3,
    Down = 0x4,
    MyDown = 0x5,
    UpDown = 0x6,
    MyUpDown = 0x7,
    Prog = 0x8,
    SunFlag = 0x9,
    Flag = 0xA,
    StepDown = 0xB,
    Toggle = 0xC,
    Sensor = 0xE,
    RtwProto = 0xF,
    StepUp = 0x8B,
    Favorite = 0xC1,
    Stop = 0xF1,
}

impl Command {
    /// Low nibble placed in the 56-bit frame command field.
    pub fn nibble(self) -> u8 {
        (self as u8) & 0x0F
    }

    pub fn is_extended(self) -> bool {
        (self as u8) > 0x0F
    }

    pub fn from_nibble(n: u8) -> Option<Command> {
        Some(match n & 0x0F {
            0x1 => Command::My,
            0x2 => Command::Up,
            0x3 => Command::MyUp,
            0x4 => Command::Down,
            0x5 => Command::MyDown,
            0x6 => Command::UpDown,
            0x7 => Command::MyUpDown,
            0x8 => Command::Prog,
            0x9 => Command::SunFlag,
            0xA => Command::Flag,
            0xB => Command::StepDown,
            0xC => Command::Toggle,
            0xE => Command::Sensor,
            0xF => Command::RtwProto,
            _ => return None,
        })
    }
}
```

`crates/somfy-rts/src/frame.rs`:

```rust
use crate::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// "Encryption key" byte; C++ uses 0xA0 | low counter nibble.
    pub key: u8,
    pub command: Command,
    pub rolling_code: u16,
    /// 24-bit remote address.
    pub address: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    BadChecksum,
    UnknownCommand,
}

/// Layout before obfuscation (matches ESPSomfy-RTS src/Somfy.cpp):
/// [0]=key [1]=cmd<<4|cksum [2..3]=rolling code BE [4..6]=address LSB-first.
pub fn encode56(f: &Frame) -> [u8; 7] {
    let mut b = [0u8; 7];
    b[0] = f.key;
    b[1] = f.command.nibble() << 4;
    b[2] = (f.rolling_code >> 8) as u8;
    b[3] = f.rolling_code as u8;
    b[4] = f.address as u8;
    b[5] = (f.address >> 8) as u8;
    b[6] = (f.address >> 16) as u8;

    b[1] |= checksum(&b);
    obfuscate(&mut b);
    b
}

pub fn decode56(bytes: &[u8; 7]) -> Result<Frame, FrameError> {
    let mut b = *bytes;
    deobfuscate(&mut b);
    if checksum(&b) != 0 {
        return Err(FrameError::BadChecksum);
    }
    let command = Command::from_nibble(b[1] >> 4).ok_or(FrameError::UnknownCommand)?;
    Ok(Frame {
        key: b[0],
        command,
        rolling_code: ((b[2] as u16) << 8) | b[3] as u16,
        address: (b[4] as u32) | ((b[5] as u32) << 8) | ((b[6] as u32) << 16),
    })
}

/// XOR of all nibbles; a valid frame's nibbles XOR to 0.
fn checksum(b: &[u8; 7]) -> u8 {
    b.iter().fold(0u8, |acc, x| acc ^ (x >> 4) ^ (x & 0x0F)) & 0x0F
}

fn obfuscate(b: &mut [u8; 7]) {
    for i in 1..7 {
        b[i] ^= b[i - 1];
    }
}

fn deobfuscate(b: &mut [u8; 7]) {
    for i in (1..7).rev() {
        b[i] ^= b[i - 1];
    }
}
```

`crates/somfy-rts/src/lib.rs`:

```rust
#![cfg_attr(not(test), no_std)]

mod command;
mod frame;

pub use command::Command;
pub use frame::{decode56, encode56, Frame, FrameError};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p somfy-rts --test frame56`
Expected: 4 passed.

- [ ] **Step 5: Cross-check byte layout against C++ and commit**

Open `/home/dhia/Sources/personal/ESPSomfy-RTS/src/Somfy.cpp`, find the 56-bit frame build (search for `frame[0]` assignments and the checksum loop). Confirm: key byte format, command nibble position, rolling-code byte order, address byte order, obfuscation direction. If any differ from the implementation above, fix the Rust code and re-run tests.

```bash
git add -A && git commit -m "feat: 56-bit RTS frame encode/decode with checksum and obfuscation"
```

---

### Task 3: Property tests for frame layer

**Files:**
- Test: `crates/somfy-rts/tests/frame_props.rs`

**Interfaces:**
- Consumes: `encode56`, `decode56`, `Frame`, `Command` from Task 2.
- Produces: confidence; no new API.

- [ ] **Step 1: Write property tests**

`crates/somfy-rts/tests/frame_props.rs`:

```rust
use proptest::prelude::*;
use somfy_rts::{decode56, encode56, Command, Frame};

fn any_basic_command() -> impl Strategy<Value = Command> {
    prop::sample::select(alloc_cmds())
}

fn alloc_cmds() -> Vec<Command> {
    vec![
        Command::My, Command::Up, Command::MyUp, Command::Down, Command::MyDown,
        Command::UpDown, Command::MyUpDown, Command::Prog, Command::SunFlag,
        Command::Flag, Command::StepDown, Command::Toggle, Command::Sensor,
        Command::RtwProto,
    ]
}

proptest! {
    #[test]
    fn encode_decode_roundtrip(
        key_low in 0u8..=0x0F,
        cmd in any_basic_command(),
        code in any::<u16>(),
        addr in 0u32..0x0100_0000,
    ) {
        let f = Frame { key: 0xA0 | key_low, command: cmd, rolling_code: code, address: addr };
        let back = decode56(&encode56(&f)).unwrap();
        prop_assert_eq!(back, f);
    }

    #[test]
    fn single_bit_corruption_never_decodes_silently_to_other_fields(
        code in any::<u16>(),
        addr in 0u32..0x0100_0000,
        byte_idx in 0usize..7,
        bit in 0u8..8,
    ) {
        let f = Frame { key: 0xA7, command: Command::Up, rolling_code: code, address: addr };
        let mut bytes = encode56(&f);
        bytes[byte_idx] ^= 1 << bit;
        // Either rejected, or (checksum is only 4 bits) decodes to *something* —
        // but never silently to the original frame with a different meaning.
        if let Ok(back) = decode56(&bytes) {
            prop_assert_ne!(back, f);
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p somfy-rts --test frame_props`
Expected: 2 passed (256 cases each).

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "test: property tests for 56-bit frame layer"
```

---

### Task 4: Rolling-code state machine

**Files:**
- Create: `crates/somfy-rts/src/rolling.rs`
- Modify: `crates/somfy-rts/src/lib.rs` (add `mod rolling; pub use rolling::RollingCode;`)
- Test: `crates/somfy-rts/tests/rolling.rs`

**Interfaces:**
- Consumes: `Frame`, `Command` from Task 2.
- Produces:
  - `pub struct RollingCode(pub u16);`
  - `impl RollingCode { pub fn next_frame(&mut self, command: Command, address: u32) -> Frame }` — builds a `Frame` with the **current** code, then increments (wrapping). Key byte = `0xA0 | (code as u8 & 0x0F)` matching C++.

- [ ] **Step 1: Write failing tests**

`crates/somfy-rts/tests/rolling.rs`:

```rust
use somfy_rts::{Command, RollingCode};

#[test]
fn transmits_current_code_then_increments() {
    let mut rc = RollingCode(41);
    let f1 = rc.next_frame(Command::Up, 0x123456);
    assert_eq!(f1.rolling_code, 41);
    assert_eq!(rc.0, 42);
    let f2 = rc.next_frame(Command::Down, 0x123456);
    assert_eq!(f2.rolling_code, 42);
    assert_eq!(rc.0, 43);
}

#[test]
fn wraps_at_u16_max() {
    let mut rc = RollingCode(u16::MAX);
    let f = rc.next_frame(Command::My, 1);
    assert_eq!(f.rolling_code, u16::MAX);
    assert_eq!(rc.0, 0);
}

#[test]
fn key_byte_is_0xa_high_nibble_and_code_low_nibble() {
    let mut rc = RollingCode(0x0102);
    let f = rc.next_frame(Command::Up, 1);
    assert_eq!(f.key, 0xA2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test rolling`
Expected: compile FAIL — `RollingCode` not found.

- [ ] **Step 3: Implement**

`crates/somfy-rts/src/rolling.rs`:

```rust
use crate::{Command, Frame};

/// Persisted per-shade rolling code. The transmitted frame carries the
/// current value; the store increments after building. The CALLER must
/// persist the incremented value BEFORE transmitting the frame
/// (spec §4 invariant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollingCode(pub u16);

impl RollingCode {
    pub fn next_frame(&mut self, command: Command, address: u32) -> Frame {
        let code = self.0;
        self.0 = self.0.wrapping_add(1);
        Frame {
            key: 0xA0 | (code as u8 & 0x0F),
            command,
            rolling_code: code,
            address,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p somfy-rts --test rolling`
Expected: 3 passed.

- [ ] **Step 5: Cross-check against C++ and commit**

Confirm in `/home/dhia/Sources/personal/ESPSomfy-RTS/src/Somfy.cpp` (`sendCommand` / frame build): key derivation and increment ordering. Fix if divergent.

```bash
git add -A && git commit -m "feat: rolling-code state machine with persist-before-tx contract"
```

---

### Task 5: Pulse timing constants + TX pulse rendering

**Files:**
- Create: `crates/somfy-rts/src/pulse.rs`
- Modify: `crates/somfy-rts/src/lib.rs` (add `mod pulse; pub use pulse::{render_pulses, FrameKind, Pulse, TIMINGS};`)
- Test: `crates/somfy-rts/tests/pulse_tx.rs`

**Interfaces:**
- Consumes: `encode56` from Task 2.
- Produces:
  - `pub struct Pulse { pub high: bool, pub micros: u32 }`
  - `pub enum FrameKind { First, Repeat }` — first frame uses wakeup + 2 hardware syncs; repeats use 7 hardware syncs and no wakeup (verify counts against `Somfy.cpp`).
  - `pub mod TIMINGS` constants (µs): `WAKEUP_HIGH = 9415`, `WAKEUP_LOW = 89_565`, `HW_SYNC_HALF = 2560`, `SW_SYNC_HIGH = 4550`, `HALF_SYMBOL = 604`, `INTER_FRAME_GAP = 30_415`. **Port the authoritative values from `src/Somfy.cpp` — these are the folklore values and must be verified in Step 5.**
  - `pub fn render_pulses(bytes: &[u8], kind: FrameKind, out: &mut heapless::Vec<Pulse, 320>)` — Manchester: bit `1` = low→high half-symbols, bit `0` = high→low (verify polarity against C++ TX code), MSB-first per byte.

- [ ] **Step 1: Write failing tests**

`crates/somfy-rts/tests/pulse_tx.rs`:

```rust
use heapless::Vec;
use somfy_rts::{encode56, render_pulses, Command, Frame, FrameKind, Pulse, TIMINGS};

fn bytes() -> [u8; 7] {
    encode56(&Frame { key: 0xA7, command: Command::Up, rolling_code: 7, address: 0xAABBCC })
}

#[test]
fn first_frame_starts_with_wakeup_then_two_hw_syncs() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::First, &mut out);
    assert!(out[0].high && out[0].micros == TIMINGS::WAKEUP_HIGH);
    assert!(!out[1].high && out[1].micros == TIMINGS::WAKEUP_LOW);
    // 2 hardware syncs = 4 half-pulses of HW_SYNC_HALF
    for p in &out[2..6] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(out[6].micros, TIMINGS::SW_SYNC_HIGH);
}

#[test]
fn repeat_frame_has_no_wakeup_and_seven_hw_syncs() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    for p in &out[0..14] {
        assert_eq!(p.micros, TIMINGS::HW_SYNC_HALF);
    }
    assert_eq!(out[14].micros, TIMINGS::SW_SYNC_HIGH);
}

#[test]
fn data_section_is_manchester_with_constant_energy() {
    // 56 data bits -> exactly 112 half-symbols of HALF_SYMBOL µs each,
    // adjacent same-level halves merged is NOT done at this layer.
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    let data: Vec<&Pulse, 320> = out
        .iter()
        .filter(|p| p.micros == TIMINGS::HALF_SYMBOL)
        .collect();
    // 112 half symbols + the SW-sync trailing 604µs low half
    assert_eq!(data.len(), 113);
    let highs = data.iter().filter(|p| p.high).count();
    assert_eq!(highs, 56); // Manchester: every bit contributes one high half
}

#[test]
fn frame_ends_with_inter_frame_gap() {
    let mut out: Vec<Pulse, 320> = Vec::new();
    render_pulses(&bytes(), FrameKind::Repeat, &mut out);
    let last = out.last().unwrap();
    assert!(!last.high && last.micros == TIMINGS::INTER_FRAME_GAP);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test pulse_tx`
Expected: compile FAIL.

- [ ] **Step 3: Implement**

`crates/somfy-rts/src/pulse.rs`:

```rust
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

/// Timing constants in µs. Authoritative source: ESPSomfy-RTS src/Somfy.cpp.
#[allow(non_snake_case)]
pub mod TIMINGS {
    pub const WAKEUP_HIGH: u32 = 9415;
    pub const WAKEUP_LOW: u32 = 89_565;
    pub const HW_SYNC_HALF: u32 = 2560;
    pub const SW_SYNC_HIGH: u32 = 4550;
    pub const HALF_SYMBOL: u32 = 604;
    pub const INTER_FRAME_GAP: u32 = 30_415;
}

/// Render an encoded frame (7 bytes for 56-bit, 10 for 80-bit) to pulses.
/// Manchester (verify polarity vs C++): bit 1 = low half then high half;
/// bit 0 = high half then low half. Bits sent MSB-first.
pub fn render_pulses(bytes: &[u8], kind: FrameKind, out: &mut Vec<Pulse, 320>) {
    let hw_syncs = match kind {
        FrameKind::First => {
            out.push(Pulse { high: true, micros: TIMINGS::WAKEUP_HIGH }).unwrap();
            out.push(Pulse { high: false, micros: TIMINGS::WAKEUP_LOW }).unwrap();
            2
        }
        FrameKind::Repeat => 7,
    };
    for _ in 0..hw_syncs {
        out.push(Pulse { high: true, micros: TIMINGS::HW_SYNC_HALF }).unwrap();
        out.push(Pulse { high: false, micros: TIMINGS::HW_SYNC_HALF }).unwrap();
    }
    out.push(Pulse { high: true, micros: TIMINGS::SW_SYNC_HIGH }).unwrap();
    out.push(Pulse { high: false, micros: TIMINGS::HALF_SYMBOL }).unwrap();

    for byte in bytes {
        for bit in (0..8).rev() {
            let one = (byte >> bit) & 1 == 1;
            out.push(Pulse { high: !one, micros: TIMINGS::HALF_SYMBOL }).unwrap();
            out.push(Pulse { high: one, micros: TIMINGS::HALF_SYMBOL }).unwrap();
        }
    }
    out.push(Pulse { high: false, micros: TIMINGS::INTER_FRAME_GAP }).unwrap();
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p somfy-rts --test pulse_tx`
Expected: 4 passed.

- [ ] **Step 5: Port authoritative constants from C++ and commit**

Open `/home/dhia/Sources/personal/ESPSomfy-RTS/src/Somfy.cpp`, locate the TX pulse loop (search for `9415`, `2560`, `4550`, or `delayMicroseconds`/RMT-style tables) and the sync-count logic per frame index. Correct `TIMINGS`, sync counts, and Manchester polarity to match. Re-run tests (update expected values if constants changed).

```bash
git add -A && git commit -m "feat: OOK pulse rendering for RTS frames (wakeup, syncs, Manchester)"
```

---

### Task 6: RX decoder state machine

**Files:**
- Create: `crates/somfy-rts/src/rx.rs`
- Modify: `crates/somfy-rts/src/lib.rs` (add `mod rx; pub use rx::{RxDecoder, RxFrame};`)
- Test: `crates/somfy-rts/tests/rx_loopback.rs`

**Interfaces:**
- Consumes: `render_pulses`, `Pulse`, `FrameKind`, `TIMINGS` (Task 5); `decode56`, `Frame` (Task 2).
- Produces:
  - `pub struct RxFrame { pub bytes: heapless::Vec<u8, 10>, pub bit_length: u8 }`
  - `pub struct RxDecoder { ... }` with:
    - `pub fn new() -> Self`
    - `pub fn push(&mut self, p: Pulse) -> Option<RxFrame>` — feed measured pulses; yields a frame when complete.
    - `pub fn reset(&mut self)`
  - Port of the C++ `somfy_rx_t` state machine (`src/Somfy.h:89-116` + its .cpp driver): states waiting_synchro → receiving_data → complete; ±25% duration tolerance windows; detects 56 vs 80-bit by sync pattern/bit count (mirror the C++ `bit_length` handling).

- [ ] **Step 1: Write failing loopback tests**

`crates/somfy-rts/tests/rx_loopback.rs`:

```rust
use heapless::Vec;
use somfy_rts::{
    decode56, encode56, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder,
};

fn tx_pulses(f: &Frame, kind: FrameKind) -> Vec<Pulse, 320> {
    let mut out = Vec::new();
    render_pulses(&encode56(f), kind, &mut out);
    out
}

fn decode_stream(pulses: &[Pulse]) -> Option<somfy_rts::RxFrame> {
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in pulses {
        if let Some(fr) = rx.push(*p) {
            got = Some(fr);
        }
    }
    got
}

#[test]
fn software_loopback_roundtrip_first_frame() {
    let f = Frame { key: 0xA7, command: Command::Down, rolling_code: 1234, address: 0x0BCDEF };
    let rxf = decode_stream(&tx_pulses(&f, FrameKind::First)).expect("frame decoded");
    assert_eq!(rxf.bit_length, 56);
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn software_loopback_roundtrip_repeat_frame() {
    let f = Frame { key: 0xA1, command: Command::My, rolling_code: 9, address: 0x000001 };
    let rxf = decode_stream(&tx_pulses(&f, FrameKind::Repeat)).expect("frame decoded");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn tolerates_10_percent_timing_jitter() {
    let f = Frame { key: 0xA7, command: Command::Up, rolling_code: 77, address: 0x123456 };
    let mut pulses = tx_pulses(&f, FrameKind::Repeat);
    for (i, p) in pulses.iter_mut().enumerate() {
        let sign: i64 = if i % 2 == 0 { 1 } else { -1 };
        p.micros = (p.micros as i64 + sign * (p.micros as i64 / 10)) as u32;
    }
    let rxf = decode_stream(&pulses).expect("jittered frame decoded");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn noise_before_frame_is_ignored() {
    let f = Frame { key: 0xA7, command: Command::Up, rolling_code: 2, address: 0x424242 };
    let mut stream: Vec<Pulse, 400> = Vec::new();
    for i in 0..40 {
        stream.push(Pulse { high: i % 2 == 0, micros: 137 + i * 13 }).unwrap();
    }
    stream.extend(tx_pulses(&f, FrameKind::Repeat).iter().copied());
    let rxf = decode_stream(&stream).expect("frame found after noise");
    let back = decode56(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test rx_loopback`
Expected: compile FAIL — `RxDecoder` not found.

- [ ] **Step 3: Implement the decoder**

`crates/somfy-rts/src/rx.rs` — port of `somfy_rx_t` from the C++ (`src/Somfy.h:89-116` and its feed logic in `src/Somfy.cpp`, search `waiting_synchro`):

```rust
use crate::pulse::{Pulse, TIMINGS};
use heapless::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RxFrame {
    pub bytes: Vec<u8, 10>,
    pub bit_length: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    WaitingSync,
    ReceivingData,
}

pub struct RxDecoder {
    state: State,
    hw_syncs: u8,
    bit_length: u8,
    bits: u16,
    payload: [u8; 10],
    waiting_half: bool,
    prev_bit: u8,
}

fn within(actual: u32, expected: u32) -> bool {
    // ±25% tolerance window, mirroring the C++ decoder.
    let lo = expected - expected / 4;
    let hi = expected + expected / 4;
    actual >= lo && actual <= hi
}

impl RxDecoder {
    pub fn new() -> Self {
        RxDecoder {
            state: State::WaitingSync,
            hw_syncs: 0,
            bit_length: 56,
            bits: 0,
            payload: [0; 10],
            waiting_half: false,
            prev_bit: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = RxDecoder::new();
    }

    fn store_bit(&mut self, bit: u8) {
        let idx = (self.bits / 8) as usize;
        self.payload[idx] = (self.payload[idx] << 1) | bit;
        self.prev_bit = bit;
        self.bits += 1;
    }

    fn complete(&mut self) -> Option<RxFrame> {
        if self.bits as u8 == self.bit_length {
            let n = (self.bit_length / 8) as usize;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&self.payload[..n]).ok()?;
            let f = RxFrame { bytes, bit_length: self.bit_length };
            self.reset();
            return Some(f);
        }
        None
    }

    /// Feed one measured pulse. Returns a complete frame when the last
    /// data bit arrives.
    pub fn push(&mut self, p: Pulse) -> Option<RxFrame> {
        match self.state {
            State::WaitingSync => {
                if within(p.micros, TIMINGS::HW_SYNC_HALF) {
                    self.hw_syncs += 1;
                } else if self.hw_syncs >= 4 && p.high && within(p.micros, TIMINGS::SW_SYNC_HIGH) {
                    // 56-bit frames follow the standard sw-sync; the C++
                    // decoder selects 80-bit via the extended sync pattern —
                    // mirrored here when porting Task 7.
                    self.state = State::ReceivingData;
                    self.bits = 0;
                    self.payload = [0; 10];
                    self.waiting_half = true; // consume the 604µs low half
                    self.prev_bit = 0;
                } else {
                    self.hw_syncs = 0;
                }
                None
            }
            State::ReceivingData => {
                if within(p.micros, TIMINGS::HALF_SYMBOL) {
                    if self.waiting_half {
                        // second half of a symbol: level identifies the bit,
                        // Manchester high-half == 1 (verify vs C++).
                        self.store_bit(p.high as u8);
                        self.waiting_half = false;
                    } else {
                        self.waiting_half = true;
                    }
                    self.complete()
                } else if within(p.micros, 2 * TIMINGS::HALF_SYMBOL) {
                    // full-symbol pulse: two consecutive identical halves ⇒
                    // this pulse is the 2nd half of one bit and 1st of the next.
                    self.store_bit(p.high as u8);
                    self.waiting_half = true;
                    self.complete()
                } else {
                    // out-of-family duration: frame over (gap) or corrupt
                    let done = self.complete();
                    self.reset();
                    done
                }
            }
        }
    }
}

impl Default for RxDecoder {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests, iterate until green**

Run: `cargo test -p somfy-rts --test rx_loopback`
Expected: 4 passed. The Manchester half-symbol bookkeeping is the fiddly part — if `software_loopback_roundtrip_*` fails, dump the first 20 pulses and walk the state machine by hand; the C++ `Somfy.cpp` RX ISR is the reference for the half-symbol rules.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: RTS RX decoder state machine with software TX/RX loopback tests"
```

---

### Task 7: 80-bit frame support (encode/decode/pulse/RX)

**Files:**
- Modify: `crates/somfy-rts/src/frame.rs` (add `encode80`, `decode80`)
- Modify: `crates/somfy-rts/src/rx.rs` (80-bit sync selection)
- Test: `crates/somfy-rts/tests/frame80.rs`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `pub fn encode80(f: &Frame) -> [u8; 10]`
  - `pub fn decode80(bytes: &[u8; 10]) -> Result<Frame, FrameError>`
  - `RxDecoder` yields `RxFrame { bit_length: 80 }` for 80-bit transmissions.

**IMPORTANT — source of truth:** the 80-bit layout (extended command byte placement, second checksum, sync differences) is NOT public folklore; port it directly from `/home/dhia/Sources/personal/ESPSomfy-RTS/src/Somfy.cpp` — search for `80`, `bit_length`, and the extended-command handling for `StepUp (0x8B)`, `Favorite (0xC1)`, `Stop (0xF1)`. Read that code FIRST, then write the tests to encode its behavior.

- [ ] **Step 1: Read the C++ 80-bit implementation and record the layout**

Read `/home/dhia/Sources/personal/ESPSomfy-RTS/src/Somfy.cpp` sections handling `bit_length == 80`. Write the discovered layout as a doc comment on `encode80` (byte map like the one on `encode56`).

- [ ] **Step 2: Write failing tests (same shape as Task 2/6)**

`crates/somfy-rts/tests/frame80.rs`:

```rust
use heapless::Vec;
use somfy_rts::{
    decode80, encode80, render_pulses, Command, Frame, FrameKind, Pulse, RxDecoder,
};

#[test]
fn roundtrip_80_extended_commands() {
    for cmd in [Command::StepUp, Command::Favorite, Command::Stop] {
        let f = Frame { key: 0xA5, command: cmd, rolling_code: 100, address: 0x654321 };
        let back = decode80(&encode80(&f)).unwrap();
        assert_eq!(back, f, "roundtrip failed for {:?}", cmd);
    }
}

#[test]
fn rx_decoder_recognizes_80_bit_frames() {
    let f = Frame { key: 0xA5, command: Command::StepUp, rolling_code: 5, address: 0x111111 };
    let mut pulses: Vec<Pulse, 320> = Vec::new();
    render_pulses(&encode80(&f), FrameKind::Repeat, &mut pulses);
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in &pulses {
        if let Some(fr) = rx.push(*p) {
            got = Some(fr);
        }
    }
    let rxf = got.expect("80-bit frame decoded");
    assert_eq!(rxf.bit_length, 80);
    let back = decode80(rxf.bytes.as_slice().try_into().unwrap()).unwrap();
    assert_eq!(back, f);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test frame80`
Expected: compile FAIL — `encode80` not found.

- [ ] **Step 4: Implement from the C++ layout recorded in Step 1**

Implement `encode80`/`decode80` in `frame.rs` mirroring the C++ byte map exactly (extended command uses the full `Command as u8` value; obfuscation and checksum rules as found in Step 1). Update `RxDecoder::push` sync handling so the 80-bit sync variant sets `bit_length = 80` the same way the C++ decoder does.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p somfy-rts --test frame80` then full suite `cargo test -p somfy-rts`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: 80-bit RTS frames with extended commands (StepUp/Favorite/Stop)"
```

---

### Task 8: Frame repeat/dedupe policy

**Files:**
- Create: `crates/somfy-rts/src/dedupe.rs`
- Modify: `crates/somfy-rts/src/lib.rs` (add `mod dedupe; pub use dedupe::RxDeduper;`)
- Test: `crates/somfy-rts/tests/dedupe.rs`

**Interfaces:**
- Consumes: `RxFrame` (Task 6), `decode56`/`decode80` (Tasks 2/7).
- Produces:
  - `pub struct RxDeduper { ... }` with `pub fn new(window_ms: u32) -> Self` and `pub fn accept(&mut self, frame: &Frame, now_ms: u32) -> bool` — returns `true` only for the first occurrence of an (address, rolling_code) pair inside the window; RTS remotes transmit each press as 1 first frame + N repeats and the domain layer must see one event per press.

- [ ] **Step 1: Write failing tests**

`crates/somfy-rts/tests/dedupe.rs`:

```rust
use somfy_rts::{Command, Frame, RxDeduper};

fn f(code: u16) -> Frame {
    Frame { key: 0xA0, command: Command::Up, rolling_code: code, address: 0xABCDEF }
}

#[test]
fn repeats_within_window_are_suppressed() {
    let mut d = RxDeduper::new(2000);
    assert!(d.accept(&f(10), 0));
    assert!(!d.accept(&f(10), 50));
    assert!(!d.accept(&f(10), 500));
}

#[test]
fn next_rolling_code_is_a_new_event() {
    let mut d = RxDeduper::new(2000);
    assert!(d.accept(&f(10), 0));
    assert!(d.accept(&f(11), 300));
}

#[test]
fn same_code_after_window_expiry_is_accepted() {
    let mut d = RxDeduper::new(2000);
    assert!(d.accept(&f(10), 0));
    assert!(d.accept(&f(10), 2500));
}

#[test]
fn different_addresses_do_not_collide() {
    let mut d = RxDeduper::new(2000);
    let mut g = f(10);
    g.address = 0x000001;
    assert!(d.accept(&f(10), 0));
    assert!(d.accept(&g, 10));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p somfy-rts --test dedupe`
Expected: compile FAIL.

- [ ] **Step 3: Implement**

`crates/somfy-rts/src/dedupe.rs`:

```rust
use crate::Frame;
use heapless::FnvIndexMap;

/// Collapses RTS repeat frames (1 first + N repeats per button press)
/// into a single logical event per (address, rolling_code) within a window.
pub struct RxDeduper {
    window_ms: u32,
    seen: FnvIndexMap<(u32, u16), u32, 8>,
}

impl RxDeduper {
    pub fn new(window_ms: u32) -> Self {
        RxDeduper { window_ms, seen: FnvIndexMap::new() }
    }

    pub fn accept(&mut self, frame: &Frame, now_ms: u32) -> bool {
        let key = (frame.address, frame.rolling_code);
        if let Some(&t) = self.seen.get(&key) {
            if now_ms.wrapping_sub(t) < self.window_ms {
                return false;
            }
        }
        if self.seen.len() == self.seen.capacity() {
            // Evict the oldest entry to stay bounded.
            if let Some(oldest) = self
                .seen
                .iter()
                .min_by_key(|(_, &t)| now_ms.wrapping_sub(t).wrapping_neg())
                .map(|(k, _)| *k)
            {
                self.seen.remove(&oldest);
            }
        }
        let _ = self.seen.insert(key, now_ms);
        true
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p somfy-rts --test dedupe`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: RX repeat-frame dedupe keyed on address and rolling code"
```

---

### Task 9: Golden capture fixtures from the C++ firmware

**Files:**
- Create: `crates/somfy-rts/tests/fixtures/README.md`
- Create: `crates/somfy-rts/tests/fixtures/*.pulses` (captured data)
- Test: `crates/somfy-rts/tests/golden.rs`

**Interfaces:**
- Consumes: `RxDecoder`, `decode56`, `decode80`.
- Produces: real-world validation; settles Manchester polarity, sync counts, and timing constants against reality. **This task requires the author's running C++ ESPSomfy-RTS device once**; the resulting fixtures are committed so CI never needs hardware.

- [ ] **Step 1: Document the capture procedure**

`crates/somfy-rts/tests/fixtures/README.md`:

```markdown
# Golden pulse captures

Source: ESPSomfy-RTS C++ firmware (this project's reference device).
The C++ RX struct `somfy_rx_t` (src/Somfy.h:95-116) stores raw pulse
durations in `pulses[MAX_TIMINGS]`. Captures were taken by:

1. Enabling the firmware's transceiver debug output (Radio settings →
   enable pulse logging; or add a temporary dump of `rx.pulses` /
   `rx.pulseCount` in src/Somfy.cpp where a completed frame is processed).
2. Pressing a paired remote button near the device per capture file.
3. Saving one line per pulse: `<level 0|1>,<duration_us>`.

File naming: `<command>_<bits>bit_<n>.pulses`, e.g. `up_56bit_1.pulses`.
Each file's expected decode result is in `golden.rs`.
```

- [ ] **Step 2: Capture at least four fixtures on the reference device**

Using the procedure above, capture and save: `up_56bit_1.pulses`, `down_56bit_1.pulses`, `my_56bit_1.pulses`, and one 80-bit capture if a compatible remote is available (else note its absence in the README and defer the 80-bit fixture).

- [ ] **Step 3: Write the golden test**

`crates/somfy-rts/tests/golden.rs`:

```rust
use somfy_rts::{decode56, Command, Pulse, RxDecoder};

fn load(name: &str) -> Vec<Pulse> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (lvl, us) = l.split_once(',').unwrap();
            Pulse { high: lvl.trim() == "1", micros: us.trim().parse().unwrap() }
        })
        .collect()
}

fn decode_capture(name: &str) -> somfy_rts::Frame {
    let mut rx = RxDecoder::new();
    let mut got = None;
    for p in load(name) {
        if let Some(fr) = rx.push(p) {
            got = Some(fr);
        }
    }
    let fr = got.unwrap_or_else(|| panic!("no frame decoded from {name}"));
    decode56(fr.bytes.as_slice().try_into().unwrap()).unwrap()
}

#[test]
fn golden_up_capture_decodes_as_up() {
    assert_eq!(decode_capture("up_56bit_1.pulses").command, Command::Up);
}

#[test]
fn golden_down_capture_decodes_as_down() {
    assert_eq!(decode_capture("down_56bit_1.pulses").command, Command::Down);
}

#[test]
fn golden_my_capture_decodes_as_my() {
    assert_eq!(decode_capture("my_56bit_1.pulses").command, Command::My);
}
```

- [ ] **Step 4: Run golden tests — fix constants/polarity until green**

Run: `cargo test -p somfy-rts --test golden`
Expected: PASS. If FAIL: this is the moment folklore meets reality — adjust `TIMINGS`, Manchester polarity in `render_pulses`/`RxDecoder`, or sync handling to match the captures (and re-run ALL suites: `cargo test -p somfy-rts`). The captures are authoritative.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "test: golden pulse captures from C++ reference firmware"
```

---

### Task 10: CI workflow + crate docs

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `crates/somfy-rts/src/lib.rs` (crate-level docs)
- Create: `README.md`

**Interfaces:**
- Consumes: the full workspace.
- Produces: green CI on every push; documented crate.

- [ ] **Step 1: Write the CI workflow**

`.github/workflows/ci.yml`:

```yaml
name: CI
on:
  push: { branches: [main] }
  pull_request:
jobs:
  host:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "rustfmt, clippy" }
      - run: cargo fmt --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      - name: no_std check
        run: |
          rustup target add thumbv7em-none-eabihf
          cargo build -p somfy-rts --target thumbv7em-none-eabihf
```

(The `thumbv7em` build is a cheap universal `no_std` guard; ESP targets arrive in Plan 4.)

- [ ] **Step 2: Write crate-level docs and README**

`crates/somfy-rts/src/lib.rs` doc header:

```rust
//! # somfy-rts
//!
//! `no_std` Somfy RTS protocol engine: 56/80-bit frame encoding and
//! decoding, rolling-code management, OOK pulse-train rendering (TX)
//! and pulse-stream decoding (RX).
//!
//! This crate is hardware-free: TX produces [`Pulse`] sequences for any
//! replay mechanism (ESP32 RMT, tests), RX consumes measured [`Pulse`]
//! sequences from any capture source (RMT RX, GPIO interrupts, files).
//!
//! Reference implementation: ESPSomfy-RTS (C++). Golden captures from
//! that firmware live in `tests/fixtures/`.
```

`README.md`: project title, one-paragraph description (Rust rewrite of ESPSomfy-RTS; spec in `docs/specs/`), workspace crate table (currently `somfy-rts`), build/test instructions (`cargo test --workspace`), license note.

- [ ] **Step 3: Verify everything, push**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green.

```bash
git add -A && git commit -m "chore: CI workflow, crate docs, README"
```

---

## Self-Review Notes

- **Spec coverage (Plan-1 slice):** spec §3.1 (`somfy-rts` scope) fully covered by Tasks 2–9; §5.4 golden captures = Task 9; the pulse layer's hardware-free contract (§3.1, §5.2–5.3) is enforced by design. Rolling-code persist-before-TX is a caller contract documented in Task 4 (implementation lands in Plan 6 persistence).
- **Known verification points deliberately deferred to reality:** Manchester polarity, sync counts per frame kind, exact timing constants, and the whole 80-bit layout are marked port-from-C++ (Tasks 5/7) and settled by golden captures (Task 9). Tests are written to be updated by those authoritative sources, not to enshrine folklore.
- **Type consistency:** `Frame`/`Command`/`Pulse`/`RxFrame`/`RxDecoder`/`RollingCode`/`RxDeduper` names and signatures are used consistently across Tasks 2–9.
